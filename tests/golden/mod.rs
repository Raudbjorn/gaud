use std::path::PathBuf;
use std::fs;
use serde::de::DeserializeOwned;
use pretty_assertions::assert_eq;

mod gemini;

pub struct GoldenTest {
    root: PathBuf,
}

impl GoldenTest {
    pub fn new(suite: &str) -> Self {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
        let root = PathBuf::from(manifest_dir).join("tests").join("golden").join("data").join(suite);
        Self { root }
    }

    pub fn load_json<T: DeserializeOwned>(&self, name: &str) -> T {
        let path = self.root.join(format!("{}.json", name));
        let content = fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("Failed to read golden file: {:?}", path));
        serde_json::from_str(&content)
            .unwrap_or_else(|e| panic!("Failed to parse golden file {:?}: {}", path, e))
    }

    pub fn assert_json<T: serde::Serialize + std::fmt::Debug>(&self, name: &str, actual: &T) {
        let expected_path = self.root.join(format!("{}.json", name));

        // Serialize actual to pretty JSON
        let actual_json = serde_json::to_string_pretty(actual).expect("Failed to serialize actual value");

        if !expected_path.exists() {
            // implementing update mode would be nice here in future
             panic!(
                "Golden file missing: {:?}.\nActual content:\n{}",
                expected_path, actual_json
            );
        }

        let expected_content = fs::read_to_string(&expected_path)
            .expect("Failed to read expected golden file");

        // Compare as generic Value to ignore whitespace diffs if we want,
        // but string comparison enforces formatting which is good for snapshot stability.
        // Let's compare parsed values to be robust against formatting changes.
        let expected_val: serde_json::Value = serde_json::from_str(&expected_content)
            .expect("Failed to parse expected golden file");
        let actual_val: serde_json::Value = serde_json::from_str(&actual_json)
            .expect("Failed to parse actual json");

        assert_eq!(expected_val, actual_val, "Golden test failed for {}", name);
    }
}
