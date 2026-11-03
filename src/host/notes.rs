//! Viewers' notes on charts (`options.notes`, #889): a note pinned to a
//! chart at an x value or over an x range ("promotion", "data error"),
//! shown to every viewer of the dashboard as a rule or a band with its text,
//! author and day ([`crate::ViewerNote`]). Writers add them; the author, or
//! a moderator, deletes one. Kept in the server's own append-only JSONL file
//! ([`crate::host::state::Store`], kind `notes`).
//!
//! Records reuse the state store's line format: `submit` is `"note"` or
//! `"delete"`, `id` the note's id, `vars` `{"panel": n, "x": …, "x2": …,
//! "text": …}` (a delete: `{}`). [`fold`] turns a dashboard's lines into
//! the current notes.
//!
//! A note is text a viewer wrote, shown to others: the renderer escapes it
//! (an SVG `<title>`), the page sets it as text; control characters are
//! refused. Its anchor is a value the writer types or picks — never read
//! from the data — so a note reveals only what its writer chose to write.

use crate::host::state::Record;
use serde_json::Value;
use std::path::PathBuf;

/// Parsed, structurally valid `options.notes`.
#[derive(Debug, Clone, PartialEq)]
pub struct NotesOptions {
    pub path: PathBuf,
    /// Dashboards that show and take notes.
    pub dashboards: Vec<String>,
    /// Who may add notes; both empty: every personal sign-in.
    pub writer_groups: Vec<String>,
    pub writer_users: Vec<String>,
    /// Who may delete anyone's note (an author may always delete theirs).
    pub moderator_groups: Vec<String>,
    pub moderator_users: Vec<String>,
    pub max_per_dashboard: usize,
    pub text_max_chars: usize,
    pub max_mb: u64,
}

pub const DEFAULT_MAX_PER_DASHBOARD: usize = 200;
pub const MAX_MAX_PER_DASHBOARD: usize = 2000;
pub const DEFAULT_TEXT_MAX_CHARS: usize = 280;
pub const MAX_TEXT_MAX_CHARS: usize = 1000;
pub const DEFAULT_MAX_MB: u64 = 64;
/// An anchor value (`x`, `x2`), in characters.
pub const X_MAX_CHARS: usize = 64;
/// `POST /api/notes` body cap.
pub const NOTES_MAX_BODY: usize = 8 * 1024;
/// Lines one user may have in the file.
pub const MAX_LINES_PER_USER: u64 = 5000;
pub const RATE_PER_MINUTE: u32 = 20;
pub const RATE_BURST: u32 = 5;

const KEYS: &str =
    "path, dashboards, writers, moderators, max_per_dashboard, text_max_chars, max_mb";

/// `{"groups": [...], "users": [...]}`.
fn who(v: &Value, key: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let m = v.as_object().ok_or_else(|| {
        format!("options.notes.{key} must be an object like {{\"groups\": […], \"users\": […]}}")
    })?;
    if let Some(other) = m.keys().find(|k| !matches!(k.as_str(), "groups" | "users")) {
        return Err(format!(
            "options.notes.{key}: unknown key '{other}' (groups, users)"
        ));
    }
    let list = |k: &str| -> Result<Vec<String>, String> {
        match m.get(k) {
            None => Ok(Vec::new()),
            Some(a) => a
                .as_array()
                .ok_or_else(|| format!("options.notes.{key}.{k} must be an array of strings"))?
                .iter()
                .map(|x| {
                    x.as_str().map(str::to_string).ok_or_else(|| {
                        format!("options.notes.{key}.{k}: every entry must be a string")
                    })
                })
                .collect(),
        }
    };
    Ok((list("groups")?, list("users")?))
}

