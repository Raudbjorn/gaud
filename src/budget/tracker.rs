use chrono::{Datelike, NaiveDate, NaiveDateTime, Utc};
use rusqlite::params;
use serde::Serialize;

use crate::db::Database;
use crate::error::AppError;

/// Budget data for a single user.
#[derive(Debug, Clone, Serialize)]
pub struct Budget {
    pub user_id: String,
    pub monthly_limit: Option<f64>,
    pub daily_limit: Option<f64>,
    pub monthly_used: f64,
    pub daily_used: f64,
    pub period_start: String,
    pub day_start: String,
}

/// Result of a budget check.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetStatus {
    /// Budget is within acceptable limits.
    Ok,
    /// Budget usage has crossed the warning threshold (percentage used).
    Warning(f64),
    /// Budget has been exceeded -- requests should be rejected.
    Exceeded,
}

/// Tracks per-user budget allocation and usage against the SQLite database.
pub struct BudgetTracker {
    db: Database,
}

impl BudgetTracker {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Check whether a user is within their budget, resetting period counters
    /// if the current period has elapsed.
    pub fn check_budget(
        &self,
        user_id: &str,
        warning_threshold_percent: u8,
    ) -> Result<BudgetStatus, AppError> {
        // Reset stale periods first.
        self.maybe_reset_periods(user_id)?;

        let budget = match self.get_budget(user_id)? {
            Some(b) => b,
            None => return Ok(BudgetStatus::Ok), // no budget row => unlimited
        };

        // Check monthly limit.
        if let Some(monthly_limit) = budget.monthly_limit {
            if monthly_limit > 0.0 && budget.monthly_used >= monthly_limit {
                return Ok(BudgetStatus::Exceeded);
            }
        }

        // Check daily limit.
        if let Some(daily_limit) = budget.daily_limit {
            if daily_limit > 0.0 && budget.daily_used >= daily_limit {
                return Ok(BudgetStatus::Exceeded);
            }
        }

        // Check warning threshold.
        let threshold = f64::from(warning_threshold_percent) / 100.0;

        if let Some(monthly_limit) = budget.monthly_limit {
            if monthly_limit > 0.0 {
                let pct = budget.monthly_used / monthly_limit * 100.0;
                if budget.monthly_used / monthly_limit >= threshold {
                    return Ok(BudgetStatus::Warning(pct));
                }
            }
        }

        if let Some(daily_limit) = budget.daily_limit {
            if daily_limit > 0.0 {
                let pct = budget.daily_used / daily_limit * 100.0;
                if budget.daily_used / daily_limit >= threshold {
                    return Ok(BudgetStatus::Warning(pct));
                }
            }
        }

        Ok(BudgetStatus::Ok)
    }

