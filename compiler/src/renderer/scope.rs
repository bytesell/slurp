use serde_json::Value;
use std::collections::HashMap;

pub struct Scope<'a> {
    bindings: HashMap<String, Value>,
    parent: Option<&'a Scope<'a>>,
}

impl<'a> Scope<'a> {
    /// Create a root scope from a JSON context object.
    ///
    /// Returns `Scope<'static>` because the root has no parent reference - its
    /// bindings are fully owned.  Child scopes borrow the parent and so get
    /// shorter lifetimes.
    #[must_use]
    pub fn root(context: &Value) -> Scope<'static> {
        let bindings = match context {
            Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            _ => HashMap::new(),
        };
        Scope {
            bindings,
            parent: None,
        }
    }

    pub fn child<'b>(&'b self) -> Scope<'b> {
        Scope {
            bindings: HashMap::new(),
            parent: Some(self),
        }
    }

    pub fn set(&mut self, name: impl Into<String>, value: Value) {
        self.bindings.insert(name.into(), value);
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Value {
        if let Some(v) = self.bindings.get(name) {
            return v.clone();
        }
        if let Some(parent) = self.parent {
            return parent.get(name);
        }
        Value::Null
    }

    #[must_use]
    pub fn resolve_path(&self, segments: &[crate::ast::PathSegment]) -> Value {
        use crate::ast::PathSegment;

        if segments.is_empty() {
            return Value::Null;
        }

        // Fast path: the loop auto-vars (`loop.index`, `loop.count`, `loop.first`,
        // `loop.last`) are stored under a FLAT dotted key. Restrict this flat
        // lookup to exactly those `loop.*` bindings - using it for arbitrary
        // dotted paths would let a literal `"a.b"` key shadow real nested
        // traversal of `a -> b` (and is non-deterministic on collision).
        if segments.len() == 2
            && let (PathSegment::Ident(root), PathSegment::Ident(leaf)) =
                (&segments[0], &segments[1])
            && root == "loop"
            && matches!(leaf.as_str(), "index" | "count" | "first" | "last")
        {
            let v = self.get(&format!("loop.{leaf}"));
            if !v.is_null() {
                return v;
            }
        }

        let mut current = match &segments[0] {
            PathSegment::Ident(name) => self.get(name),
            PathSegment::Index(_expr) => {
                // Index at root - unusual but handle gracefully
                Value::Null
            }
        };

        for seg in &segments[1..] {
            if current.is_null() {
                return Value::Null;
            }
            current = match seg {
                PathSegment::Ident(key) => match &current {
                    Value::Object(map) => map.get(key).cloned().unwrap_or(Value::Null),
                    _ => Value::Null,
                },
                PathSegment::Index(idx_expr) => index_into(&current, idx_expr),
            };
        }

        current
    }
}

/// Resolve a `path[index]` segment for the constant-index case (`arr[0]`,
/// `obj["key"]`).
///
/// A non-constant index (`arr[i]`) is NOT a separate `ExprKind::Index` - a subscript
/// written directly on a path is part of that path, as a `PathSegment::Index`, and it
/// arrives here like any other. This function has no renderer and cannot evaluate `i`,
/// so `Renderer::resolve_index_segments` evaluates such subscripts to constants BEFORE
/// calling `resolve_path`. Skip that pre-pass and every dynamic index on a bare path
/// renders empty, with no diagnostic.
fn index_into(current: &Value, idx_expr: &crate::ast::Expression) -> Value {
    use crate::ast::ExprKind;
    // Only bare literal indices are resolvable without a renderer/scope.
    if !idx_expr.filters.is_empty() {
        return Value::Null;
    }
    match (&idx_expr.kind, current) {
        (ExprKind::NumberLiteral(n), Value::Array(arr)) => {
            if *n < 0.0 {
                return Value::Null;
            }
            arr.get(*n as usize).cloned().unwrap_or(Value::Null)
        }
        (ExprKind::StringLiteral(key), Value::Object(map)) => {
            map.get(key).cloned().unwrap_or(Value::Null)
        }
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::PathSegment;
    use serde_json::json;

    #[test]
    fn test_root_scope_lookup() {
        let ctx = json!({"name": "Alice", "age": 30});
        let scope = Scope::root(&ctx);
        assert_eq!(scope.get("name"), json!("Alice"));
        assert_eq!(scope.get("age"), json!(30));
    }

    #[test]
    fn test_missing_key_returns_null() {
        let ctx = json!({"name": "Alice"});
        let scope = Scope::root(&ctx);
        assert_eq!(scope.get("nonexistent"), Value::Null);
    }

    #[test]
    fn test_child_scope_inherits() {
        let ctx = json!({"name": "Alice"});
        let parent = Scope::root(&ctx);
        let mut child = parent.child();
        child.set("extra", json!("bonus"));
        assert_eq!(child.get("name"), json!("Alice")); // inherited
        assert_eq!(child.get("extra"), json!("bonus")); // local
    }

    #[test]
    fn test_child_shadows_parent() {
        let ctx = json!({"x": 1});
        let parent = Scope::root(&ctx);
        let mut child = parent.child();
        child.set("x", json!(99));
        assert_eq!(child.get("x"), json!(99)); // shadowed
        assert_eq!(parent.get("x"), json!(1)); // parent unchanged
    }

    #[test]
    fn test_resolve_path_single() {
        let ctx = json!({"user": {"name": "Bob"}});
        let scope = Scope::root(&ctx);
        let path = vec![
            PathSegment::Ident("user".into()),
            PathSegment::Ident("name".into()),
        ];
        assert_eq!(scope.resolve_path(&path), json!("Bob"));
    }

    #[test]
    fn test_resolve_path_null_safety() {
        let ctx = json!({"user": null});
        let scope = Scope::root(&ctx);
        let path = vec![
            PathSegment::Ident("user".into()),
            PathSegment::Ident("name".into()),
            PathSegment::Ident("first".into()),
        ];
        // Safe navigation: null at "user" level → null, not error
        assert_eq!(scope.resolve_path(&path), Value::Null);
    }

    #[test]
    fn test_resolve_path_missing_intermediate() {
        let ctx = json!({"a": {"b": 1}});
        let scope = Scope::root(&ctx);
        let path = vec![
            PathSegment::Ident("a".into()),
            PathSegment::Ident("c".into()),
            PathSegment::Ident("d".into()),
        ];
        assert_eq!(scope.resolve_path(&path), Value::Null);
    }

    #[test]
    fn test_resolve_path_literal_array_index() {
        use crate::ast::{ExprKind, Expression};
        // products[0].name with a literal index now resolves instead of yielding null.
        let ctx = json!({"products": [{"name": "Widget"}, {"name": "Gadget"}]});
        let scope = Scope::root(&ctx);
        let path = vec![
            PathSegment::Ident("products".into()),
            PathSegment::Index(Box::new(Expression::new(
                ExprKind::NumberLiteral(1.0),
                0,
                0,
            ))),
            PathSegment::Ident("name".into()),
        ];
        assert_eq!(scope.resolve_path(&path), json!("Gadget"));
    }

    #[test]
    fn test_resolve_path_dotted_key_is_not_flat_shadowed() {
        // A literal "a.b" key must NOT shadow real nested traversal of a -> b.
        let ctx = json!({"a": {"b": "nested"}, "a.b": "flat"});
        let scope = Scope::root(&ctx);
        let path = vec![
            PathSegment::Ident("a".into()),
            PathSegment::Ident("b".into()),
        ];
        assert_eq!(scope.resolve_path(&path), json!("nested"));
    }
}
