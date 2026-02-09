use std::time::Duration;

use rusqlite::params;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::budget::AuditEntry;
use crate::budget::BudgetTracker;
use crate::db::Database;

/// Maximum number of entries to buffer before flushing, regardless of timer.
const BATCH_SIZE: usize = 100;

/// How often to flush buffered entries even if the batch is not full.
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// Spawn a background task that reads `AuditEntry` values from the channel
/// and batch-writes them to the `usage_log` table.  Also atomically updates
/// the user's `monthly_used` / `daily_used` counters in the `budgets` table.
///
/// The returned `JoinHandle` can be used to wait for graceful shutdown (the
/// task exits when the sender half is dropped and remaining entries are
/// flushed).
pub fn spawn_audit_logger(
    db: Database,
    _budget: std::sync::Arc<BudgetTracker>,
    mut rx: mpsc::UnboundedReceiver<AuditEntry>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buffer: Vec<AuditEntry> = Vec::with_capacity(BATCH_SIZE);
        let mut interval = tokio::time::interval(FLUSH_INTERVAL);
        // Don't pile up ticks while we're busy flushing.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                entry = rx.recv() => {
                    match entry {
                        Some(e) => {
                            buffer.push(e);
                            if buffer.len() >= BATCH_SIZE {
                                flush_batch(&db, &mut buffer);
                            }
                        }
                        None => {
                            // Channel closed -- flush remaining and exit.
                            if !buffer.is_empty() {
                                flush_batch(&db, &mut buffer);
                            }
                            tracing::info!("Audit logger shutting down");
                            break;
                        }
                    }
                }
                _ = interval.tick() => {
                    if !buffer.is_empty() {
                        flush_batch(&db, &mut buffer);
                    }
                }
            }
        }
    })
}

/// Write a batch of audit entries to the database in a single transaction.
fn flush_batch(db: &Database, buffer: &mut Vec<AuditEntry>) {
    let entries = std::mem::take(buffer);
    let count = entries.len();

    if let Err(e) = write_entries(db, &entries) {
        tracing::error!(count, error = %e, "Failed to flush audit batch");
        // Put entries back so we can retry on the next tick.
        buffer.extend(entries);
    } else {
        tracing::debug!(count, "Flushed audit batch");
    }
}

/// Perform the actual DB writes inside a transaction.
///
/// Inserts rows into `usage_log` and atomically updates the `budgets` table
/// counters (`monthly_used`, `daily_used`) for each entry with a non-zero cost.
fn write_entries(
    db: &Database,
    entries: &[AuditEntry],
) -> Result<(), Box<dyn std::error::Error>> {
    db.with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;

        {
            let mut insert_stmt = tx.prepare_cached(
                "INSERT INTO usage_log (id, user_id, request_id, provider, model, \
                 input_tokens, output_tokens, cost, latency_ms, status) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;

            let mut update_budget_stmt = tx.prepare_cached(
                "UPDATE budgets SET monthly_used = monthly_used + ?1, \
                 daily_used = daily_used + ?1 WHERE user_id = ?2",
            )?;

            for entry in entries {
                let id = Uuid::new_v4().to_string();
                insert_stmt.execute(params![
                    id,
                    entry.user_id,
                    entry.request_id,
                    entry.provider,
                    entry.model,
                    entry.input_tokens,
                    entry.output_tokens,
                    entry.cost,
                    entry.latency_ms,
                    entry.status,
                ])?;

                // Update budget counters atomically within the same transaction.
                if entry.cost > 0.0 {
                    update_budget_stmt.execute(params![entry.cost, entry.user_id])?;
                }
            }
        }

        tx.commit()?;
        Ok(())
    })
    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::BudgetTracker;
    use std::sync::Arc;

    fn test_db() -> Database {
        let db = Database::open_in_memory().unwrap();
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

    fn make_entry(user_id: &str, cost: f64) -> AuditEntry {
        AuditEntry {
            user_id: user_id.to_string(),
            request_id: Uuid::new_v4().to_string(),
            provider: "test".to_string(),
            model: "test-model".to_string(),
            input_tokens: 100,
            output_tokens: 50,
            cost,
            latency_ms: 200,
            status: "success".to_string(),
        }
    }

    #[test]
    fn test_write_entries_inserts_usage_log() {
        let db = test_db();
        let entries = vec![make_entry("user1", 0.5)];

        write_entries(&db, &entries).unwrap();

        let count: i64 = db
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM usage_log", [], |row| row.get(0))
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_write_entries_updates_budget() {
        let db = test_db();
        let budget = BudgetTracker::new(db.clone());

        // Set up a budget row first.
        budget.set_budget("user1", Some(100.0), Some(10.0)).unwrap();

        let entries = vec![make_entry("user1", 2.5)];
        write_entries(&db, &entries).unwrap();

        let b = budget.get_budget("user1").unwrap().unwrap();
        assert!((b.monthly_used - 2.5).abs() < f64::EPSILON);
        assert!((b.daily_used - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_write_entries_batch() {
        let db = test_db();

        let entries: Vec<AuditEntry> = (0..10).map(|_| make_entry("user1", 0.1)).collect();
        write_entries(&db, &entries).unwrap();

        let count: i64 = db
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM usage_log", [], |row| row.get(0))
            })
            .unwrap();
        assert_eq!(count, 10);
    }

    #[test]
    fn test_write_entries_zero_cost_skips_budget_update() {
        let db = test_db();
        let budget = BudgetTracker::new(db.clone());

        budget.set_budget("user1", Some(100.0), Some(10.0)).unwrap();

        let entries = vec![make_entry("user1", 0.0)];
        write_entries(&db, &entries).unwrap();

        let b = budget.get_budget("user1").unwrap().unwrap();
        assert!((b.monthly_used - 0.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_spawn_audit_logger_flushes_on_close() {
        let db = test_db();
        let budget = Arc::new(BudgetTracker::new(db.clone()));
        let (tx, rx) = mpsc::unbounded_channel();

        let handle = spawn_audit_logger(db.clone(), budget, rx);

        tx.send(make_entry("user1", 1.0)).unwrap();
        tx.send(make_entry("user1", 2.0)).unwrap();

        // Drop the sender to trigger shutdown.
        drop(tx);

        // Wait for the logger to finish.
        handle.await.unwrap();

        let count: i64 = db
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM usage_log", [], |row| row.get(0))
            })
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_spawn_audit_logger_periodic_flush() {
        let db = test_db();
        let budget = Arc::new(BudgetTracker::new(db.clone()));
        let (tx, rx) = mpsc::unbounded_channel();

        let _handle = spawn_audit_logger(db.clone(), budget, rx);

        tx.send(make_entry("user1", 0.5)).unwrap();

        // Wait for the periodic flush (1 second + margin).
        tokio::time::sleep(Duration::from_millis(1500)).await;

        let count: i64 = db
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM usage_log", [], |row| row.get(0))
            })
            .unwrap();
        assert_eq!(count, 1);

        drop(tx);
    }
}