    /// Add cost to both daily and monthly usage counters.
    pub fn record_usage(&self, user_id: &str, cost: f64) -> Result<(), AppError> {
        // Reset stale periods first.
        self.maybe_reset_periods(user_id)?;

        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE budgets SET monthly_used = monthly_used + ?1, daily_used = daily_used + ?1 WHERE user_id = ?2",
                params![cost, user_id],
            )?;
            Ok(())
        })?;

        Ok(())
    }

    /// Retrieve the budget row for a user, if one exists.
    pub fn get_budget(&self, user_id: &str) -> Result<Option<Budget>, AppError> {
        let result = self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT user_id, monthly_limit, daily_limit, monthly_used, daily_used, period_start, day_start \
                 FROM budgets WHERE user_id = ?1",
            )?;
            let budget = stmt.query_row(params![user_id], |row| {
                Ok(Budget {
                    user_id: row.get(0)?,
                    monthly_limit: row.get(1)?,
                    daily_limit: row.get(2)?,
                    monthly_used: row.get(3)?,
                    daily_used: row.get(4)?,
                    period_start: row.get(5)?,
                    day_start: row.get(6)?,
                })
            });

            match budget {
                Ok(b) => Ok(Some(b)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e),
            }
        })?;

        Ok(result)
    }

    /// Upsert budget limits for a user.
    pub fn set_budget(
        &self,
        user_id: &str,
        monthly_limit: Option<f64>,
        daily_limit: Option<f64>,
    ) -> Result<(), AppError> {
        let now = Utc::now();
        let period_start = now.format("%Y-%m-%d %H:%M:%S").to_string();
        let day_start = now.format("%Y-%m-%d").to_string();

        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO budgets (user_id, monthly_limit, daily_limit, period_start, day_start) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(user_id) DO UPDATE SET \
                   monthly_limit = ?2, \
                   daily_limit = ?3",
                params![user_id, monthly_limit, daily_limit, period_start, day_start],
            )?;
            Ok(())
        })?;

        tracing::info!(
            user_id = %user_id,
            monthly_limit = ?monthly_limit,
            daily_limit = ?daily_limit,
            "Budget set"
        );
        Ok(())
    }

    /// Reset monthly/daily counters if the current period has elapsed.
    fn maybe_reset_periods(&self, user_id: &str) -> Result<(), AppError> {
        let budget = match self.get_budget(user_id)? {
            Some(b) => b,
            None => return Ok(()),
        };

        let now = Utc::now().naive_utc();
        let mut needs_monthly_reset = false;
        let mut needs_daily_reset = false;

        // Check monthly period.
        if let Ok(period_start) =
            NaiveDateTime::parse_from_str(&budget.period_start, "%Y-%m-%d %H:%M:%S")
        {
            // Reset if we're in a new month relative to period_start.
            let next_month = add_one_month(period_start);
            if now >= next_month {
                needs_monthly_reset = true;
            }
        }

        // Check daily period.
        if let Ok(day_start) = NaiveDate::parse_from_str(&budget.day_start, "%Y-%m-%d") {
            let today = now.date();
            if today > day_start {
                needs_daily_reset = true;
            }
        }

        if needs_monthly_reset || needs_daily_reset {
            let new_period_start = now.format("%Y-%m-%d %H:%M:%S").to_string();
            let new_day_start = now.format("%Y-%m-%d").to_string();

            self.db.with_conn(|conn| {
                if needs_monthly_reset && needs_daily_reset {
                    conn.execute(
                        "UPDATE budgets SET monthly_used = 0.0, daily_used = 0.0, \
                         period_start = ?1, day_start = ?2 WHERE user_id = ?3",
                        params![new_period_start, new_day_start, user_id],
                    )?;
                } else if needs_monthly_reset {
                    conn.execute(
                        "UPDATE budgets SET monthly_used = 0.0, period_start = ?1 WHERE user_id = ?2",
                        params![new_period_start, user_id],
                    )?;
                } else {
                    conn.execute(
                        "UPDATE budgets SET daily_used = 0.0, day_start = ?1 WHERE user_id = ?2",
                        params![new_day_start, user_id],
                    )?;
                }
                Ok(())
            })?;
        }

        Ok(())
    }
}

/// Add one month to a NaiveDateTime, clamping the day to the last day of the
/// target month.
fn add_one_month(dt: NaiveDateTime) -> NaiveDateTime {
    let (year, month) = if dt.month() == 12 {
        (dt.year() + 1, 1)
    } else {
        (dt.year(), dt.month() + 1)
    };

    // Clamp day to max days in target month.
    let max_day = days_in_month(year, month);
    let day = dt.day().min(max_day);

    NaiveDate::from_ymd_opt(year, month, day)
        .unwrap_or(dt.date())
        .and_time(dt.time())
}