impl NotesOptions {
    pub fn parse(v: &Value) -> Result<NotesOptions, String> {
        let m = v.as_object().ok_or("options.notes must be an object")?;
        let pos = |val: &Value, key: &str| -> Result<u64, String> {
            val.as_u64()
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("options.notes.{key} must be a positive integer"))
        };
        let mut o = NotesOptions {
            path: PathBuf::new(),
            dashboards: Vec::new(),
            writer_groups: Vec::new(),
            writer_users: Vec::new(),
            moderator_groups: Vec::new(),
            moderator_users: Vec::new(),
            max_per_dashboard: DEFAULT_MAX_PER_DASHBOARD,
            text_max_chars: DEFAULT_TEXT_MAX_CHARS,
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
                        .ok_or("options.notes.path must be a file path")?;
                    o.path = PathBuf::from(p);
                    has_path = true;
                }
                "dashboards" => {
                    o.dashboards = val
                        .as_array()
                        .ok_or("options.notes.dashboards must be an array of dashboard ids")?
                        .iter()
                        .map(|x| {
                            x.as_str().map(str::to_string).ok_or_else(|| {
                                "options.notes.dashboards: every entry must be a string".to_string()
                            })
                        })
                        .collect::<Result<_, _>>()?
                }
                "writers" => (o.writer_groups, o.writer_users) = who(val, "writers")?,
                "moderators" => (o.moderator_groups, o.moderator_users) = who(val, "moderators")?,
                "max_per_dashboard" => {
                    o.max_per_dashboard =
                        (pos(val, "max_per_dashboard")? as usize).min(MAX_MAX_PER_DASHBOARD)
                }
                "text_max_chars" => {
                    o.text_max_chars =
                        (pos(val, "text_max_chars")? as usize).min(MAX_TEXT_MAX_CHARS)
                }
                "max_mb" => o.max_mb = pos(val, "max_mb")?.min(4096),
                other => {
                    return Err(format!("options.notes: unknown key '{other}' ({KEYS})"));
                }
            }
        }
        if !has_path {
            return Err("options.notes needs a path (the file the notes are kept in)".into());
        }
        if o.dashboards.is_empty() {
            return Err("options.notes needs dashboards (the ones that show notes)".into());
        }
        Ok(o)
    }

    pub fn enabled_for(&self, dashboard: &str) -> bool {
        self.dashboards.iter().any(|d| d == dashboard)
    }

    /// May `user` (in `groups`) add notes? Everyone when no writers are named.
    pub fn is_writer(&self, user: &str, groups: &[String]) -> bool {
        (self.writer_groups.is_empty() && self.writer_users.is_empty())
            || self.writer_users.iter().any(|u| u == user)
            || self.writer_groups.iter().any(|g| groups.contains(g))
    }

    /// May `user` delete anyone's note?
    pub fn is_moderator(&self, user: &str, groups: &[String]) -> bool {
        self.moderator_users.iter().any(|u| u == user)
            || self.moderator_groups.iter().any(|g| groups.contains(g))
    }
}

/// A note as `GET /api/notes` returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: String,
    pub panel: u64,
    pub x: String,
    pub x2: Option<String>,
    pub text: String,
    pub by: String,
    /// When it was written (RFC 3339).
    pub at: String,
}

impl Note {
    pub fn to_json(&self, mine: bool) -> Value {
        serde_json::json!({
            "id": self.id, "panel": self.panel, "x": self.x, "x2": self.x2,
            "text": self.text, "by": self.by, "at": self.at, "mine": mine,
        })
    }
}

/// A note's text: trimmed, 1–`max` characters, no control characters but
/// line breaks (shown as spaces).
pub fn check_text(text: &str, max: usize) -> Result<String, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("a note needs a text".into());
    }
    if t.chars().count() > max {
        return Err(format!("a note has at most {max} characters"));
    }
    if t.chars().any(|c| c.is_control() && c != '\n') {
        return Err("a note can't hold control characters".into());
    }
    Ok(t.replace('\n', " "))
}

/// An anchor value (`x`, `x2`): 1–64 characters, no control characters.
pub fn check_x(x: &str, what: &str) -> Result<String, String> {
    let t = x.trim();
    if t.is_empty() || t.chars().count() > X_MAX_CHARS || t.chars().any(char::is_control) {
        return Err(format!(
            "'{what}' must be the chart's x value (1–{X_MAX_CHARS} characters)"
        ));
    }
    Ok(t.to_string())
}

