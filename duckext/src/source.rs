//! A live DuckDB 2.0 Quack source for a locked server's snapshot (#352
//! prototype): `options.source` builds (part of) the snapshot by copying from
//! a remote `quack_serve` instance instead of (or alongside) the local
//! session, so a locked dashboard can serve data that lives on another
//! DuckDB 2.0 process.
//!
//! **Quack never enters the serving instance or the user's session.** A
//! dedicated staging instance does the copy (`copy_from_quack`, added next);
//! this module is just its options and the token handling: parsing
//! `options.source`, reading the token fresh per build (never cached, never
//! logged — every error is passed through [`redact`] first), and
//! [`SourceOpts`]'s `Debug`, which never prints the token itself (only the
//! *name* of the env var / file it comes from). See `docs/secure-serving.md`,
//! "Quack source".

use anofox_visualization::host::serving::valid_ident;
use std::path::PathBuf;
use std::time::Duration;

/// Remote views: materialise them as tables in the snapshot (Quack exposes no
/// `CREATE VIEW` DDL to replay, only the query result — fact #2), or leave
/// them out.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Views {
    Materialize,
    Skip,
}

/// `options.source` of a locked server: build (part of) the snapshot from a
/// live Quack server. Its `Debug` never prints the token — only where it
/// comes from (`token_env`'s name, or `token_file`'s path).
#[derive(Clone)]
pub struct SourceOpts {
    /// `quack:host:port` (checked to start with `quack:` at parse time).
    pub uri: String,
    pub token_env: Option<String>,
    pub token_file: Option<PathBuf>,
    /// The database name the snapshot attaches the copy under (`options.source.as`).
    pub r#as: String,
    /// Schemas to copy (empty = every schema the source exposes).
    pub schemas: Vec<String>,
    pub views: Views,
    /// A deadline for the whole build (`timeout_s`).
    pub timeout: Duration,
    /// `tls.fingerprint` — reserved for a non-local source (open question:
    /// not enforced by this prototype; see docs/secure-serving.md).
    pub tls_fingerprint: Option<String>,
}

impl std::fmt::Debug for SourceOpts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceOpts")
            .field("uri", &self.uri)
            .field("token_env", &self.token_env)
            .field("token_file", &self.token_file)
            .field("as", &self.r#as)
            .field("schemas", &self.schemas)
            .field("views", &matches!(self.views, Views::Materialize))
            .field("timeout", &self.timeout)
            .field("tls_fingerprint_set", &self.tls_fingerprint.is_some())
            .finish()
    }
}

