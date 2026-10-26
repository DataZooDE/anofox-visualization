//! The shared BOOLEAN-condition evaluator behind `::SHOW_IF` (#515) and,
//! later, `::GATE` (#516). One rule everywhere: a condition **holds** only
//! when its value is literally `TRUE` — `FALSE`, `NULL`, no row, a
//! non-boolean value or a query error all count as "does not hold" (fail
//! closed, which is what a quiz or a gate needs). See `src/sql/show_if.rs`
//! for how a condition's "range" (what it governs) is computed, and
//! `web/app.js`'s `condHolds` / `conditionHolds` for the browser twin.

use ggplot_rs::prelude::Value;

/// Does a native query result's value hold? `true` only for `Value::Bool(true)`.
pub fn holds(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Bool(true)))
}

/// Does a served/JSON result's value hold? `true` only for the JSON boolean
/// `true`. DuckDB-wasm and the served JSON API both give booleans as JS
/// `true`/`false` (verified by the served e2e), but a `"true"` string is
/// accepted too, in case either side ever stringifies one — see
/// `web/app.js`'s `condHolds`, which must agree.
pub fn holds_json(v: Option<&serde_json::Value>) -> bool {
    match v {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => s == "true",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_is_true_only_for_true() {
        assert!(holds(Some(&Value::Bool(true))));
        assert!(!holds(Some(&Value::Bool(false))));
        assert!(!holds(Some(&Value::Na)));
        assert!(!holds(None));
        assert!(!holds(Some(&Value::Str("true".into()))));
        assert!(!holds(Some(&Value::Float(1.0))));
    }

    #[test]
    fn holds_json_is_true_only_for_true() {
        assert!(holds_json(Some(&serde_json::json!(true))));
        assert!(!holds_json(Some(&serde_json::json!(false))));
        assert!(!holds_json(Some(&serde_json::Value::Null)));
        assert!(!holds_json(None));
        assert!(holds_json(Some(&serde_json::json!("true"))));
        assert!(!holds_json(Some(&serde_json::json!("yes"))));
        assert!(!holds_json(Some(&serde_json::json!(1))));
    }
}
