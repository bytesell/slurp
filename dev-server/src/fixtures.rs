//! Backend-agnostic fixtures source for offline theme preview.
//!
//! A fixtures file is a JSON object mapping a request path to the canned JSON response to return
//! for it, e.g.
//!
//! ```json
//! {
//!   "/api/products": [ { "name": "Example" } ],
//!   "/api/config":   { "name": "Demo Store" }
//! }
//! ```
//!
//! With `--fixtures` set, `slurp-dev` resolves `{fetch}` URLs and answers `/api`, `/auth` requests
//! from this map instead of a live backend, so a theme renders with no backend running. slurp knows
//! nothing about what the paths mean - it replays whatever JSON the file records for a given path.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

/// A loaded fixtures map: request path -> canned JSON response.
#[derive(Debug, Clone, Default)]
pub struct Fixtures {
    routes: HashMap<String, Value>,
}

impl Fixtures {
    /// Load fixtures from a JSON file, or from `<dir>/fixtures.json` when a directory is given.
    ///
    /// The file must be a JSON object whose keys are request paths (e.g. `"/api/products"`) and
    /// whose values are the JSON responses to return for them.
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = if path.is_dir() {
            path.join("fixtures.json")
        } else {
            path.to_path_buf()
        };
        let raw = std::fs::read_to_string(&file)
            .map_err(|e| format!("cannot read fixtures file {}: {e}", file.display()))?;
        Self::from_str(&raw).map_err(|e| format!("fixtures file {}: {e}", file.display()))
    }

    /// Parse fixtures from a JSON string (the map form). Separated from [`load`] for testing.
    pub fn from_str(raw: &str) -> Result<Self, String> {
        let parsed: Value =
            serde_json::from_str(raw).map_err(|e| format!("not valid JSON: {e}"))?;
        let obj = parsed
            .as_object()
            .ok_or_else(|| "must be a JSON object mapping request path -> response".to_string())?;
        let mut routes = HashMap::with_capacity(obj.len());
        for (k, v) in obj {
            routes.insert(k.trim().to_string(), v.clone());
        }
        Ok(Self { routes })
    }

    /// Look up a canned response for a request URL/path. Tries the exact string first, then falls
    /// back to the path with any `?query` stripped, so `"/api/products?limit=10"` still matches a
    /// `"/api/products"` fixture.
    pub fn lookup(&self, url: &str) -> Option<&Value> {
        let u = url.trim();
        if let Some(v) = self.routes.get(u) {
            return Some(v);
        }
        if let Some((path, _query)) = u.split_once('?') {
            return self.routes.get(path);
        }
        None
    }

    /// Number of fixture routes loaded.
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// True when no fixtures were loaded. Kept as the idiomatic companion to [`len`].
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Fixtures {
        Fixtures::from_str(
            r#"{
                "/api/config": { "name": "Demo Store" },
                "/api/products": [ { "name": "Widget" } ]
            }"#,
        )
        .expect("valid fixtures")
    }

    #[test]
    fn loads_and_counts_routes() {
        let f = sample();
        assert_eq!(f.len(), 2);
        assert!(!f.is_empty());
    }

    #[test]
    fn exact_match() {
        let f = sample();
        assert_eq!(f.lookup("/api/config").unwrap()["name"], "Demo Store");
    }

    #[test]
    fn query_string_falls_back_to_path() {
        let f = sample();
        // No exact key with the query, so it should fall back to the bare path.
        let v = f.lookup("/api/products?limit=10").unwrap();
        assert_eq!(v[0]["name"], "Widget");
    }

    #[test]
    fn miss_returns_none() {
        let f = sample();
        assert!(f.lookup("/api/nope").is_none());
    }

    #[test]
    fn rejects_non_object() {
        let err = Fixtures::from_str("[1, 2, 3]").unwrap_err();
        assert!(err.contains("JSON object"));
    }

    #[test]
    fn rejects_bad_json() {
        assert!(Fixtures::from_str("{not json").is_err());
    }
}
