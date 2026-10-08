//! Host-boundary helpers shared by every embedding of the renderer: the DuckDB
//! extensions (C-API `duckext/` and C++ `crates/anofox-viz-ffi`) and the
//! `serve` binary.
//!
//! * [`render_spec_checked`] — the render entry point for FFI callers. Unlike
//!   [`crate::render_spec`] (which returns a `<pre>error</pre>` string so the
//!   browser can show it inline), it validates the spec, enforces size caps and
//!   returns a real `Err` so hosts can raise a SQL error. Panics inside the
//!   renderer are caught and turned into errors as well.
//! * [`serving`] — HTTP/serving helpers (escaping, request-origin checks,
//!   tokens, typed SQL literals, dashboard header parsing) shared by
//!   `src/bin/serve.rs` and the extension's `anofox_serve*` functions.

pub mod serving;

use std::panic::{catch_unwind, AssertUnwindSafe};

/// Upper bounds applied by [`render_spec_checked`].
#[derive(Clone, Copy, Debug)]
pub struct RenderLimits {
    /// Max number of data rows in `spec.rows`.
    pub max_rows: usize,
    /// Max size of the JSON spec in bytes.
    pub max_spec_bytes: usize,
    /// Allowed range for `width` / `height` (pixels).
    pub min_dim: u64,
    pub max_dim: u64,
}

impl Default for RenderLimits {
    fn default() -> Self {
        RenderLimits {
            max_rows: 200_000,
            max_spec_bytes: 64 << 20,
            min_dim: 16,
            max_dim: 8192,
        }
    }
}

/// Run `f`, converting a panic into `Err(message)`. Used at every FFI boundary
/// (an unwinding panic must never cross an `extern "C"` frame).
pub fn catch_panic<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            Err(format!("anofox_render: internal error (panic): {msg}"))
        }
    }
}

/// Render a JSON panel spec (bytes, as handed over an FFI boundary) to SVG.
/// Invalid UTF-8, malformed JSON, unknown roles, out-of-range sizes, too many
/// rows, renderer errors and renderer panics all return `Err`.
pub fn render_spec_bytes_checked(spec: &[u8], limits: &RenderLimits) -> Result<String, String> {
    let s = std::str::from_utf8(spec)
        .map_err(|e| format!("anofox_render: spec is not valid UTF-8 ({e})"))?;
    render_spec_checked(s, limits)
}

/// See [`render_spec_bytes_checked`].
pub fn render_spec_checked(spec_json: &str, limits: &RenderLimits) -> Result<String, String> {
    if spec_json.len() > limits.max_spec_bytes {
        return Err(format!(
            "anofox_render: spec is {} bytes, the limit is {}",
            spec_json.len(),
            limits.max_spec_bytes
        ));
    }
    let spec: serde_json::Value = serde_json::from_str(spec_json)
        .map_err(|e| format!("anofox_render: bad spec JSON: {e}"))?;
    let obj = spec
        .as_object()
        .ok_or("anofox_render: spec must be a JSON object")?;
    match obj.get("rows") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Array(rows)) => {
            if rows.len() > limits.max_rows {
                return Err(format!(
                    "anofox_render: {} rows exceed the limit of {}",
                    rows.len(),
                    limits.max_rows
                ));
            }
            if rows.iter().any(|r| !r.is_object()) {
                return Err("anofox_render: every entry of 'rows' must be a JSON object".into());
            }
        }
        Some(_) => return Err("anofox_render: 'rows' must be a JSON array".into()),
    }
    let roles = obj.get("roles").and_then(|v| v.as_array()).ok_or(
        "anofox_render: spec needs a 'roles' array, e.g. [[0,\"XAXIS\"],[1,\"BARCHART\"]]",
    )?;
    for r in roles {
        let e = r
            .as_array()
            .ok_or("anofox_render: each role must be [column_index, \"ROLE\"(, \"name\")]")?;
        if e.first().and_then(|v| v.as_u64()).is_none() {
            return Err("anofox_render: role column index must be a non-negative integer".into());
        }
        match e.get(1) {
            Some(serde_json::Value::String(name)) => {
                if crate::parse_role(name).is_none() {
                    return Err(format!("anofox_render: unknown role '{name}'"));
                }
            }
            Some(serde_json::Value::Null) | None => {
                return Err("anofox_render: role name is NULL/missing".into())
            }
            Some(other) => {
                return Err(format!(
                    "anofox_render: role name must be a string, got {other}"
                ))
            }
        }
    }
    for key in ["width", "height"] {
        match obj.get(key) {
            None | Some(serde_json::Value::Null) => {}
            Some(v) => {
                let n = v
                    .as_u64()
                    .ok_or_else(|| format!("anofox_render: '{key}' must be a positive integer"))?;
                if n < limits.min_dim || n > limits.max_dim {
                    return Err(format!(
                        "anofox_render: '{key}' = {n} is outside [{}, {}]",
                        limits.min_dim, limits.max_dim
                    ));
                }
            }
        }
    }
    let out = catch_panic(|| Ok(crate::render_spec(spec_json)));
    // A panic may have left a per-thread brand override behind; reset it.
    crate::set_brand(None);
    let out = out?;
    match out.strip_prefix("<pre>") {
        Some(rest) => Err(format!(
            "anofox_render: {}",
            rest.trim_end_matches("</pre>")
        )),
        None => Ok(out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"{"rows":[{"c0":"a","c1":3},{"c0":"b","c1":5}],"roles":[[0,"XAXIS"],[1,"BARCHART"]],"width":320,"height":200}"#;

    #[test]
    fn renders_svg() {
        let svg = render_spec_checked(OK, &RenderLimits::default()).unwrap();
        assert!(svg.starts_with("<svg"), "{}", &svg[..svg.len().min(60)]);
    }

    #[test]
    fn rejects_bad_input() {
        let l = RenderLimits::default();
        assert!(render_spec_checked("nope", &l).is_err());
        assert!(render_spec_checked("[]", &l).is_err());
        assert!(render_spec_checked(r#"{"rows":[]}"#, &l).is_err());
        assert!(render_spec_checked(r#"{"rows":[],"roles":[[0,"NOT_A_ROLE"]]}"#, &l).is_err());
        assert!(render_spec_checked(r#"{"rows":[],"roles":[[0,null]]}"#, &l).is_err());
        assert!(render_spec_checked(&OK.replace("320", "100000"), &l).is_err());
        assert!(render_spec_checked(&OK.replace("320", "-1"), &l).is_err());
        assert!(render_spec_bytes_checked(b"\xff\xfe", &l).is_err());
    }

    #[test]
    fn row_cap() {
        let l = RenderLimits {
            max_rows: 1,
            ..Default::default()
        };
        let e = render_spec_checked(OK, &l).unwrap_err();
        assert!(e.contains("exceed"), "{e}");
    }

    #[test]
    fn panics_become_errors() {
        let e = catch_panic::<()>(|| panic!("boom")).unwrap_err();
        assert!(e.contains("boom"));
    }
}