/// Number of days in a given month.
fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        // Create a test user for FK constraints.
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO users (id, name, role) VALUES ('user1', 'alice', 'member')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        db
    }

    #[test]
    fn test_set_and_get_budget() {
        let db = test_db();
        let tracker = BudgetTracker::new(db);

        tracker
            .set_budget("user1", Some(100.0), Some(10.0))
            .unwrap();
        let budget = tracker.get_budget("user1").unwrap().unwrap();
        assert_eq!(budget.monthly_limit, Some(100.0));
        assert_eq!(budget.daily_limit, Some(10.0));
        assert_eq!(budget.monthly_used, 0.0);
        assert_eq!(budget.daily_used, 0.0);
    }

    #[test]
    fn test_get_budget_nonexistent() {
        let db = test_db();
        let tracker = BudgetTracker::new(db);

        let budget = tracker.get_budget("user1").unwrap();
        assert!(budget.is_none());
    }

    #[test]
    fn test_record_usage() {
        let db = test_db();
        let tracker = BudgetTracker::new(db);

        tracker
            .set_budget("user1", Some(100.0), Some(10.0))
            .unwrap();
        tracker.record_usage("user1", 5.0).unwrap();

        let budget = tracker.get_budget("user1").unwrap().unwrap();
        assert!((budget.monthly_used - 5.0).abs() < f64::EPSILON);
        assert!((budget.daily_used - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_budget_check_ok() {
        let db = test_db();
        let tracker = BudgetTracker::new(db);

        tracker
            .set_budget("user1", Some(100.0), Some(10.0))
            .unwrap();
        let status = tracker.check_budget("user1", 80).unwrap();
        assert_eq!(status, BudgetStatus::Ok);
    }

    #[test]
    fn test_budget_check_no_budget_row() {
        let db = test_db();
        let tracker = BudgetTracker::new(db);

        let status = tracker.check_budget("user1", 80).unwrap();
        assert_eq!(status, BudgetStatus::Ok);
    }

    #[test]
    fn test_budget_check_warning() {
        let db = test_db();
        let tracker = BudgetTracker::new(db);

        // Use a daily limit higher than the usage so the daily check does not
        // return Exceeded before the monthly warning threshold is evaluated.
        tracker
            .set_budget("user1", Some(100.0), Some(200.0))
            .unwrap();
        tracker.record_usage("user1", 85.0).unwrap();

        let status = tracker.check_budget("user1", 80).unwrap();
        assert!(matches!(status, BudgetStatus::Warning(_)));
    }

    #[test]
    fn test_budget_check_exceeded_monthly() {
        let db = test_db();
        let tracker = BudgetTracker::new(db);

        tracker
            .set_budget("user1", Some(100.0), Some(200.0))
            .unwrap();
        tracker.record_usage("user1", 100.0).unwrap();

        let status = tracker.check_budget("user1", 80).unwrap();
        assert_eq!(status, BudgetStatus::Exceeded);
    }

    #[test]
    fn test_budget_check_exceeded_daily() {
        let db = test_db();
        let tracker = BudgetTracker::new(db);

        tracker
            .set_budget("user1", Some(1000.0), Some(10.0))
            .unwrap();
        tracker.record_usage("user1", 10.0).unwrap();

        let status = tracker.check_budget("user1", 80).unwrap();
        assert_eq!(status, BudgetStatus::Exceeded);
    }

    #[test]
    fn test_set_budget_upsert() {
        let db = test_db();
        let tracker = BudgetTracker::new(db);

        tracker
            .set_budget("user1", Some(100.0), Some(10.0))
            .unwrap();
        tracker
            .set_budget("user1", Some(200.0), Some(20.0))
            .unwrap();

        let budget = tracker.get_budget("user1").unwrap().unwrap();
        assert_eq!(budget.monthly_limit, Some(200.0));
        assert_eq!(budget.daily_limit, Some(20.0));
    }

    #[test]
    fn test_budget_with_no_limits() {
        let db = test_db();
        let tracker = BudgetTracker::new(db);

        tracker.set_budget("user1", None, None).unwrap();
        tracker.record_usage("user1", 999.0).unwrap();

        let status = tracker.check_budget("user1", 80).unwrap();
        assert_eq!(status, BudgetStatus::Ok);
    }

    #[test]
    fn test_add_one_month_normal() {
        let dt = NaiveDate::from_ymd_opt(2025, 1, 15)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let result = add_one_month(dt);
        assert_eq!(result.month(), 2);
        assert_eq!(result.day(), 15);
    }

    #[test]
    fn test_add_one_month_december() {
        let dt = NaiveDate::from_ymd_opt(2025, 12, 15)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let result = add_one_month(dt);
        assert_eq!(result.year(), 2026);
        assert_eq!(result.month(), 1);
    }

    #[test]
    fn test_add_one_month_day_clamping() {
        // January 31 -> February 28 (non-leap year)
        let dt = NaiveDate::from_ymd_opt(2025, 1, 31)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let result = add_one_month(dt);
        assert_eq!(result.month(), 2);
        assert_eq!(result.day(), 28);
    }

    #[test]
    fn test_days_in_month_leap_year() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2025, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
    }

    #[test]
    fn test_record_usage_accumulates() {
        let db = test_db();
        let tracker = BudgetTracker::new(db);

        tracker
            .set_budget("user1", Some(100.0), Some(50.0))
            .unwrap();
        tracker.record_usage("user1", 5.0).unwrap();
        tracker.record_usage("user1", 3.0).unwrap();
        tracker.record_usage("user1", 2.0).unwrap();

        let budget = tracker.get_budget("user1").unwrap().unwrap();
        assert!((budget.monthly_used - 10.0).abs() < f64::EPSILON);
        assert!((budget.daily_used - 10.0).abs() < f64::EPSILON);
    }
}