/// The current notes of one dashboard from the file's lines, in the order
/// they were written; deletes drop theirs.
pub fn fold<'a>(rows: impl IntoIterator<Item = &'a Record>, dashboard: &str) -> Vec<Note> {
    let mut list: Vec<Note> = Vec::new();
    for r in rows {
        if r.dashboard != dashboard {
            continue;
        }
        match r.submit.as_str() {
            "note" => {
                let (Some(panel), Some(x), Some(text)) = (
                    r.vars.get("panel").and_then(Value::as_u64),
                    r.vars.get("x").and_then(Value::as_str),
                    r.vars.get("text").and_then(Value::as_str),
                ) else {
                    continue;
                };
                list.retain(|n| n.id != r.id);
                list.push(Note {
                    id: r.id.clone(),
                    panel,
                    x: x.to_string(),
                    x2: r.vars.get("x2").and_then(Value::as_str).map(str::to_string),
                    text: text.to_string(),
                    by: r.user.clone(),
                    at: r.ts.clone(),
                });
            }
            "delete" => list.retain(|n| n.id != r.id),
            _ => {}
        }
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(seq: u64, user: &str, dash: &str, kind: &str, id: &str, vars: Value) -> Record {
        Record {
            v: 1,
            seq,
            ts: format!("2026-11-03T10:00:{seq:02}Z"),
            ts_micros: None,
            user: user.into(),
            auth: "oidc".into(),
            dashboard: dash.into(),
            submit: kind.into(),
            id: id.into(),
            vars: vars.as_object().unwrap().clone(),
        }
    }

    #[test]
    fn options_parse() {
        let o = NotesOptions::parse(&json!({"path": "n.jsonl", "dashboards": ["sales"]})).unwrap();
        assert!(o.enabled_for("sales") && !o.enabled_for("other"));
        assert!(
            o.is_writer("anyone", &[]),
            "no writers named: every sign-in"
        );
        assert!(!o.is_moderator("anyone", &[]));
        let o = NotesOptions::parse(&json!({
            "path": "n.jsonl", "dashboards": ["sales"],
            "writers": {"groups": ["analysts"]}, "moderators": {"users": ["lead"]},
            "text_max_chars": 99999
        }))
        .unwrap();
        assert!(o.is_writer("x", &["analysts".into()]) && !o.is_writer("x", &[]));
        assert!(o.is_moderator("lead", &[]));
        assert_eq!(o.text_max_chars, MAX_TEXT_MAX_CHARS);
        for bad in [
            json!({"dashboards": ["s"]}),
            json!({"path": "x"}),
            json!({"path": "x", "dashboards": ["s"], "readers": []}),
            json!({"path": "x", "dashboards": ["s"], "writers": {"roles": []}}),
            json!({"path": "x", "dashboards": [1]}),
        ] {
            assert!(NotesOptions::parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn texts_and_anchors() {
        assert_eq!(
            check_text("  Promotion\nweek ", 280).unwrap(),
            "Promotion week"
        );
        assert!(check_text(" ", 280).is_err());
        assert!(check_text(&"x".repeat(281), 280).is_err());
        assert!(check_text("a\u{7}b", 280).is_err());
        assert_eq!(check_x(" 2026-02-03 ", "x").unwrap(), "2026-02-03");
        assert!(check_x("", "x").is_err());
        assert!(check_x(&"1".repeat(65), "x").is_err());
        assert!(check_x("a\tb", "x").is_err());
    }

    #[test]
    fn fold_keeps_order_and_drops_deletes() {
        let (a, b, c) = ("a".repeat(32), "b".repeat(32), "c".repeat(32));
        let rows = vec![
            rec(
                1,
                "ada",
                "sales",
                "note",
                &a,
                json!({"panel": 2, "x": "2026-02-03", "text": "Promo"}),
            ),
            rec(
                2,
                "bob",
                "sales",
                "note",
                &b,
                json!({"panel": 2, "x": "2026-02-01", "x2": "2026-02-02", "text": "Outage"}),
            ),
            rec(
                3,
                "ada",
                "other",
                "note",
                &c,
                json!({"panel": 1, "x": "1", "text": "x"}),
            ),
            rec(4, "ada", "sales", "delete", &a, json!({})),
            rec(
                5,
                "ada",
                "sales",
                "note",
                &c,
                json!({"panel": "bad", "x": "1", "text": "x"}),
            ),
        ];
        let n = fold(&rows, "sales");
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].id, b);
        assert_eq!(n[0].by, "bob");
        assert_eq!(n[0].x2.as_deref(), Some("2026-02-02"));
        assert_eq!(fold(&rows, "other").len(), 1);
    }
}