/// Parse `options.source` (a JSON object). Never reads the token — that
/// happens fresh per build, in [`read_token`].
pub fn parse(val: &serde_json::Value) -> Result<SourceOpts, String> {
    let m = val
        .as_object()
        .ok_or("options.source must be a JSON object")?;
    let (mut uri, mut token_env, mut token_file, mut as_name) = (None, None, None, None);
    let (mut schemas, mut views, mut tls_fingerprint) = (Vec::new(), Views::Materialize, None);
    let mut timeout = Duration::from_secs(600);
    for (k, v) in m {
        match k.as_str() {
            "uri" => {
                let u = v
                    .as_str()
                    .ok_or("options.source.uri must be a string")?
                    .trim();
                if u.starts_with("quack://") {
                    return Err(
                        "options.source.uri: use 'quack:host:port' (no '//'), not 'quack://…'"
                            .into(),
                    );
                }
                if !u.starts_with("quack:") {
                    return Err("options.source.uri must start with 'quack:'".into());
                }
                uri = Some(u.to_string());
            }
            "token_env" => {
                let e = v
                    .as_str()
                    .filter(|e| !e.trim().is_empty())
                    .ok_or("options.source.token_env must be a non-empty string")?;
                token_env = Some(e.to_string());
            }
            "token_file" => {
                let p = v
                    .as_str()
                    .filter(|p| !p.trim().is_empty())
                    .ok_or("options.source.token_file must be a non-empty string")?;
                token_file = Some(PathBuf::from(p));
            }
            "token" => {
                return Err(
                    "options.source.token is refused — use token_env or token_file so the \
                     credential is never written into this SQL or logged with it"
                        .into(),
                )
            }
            "as" => {
                let a = v
                    .as_str()
                    .filter(|a| valid_ident(a))
                    .ok_or("options.source.as must be a database identifier")?;
                as_name = Some(a.to_string());
            }
            "schemas" => {
                let arr = v
                    .as_array()
                    .ok_or("options.source.schemas must be a list of schema names")?;
                for s in arr {
                    let s = s
                        .as_str()
                        .filter(|s| valid_ident(s))
                        .ok_or("options.source.schemas: invalid schema name")?;
                    schemas.push(s.to_string());
                }
            }
            "views" => {
                views = match v.as_str() {
                    Some("materialize") => Views::Materialize,
                    Some("skip") => Views::Skip,
                    _ => {
                        return Err(
                            "options.source.views must be \"materialize\" or \"skip\"".into()
                        )
                    }
                };
            }
            "timeout_s" => {
                let n = v
                    .as_u64()
                    .filter(|n| *n > 0)
                    .ok_or("options.source.timeout_s must be a positive integer")?;
                timeout = Duration::from_secs(n.min(86_400));
            }
            "tls" => {
                let t = v
                    .as_object()
                    .ok_or("options.source.tls must be an object like {\"fingerprint\": \"…\"}")?;
                let fp = t
                    .get("fingerprint")
                    .and_then(|f| f.as_str())
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                    .ok_or("options.source.tls.fingerprint must be a non-empty string")?;
                tls_fingerprint = Some(fp.to_string());
            }
            other => {
                return Err(format!(
                    "options.source: unknown key '{other}' (uri, token_env, token_file, as, \
                     schemas, views, timeout_s, tls)"
                ))
            }
        }
    }
    let uri = uri.ok_or("options.source.uri is required")?;
    match (&token_env, &token_file) {
        (Some(_), Some(_)) => {
            return Err("options.source: exactly one of token_env / token_file, not both".into())
        }
        (None, None) => {
            return Err(
                "options.source: exactly one of token_env or token_file is required \
                 (token_file is recommended — an env var can be read by any pre-lock SQL \
                 in the host session)"
                    .into(),
            )
        }
        _ => {}
    }
    Ok(SourceOpts {
        uri,
        token_env,
        token_file,
        r#as: as_name.unwrap_or_else(|| "source".to_string()),
        schemas,
        views,
        timeout,
        tls_fingerprint,
    })
}

/// Read the source token fresh (never cached across builds): the env var, or
/// the file (trimmed; refuses an empty file).
pub fn read_token(opts: &SourceOpts) -> Result<String, String> {
    if let Some(name) = &opts.token_env {
        return std::env::var(name).map_err(|_| {
            format!("options.source.token_env: environment variable '{name}' is not set")
        });
    }
    if let Some(path) = &opts.token_file {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            format!(
                "options.source.token_file: cannot read '{}': {e}",
                path.display()
            )
        })?;
        let t = raw.trim();
        if t.is_empty() {
            return Err(format!(
                "options.source.token_file: '{}' is empty",
                path.display()
            ));
        }
        return Ok(t.to_string());
    }
    Err("options.source: no token source configured".into())
}

