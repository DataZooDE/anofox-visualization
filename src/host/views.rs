//! Saved views on the locked server (`options.views`, #913): a viewer's
//! named page states per dashboard, kept in the server's own append-only
//! JSONL store ([`crate::host::state::Store`], kind `views`) so they follow
//! the viewer across browsers. The browser-only list (#888) stays for
//! servers without this option and for the builder.
//!
//! A view is a **name** and a **query** — the page address' state part
//! (`v.*` inputs and selections, `frame.*`, `view.*`, `tab`), never a whole
//! address:
//! the page rebuilds the address from its own dashboard path, so a stored
//! view can't send anyone elsewhere. Only its owner ever reads it back
//! (`GET /api/views` filters by the signed-in user), and the page puts the
//! name in as text. The query is held to the same shape a share link has;
//! a `v.*` name the dashboard doesn't declare is ignored by the page, like
//! one typed into the address bar.
//!
//! Records reuse the state store's line format: `submit` is `"view"` (save
//! or replace by name) or `"delete"`, `id` the view's id, `vars`
//! `{"name": …, "query": …}` (a delete: `{}`). [`fold`] turns a viewer's
//! lines into the current list.

use crate::host::state::Record;
use serde_json::Value;
use std::path::PathBuf;

/// Parsed, structurally valid `options.views`.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewsOptions {
    pub path: PathBuf,
    /// Most views one viewer keeps per dashboard.
    pub max_per_dashboard: usize,
    pub max_mb: u64,
}

pub const DEFAULT_MAX_PER_DASHBOARD: usize = 50;
pub const MAX_MAX_PER_DASHBOARD: usize = 500;
pub const DEFAULT_MAX_MB: u64 = 64;
/// A view's name, in characters.
pub const NAME_MAX_CHARS: usize = 80;
/// A view's query, in bytes (percent-encoded).
pub const QUERY_MAX_BYTES: usize = 4096;
/// Parameters in one query.
pub const QUERY_MAX_PARAMS: usize = 64;
/// `POST /api/views` body cap.
pub const VIEWS_MAX_BODY: usize = 8 * 1024;
/// Lines one viewer may have in the store (saves and deletes, every
/// dashboard) — the append-only file's bound per user.
pub const MAX_LINES_PER_USER: u64 = 5000;
pub const RATE_PER_MINUTE: u32 = 30;
pub const RATE_BURST: u32 = 10;

const KEYS: &str = "path, max_per_dashboard, max_mb";

impl ViewsOptions {
    pub fn parse(v: &Value) -> Result<ViewsOptions, String> {
        let m = v.as_object().ok_or("options.views must be an object")?;
        let pos = |val: &Value, key: &str| -> Result<u64, String> {
            val.as_u64()
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("options.views.{key} must be a positive integer"))
        };
        let mut o = ViewsOptions {
            path: PathBuf::new(),
            max_per_dashboard: DEFAULT_MAX_PER_DASHBOARD,
            max_mb: DEFAULT_MAX_MB,
        };
        let mut has_path = false;
        for (k, val) in m {
            match k.as_str() {
                "path" => {
                    let p = val
                        .as_str()
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .ok_or("options.views.path must be a file path")?;
                    o.path = PathBuf::from(p);
                    has_path = true;
                }
                "max_per_dashboard" => {
                    o.max_per_dashboard =
                        (pos(val, "max_per_dashboard")? as usize).min(MAX_MAX_PER_DASHBOARD)
                }
                "max_mb" => o.max_mb = pos(val, "max_mb")?.min(4096),
                other => {
                    return Err(format!("options.views: unknown key '{other}' ({KEYS})"));
                }
            }
        }
        if !has_path {
            return Err("options.views needs a path (the file the views are kept in)".into());
        }
        Ok(o)
    }
}

/// A viewer's saved view, as `GET /api/views` returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub id: String,
    pub name: String,
    pub query: String,
    /// When it was saved (RFC 3339).
    pub at: String,
}

impl View {
    pub fn to_json(&self) -> Value {
        serde_json::json!({"id": self.id, "name": self.name, "query": self.query, "at": self.at})
    }
}

