//! The audit log (`options.audit` / `--audit`): one JSON object per line for
//! every authentication event — a login, a logout, an embed link exchanged,
//! every refused credential — and every admin action (an API key created or
//! revoked, the key file reloaded, the server started with a login). Who,
//! what, when, outcome; never a token, key, cookie, code or secret (an API key
//! is named by its public id, a session by its random id). Same file handling
//! as the request log: append-only, created `0600`, never through a symlink,
//! reopened per line (rename-based rotation needs no signal).

use crate::host::access::{open_log, rfc3339};
use serde_json::{json, Map, Value};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Mutex;

/// One audit event.
#[derive(Debug, Default, Clone)]
pub struct Event<'a> {
    /// `login`, `logout`, `embed`, `auth` (a refused credential),
    /// `api_key_created`, `api_key_revoked`, `api_keys_reloaded`, `start`.
    pub event: &'a str,
    /// `ok` or `deny` (`error` for a failed admin action).
    pub outcome: &'a str,
    /// The login method: `proxy`, `jwt`, `oidc`, `embed`, `api_key`.
    pub method: Option<&'a str>,
    pub user: Option<&'a str>,
    pub reason: Option<&'a str>,
    pub peer: Option<IpAddr>,
    pub path: Option<&'a str>,
    pub dashboard: Option<&'a str>,
    pub key_id: Option<&'a str>,
    pub sid: Option<&'a str>,
    /// Free text from the server itself (never request data).
    pub detail: Option<&'a str>,
}

#[derive(Debug)]
enum Sink {
    Stderr,
    File(PathBuf),
}

#[derive(Debug)]
pub struct AuditLog {
    sink: Sink,
    lock: Mutex<()>,
}

impl AuditLog {
    /// `"stderr"` or a file path (opened once here, so a bad path fails the
    /// start).
    pub fn new(target: &str) -> Result<AuditLog, String> {
        let sink = match target.trim() {
            "" => return Err("audit must be \"stderr\" or a file path".into()),
            "stderr" | "-" => Sink::Stderr,
            p => {
                let p = PathBuf::from(p);
                open_log(&p).map_err(|e| format!("audit: cannot open {}: {e}", p.display()))?;
                Sink::File(p)
            }
        };
        Ok(AuditLog {
            sink,
            lock: Mutex::new(()),
        })
    }

    /// The JSON line of `e` at `now`.
    pub fn line(e: &Event, now: std::time::SystemTime) -> String {
        let mut o = Map::new();
        o.insert("ts".into(), json!(rfc3339(now)));
        o.insert("audit".into(), json!(e.event));
        o.insert("outcome".into(), json!(e.outcome));
        let mut put = |k: &str, v: Option<&str>| {
            if let Some(v) = v {
                // Values that come from a request are bounded.
                let v: String = v.chars().take(256).collect();
                o.insert(k.into(), json!(v));
            }
        };
        put("method", e.method);
        put("user", e.user);
        put("reason", e.reason);
        put("path", e.path);
        put("dashboard", e.dashboard);
        put("key_id", e.key_id);
        put("sid", e.sid);
        put("detail", e.detail);
        if let Some(ip) = e.peer {
            let ip = match ip {
                IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
                v4 => v4,
            };
            o.insert("peer".into(), json!(ip.to_string()));
        }
        Value::Object(o).to_string()
    }

    pub fn write(&self, e: &Event) {
        let line = AuditLog::line(e, std::time::SystemTime::now());
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        match &self.sink {
            Sink::Stderr => eprintln!("{line}"),
            Sink::File(p) => {
                use std::io::Write;
                let r = open_log(p).and_then(|mut f| f.write_all(format!("{line}\n").as_bytes()));
                if let Err(err) = r {
                    eprintln!("[anofox-serve] audit log {}: {err}", p.display());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_lines_are_single_escaped_json() {
        let e = Event {
            event: "auth",
            outcome: "deny",
            method: Some("jwt"),
            user: Some("eve\n{\"audit\":\"login\",\"outcome\":\"ok\"}"),
            reason: Some("expired"),
            peer: Some("::ffff:10.0.0.1".parse().unwrap()),
            ..Default::default()
        };
        let l = AuditLog::line(&e, std::time::UNIX_EPOCH);
        assert!(!l.contains('\n'));
        let v: Value = serde_json::from_str(&l).unwrap();
        assert_eq!(v["audit"], "auth");
        assert_eq!(v["outcome"], "deny");
        assert_eq!(v["reason"], "expired");
        assert_eq!(v["peer"], "10.0.0.1");
        assert!(v.get("key_id").is_none());
    }

    #[test]
    fn audit_file_is_private_and_appended() {
        let dir = std::env::temp_dir().join(format!(
            "anofox-audit-{}",
            crate::host::serving::random_token()
        ));
        std::fs::create_dir(&dir).unwrap();
        let p = dir.join("audit.jsonl");
        let a = AuditLog::new(p.to_str().unwrap()).unwrap();
        a.write(&Event {
            event: "start",
            outcome: "ok",
            ..Default::default()
        });
        a.write(&Event {
            event: "login",
            outcome: "ok",
            user: Some("alice"),
            ..Default::default()
        });
        assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let link = dir.join("link.jsonl");
            std::os::unix::fs::symlink(&p, &link).unwrap();
            assert!(AuditLog::new(link.to_str().unwrap()).is_err());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