/// Replace every occurrence of `token` in `err` — the token must never reach
/// stderr, the request log, `/healthz`, a page, a panel or an app-config
/// (`docs/secure-serving.md`, "Quack source").
pub fn redact(err: &str, token: &str) -> String {
    if token.is_empty() {
        err.to_string()
    } else {
        err.replace(token, "<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(json: &str) -> Result<SourceOpts, String> {
        parse(&serde_json::from_str(json).unwrap())
    }

    #[test]
    fn uri_must_carry_the_quack_prefix() {
        let err = src(r#"{"uri":"host:9494","token_env":"T"}"#).unwrap_err();
        assert!(err.contains("must start with 'quack:'"), "{err}");
        let hint = src(r#"{"uri":"quack://host:9494","token_env":"T"}"#).unwrap_err();
        assert!(hint.contains("quack:host:port"), "{hint}");
        assert!(src(r#"{"uri":"quack:host:9494","token_env":"T"}"#).is_ok());
    }

    #[test]
    fn exactly_one_of_token_env_or_token_file() {
        let neither = src(r#"{"uri":"quack:h:1"}"#).unwrap_err();
        assert!(
            neither.contains("exactly one of token_env or token_file"),
            "{neither}"
        );
        let both = src(r#"{"uri":"quack:h:1","token_env":"T","token_file":"/tmp/t"}"#).unwrap_err();
        assert!(both.contains("not both"), "{both}");
        assert!(src(r#"{"uri":"quack:h:1","token_env":"T"}"#).is_ok());
        assert!(src(r#"{"uri":"quack:h:1","token_file":"/tmp/t"}"#).is_ok());
    }

    #[test]
    fn a_literal_token_key_is_refused() {
        let err = src(r#"{"uri":"quack:h:1","token":"secret"}"#).unwrap_err();
        assert!(err.contains("options.source.token is refused"), "{err}");
    }

    #[test]
    fn as_defaults_to_source_and_must_be_an_identifier() {
        let o = src(r#"{"uri":"quack:h:1","token_env":"T"}"#).unwrap();
        assert_eq!(o.r#as, "source");
        let err = src(r#"{"uri":"quack:h:1","token_env":"T","as":"bad; drop"}"#).unwrap_err();
        assert!(err.contains("options.source.as"), "{err}");
        let o = src(r#"{"uri":"quack:h:1","token_env":"T","as":"warehouse"}"#).unwrap();
        assert_eq!(o.r#as, "warehouse");
    }

    #[test]
    fn views_and_unknown_keys() {
        assert!(matches!(
            src(r#"{"uri":"quack:h:1","token_env":"T","views":"skip"}"#)
                .unwrap()
                .views,
            Views::Skip
        ));
        let err = src(r#"{"uri":"quack:h:1","token_env":"T","views":"nope"}"#).unwrap_err();
        assert!(err.contains("materialize"), "{err}");
        let err = src(r#"{"uri":"quack:h:1","token_env":"T","bogus":1}"#).unwrap_err();
        assert!(err.contains("unknown key 'bogus'"), "{err}");
    }

    #[test]
    fn schema_names_are_validated_identifiers() {
        let err =
            src(r#"{"uri":"quack:h:1","token_env":"T","schemas":["ok","bad name"]}"#).unwrap_err();
        assert!(err.contains("invalid schema name"), "{err}");
    }

    #[test]
    fn debug_never_prints_the_token() {
        // The token itself never reaches SourceOpts (read fresh per build by
        // `read_token`) — only where it comes from.
        let o = src(r#"{"uri":"quack:h:1","token_env":"VERY_SECRET_ENV_NAME"}"#).unwrap();
        let d = format!("{o:?}");
        assert!(d.contains("VERY_SECRET_ENV_NAME"), "{d}");
        let o = src(r#"{"uri":"quack:h:1","token_file":"/etc/anofox/src-token"}"#).unwrap();
        let d = format!("{o:?}");
        assert!(d.contains("src-token"), "{d}");
    }

    #[test]
    fn redact_removes_every_occurrence() {
        let err = "ATTACH failed: bad token s3cr3t-xyz near s3cr3t-xyz again";
        let r = redact(err, "s3cr3t-xyz");
        assert!(!r.contains("s3cr3t-xyz"), "{r}");
        assert_eq!(r.matches("<redacted>").count(), 2, "{r}");
        // An empty token (misconfiguration) must not turn the whole message
        // into "<redacted>" (`str::replace("", x)` would insert `x`
        // between every byte) — `redact` special-cases it.
        assert_eq!(redact(err, ""), err);
    }

    #[test]
    fn read_token_rejects_an_empty_file_and_trims_whitespace() {
        let dir = std::env::temp_dir().join(format!(
            "anofox-src-token-test-{}",
            anofox_visualization::host::serving::random_token()
        ));
        std::fs::create_dir(&dir).unwrap();
        let empty = dir.join("empty");
        std::fs::write(&empty, "   \n").unwrap();
        let padded = dir.join("padded");
        std::fs::write(&padded, "  topsecret \n").unwrap();
        let opts_empty = src(&format!(
            r#"{{"uri":"quack:h:1","token_file":{:?}}}"#,
            empty.to_string_lossy()
        ))
        .unwrap();
        let err = read_token(&opts_empty).unwrap_err();
        assert!(err.contains("is empty"), "{err}");
        let opts_padded = src(&format!(
            r#"{{"uri":"quack:h:1","token_file":{:?}}}"#,
            padded.to_string_lossy()
        ))
        .unwrap();
        assert_eq!(read_token(&opts_padded).unwrap(), "topsecret");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