/// A view's name: trimmed, 1–80 characters, no control characters.
pub fn check_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("a view needs a name".into());
    }
    if n.chars().count() > NAME_MAX_CHARS {
        return Err(format!(
            "a view's name has at most {NAME_MAX_CHARS} characters"
        ));
    }
    if n.chars().any(char::is_control) {
        return Err("a view's name can't hold control characters".into());
    }
    Ok(n.to_string())
}

/// A parameter name a view may carry: `tab`, or `v.` / `frame.` / `view.`
/// and a name of letters, digits, `_` (at most 64).
fn param_ok(key: &str) -> bool {
    if key == "tab" {
        return true;
    }
    let rest = key
        .strip_prefix("v.")
        .or_else(|| key.strip_prefix("frame."))
        .or_else(|| key.strip_prefix("view."));
    rest.is_some_and(|r| {
        !r.is_empty() && r.len() <= 64 && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    })
}

/// Percent-decode one query component (`+` is a space). `None` when it
/// isn't valid percent-encoding of UTF-8.
fn decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Percent-encode one query component (`URLSearchParams`' set: letters,
/// digits and `*-._` stay, a space is `+`).
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A view's query, checked and re-encoded: at most 4 KB and 64 parameters,
/// each `tab`, `v.<name>`, `frame.<name>` or `view.<name>`, values without control
/// characters. A leading `?` is dropped. The result is what's stored and
/// handed back — the page appends it to its own dashboard path.
pub fn check_query(query: &str) -> Result<String, String> {
    let q = query.strip_prefix('?').unwrap_or(query);
    if q.len() > QUERY_MAX_BYTES {
        return Err(format!("a view's state is at most {QUERY_MAX_BYTES} bytes"));
    }
    let mut out: Vec<String> = Vec::new();
    for part in q.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        let (Some(k), Some(v)) = (decode(k), decode(v)) else {
            return Err("a view's state isn't a valid query string".into());
        };
        if !param_ok(&k) {
            return Err(format!(
                "a view keeps only tab, v.*, frame.* and view.* parameters, not '{}'",
                k.chars().take(40).collect::<String>()
            ));
        }
        if v.chars().any(char::is_control) {
            return Err("a view's state can't hold control characters".into());
        }
        out.push(format!("{}={}", encode(&k), encode(&v)));
        if out.len() > QUERY_MAX_PARAMS {
            return Err(format!(
                "a view keeps at most {QUERY_MAX_PARAMS} parameters"
            ));
        }
    }
    Ok(out.join("&"))
}

/// The current views of one viewer's lines on one dashboard: a save adds
/// a view or replaces the one with the same name; a delete removes the view
/// with its id. Newest first, at most `max`. `rows` are the viewer's own
/// (the caller filters by user), in store order.
pub fn fold<'a>(
    rows: impl IntoIterator<Item = &'a Record>,
    dashboard: &str,
    max: usize,
) -> Vec<View> {
    let mut list: Vec<View> = Vec::new();
    for r in rows {
        if r.dashboard != dashboard {
            continue;
        }
        match r.submit.as_str() {
            "view" => {
                let (Some(name), Some(query)) = (
                    r.vars.get("name").and_then(Value::as_str),
                    r.vars.get("query").and_then(Value::as_str),
                ) else {
                    continue;
                };
                list.retain(|v| v.name != name && v.id != r.id);
                list.push(View {
                    id: r.id.clone(),
                    name: name.to_string(),
                    query: query.to_string(),
                    at: r.ts.clone(),
                });
            }
            "delete" => list.retain(|v| v.id != r.id),
            _ => {}
        }
    }
    list.reverse();
    list.truncate(max);
    list
}

/// A view id from a request: 32 hex characters.
pub fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(seq: u64, dash: &str, kind: &str, id: &str, vars: Value) -> Record {
        Record {
            v: 1,
            seq,
            ts: format!("2026-11-03T10:00:{seq:02}Z"),
            ts_micros: None,
            user: "ada".into(),
            auth: "oidc".into(),
            dashboard: dash.into(),
            submit: kind.into(),
            id: id.into(),
            vars: vars.as_object().unwrap().clone(),
        }
    }

    #[test]
    fn options_parse() {
        let o = ViewsOptions::parse(&json!({"path": "views.jsonl"})).unwrap();
        assert_eq!(o.max_per_dashboard, DEFAULT_MAX_PER_DASHBOARD);
        let o =
            ViewsOptions::parse(&json!({"path": "v.jsonl", "max_per_dashboard": 9999})).unwrap();
        assert_eq!(o.max_per_dashboard, MAX_MAX_PER_DASHBOARD);
        assert!(ViewsOptions::parse(&json!({}))
            .unwrap_err()
            .contains("path"));
        assert!(ViewsOptions::parse(&json!({"path": "x", "share": true}))
            .unwrap_err()
            .contains("unknown key 'share'"));
        assert!(ViewsOptions::parse(&json!({"path": "x", "max_mb": 0})).is_err());
    }

    #[test]
    fn names() {
        assert_eq!(check_name("  East, Q3 ").unwrap(), "East, Q3");
        assert!(check_name("   ").is_err());
        assert!(check_name(&"x".repeat(81)).is_err());
        assert!(check_name(&"ü".repeat(80)).is_ok());
        assert!(check_name("a\u{0}b").is_err());
        assert!(check_name("a\nb").is_err());
    }

    #[test]
    fn queries_keep_only_page_state() {
        assert_eq!(
            check_query("?v.region=east&tab=By+product&frame.week=W3&view.trend=Table").unwrap(),
            "v.region=east&tab=By+product&frame.week=W3&view.trend=Table"
        );
        // re-encoded the same way URLSearchParams does
        assert_eq!(check_query("v.q=a%20b%26c").unwrap(), "v.q=a+b%26c");
        assert_eq!(check_query("v.city=K%C3%B6ln").unwrap(), "v.city=K%C3%B6ln");
        assert_eq!(check_query("").unwrap(), "");
        for bad in [
            "dashboard=other",
            "v.=x",
            "v.a-b=x",
            "next=https://evil.example",
            "v.x=%ZZ",
            "v.x=%C3",
            "v.x=a%0Ab",
            "frame.=1",
            "TAB=x",
        ] {
            assert!(check_query(bad).is_err(), "{bad}");
        }
        assert!(check_query(&format!("v.x={}", "a".repeat(QUERY_MAX_BYTES))).is_err());
        let many: Vec<String> = (0..=QUERY_MAX_PARAMS)
            .map(|i| format!("v.x{i}=1"))
            .collect();
        assert!(check_query(&many.join("&")).is_err());
    }

    #[test]
    fn fold_saves_replaces_and_deletes() {
        let a = "a".repeat(32);
        let b = "b".repeat(32);
        let c = "c".repeat(32);
        let rows = vec![
            rec(
                1,
                "sales",
                "view",
                &a,
                json!({"name": "East", "query": "v.r=east"}),
            ),
            rec(
                2,
                "sales",
                "view",
                &b,
                json!({"name": "West", "query": "v.r=west"}),
            ),
            rec(
                3,
                "other",
                "view",
                &c,
                json!({"name": "East", "query": "v.r=x"}),
            ),
            // same name: replaces "East"
            rec(
                4,
                "sales",
                "view",
                &c,
                json!({"name": "East", "query": "v.r=east&tab=T"}),
            ),
            rec(5, "sales", "delete", &b, json!({})),
        ];
        let v = fold(&rows, "sales", 50);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "East");
        assert_eq!(v[0].query, "v.r=east&tab=T");
        assert_eq!(v[0].id, c);
        assert_eq!(fold(&rows, "other", 50).len(), 1);
        assert!(fold(&rows, "none", 50).is_empty());
    }

    #[test]
    fn fold_is_newest_first_and_capped() {
        let rows: Vec<Record> = (0..5)
            .map(|i| {
                rec(
                    i,
                    "d",
                    "view",
                    &format!("{i:032x}"),
                    json!({"name": format!("v{i}"), "query": ""}),
                )
            })
            .collect();
        let v = fold(&rows, "d", 3);
        let names: Vec<&str> = v.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["v4", "v3", "v2"]);
    }

    #[test]
    fn ids() {
        assert!(valid_id(&"0123456789abcdef".repeat(2)));
        assert!(!valid_id("abc"));
        assert!(!valid_id(&"g".repeat(32)));
    }
}
