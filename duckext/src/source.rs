//! A live DuckDB 2.0 Quack source for a locked server's snapshot (#352
//! prototype): `options.source` builds (part of) the snapshot by copying from
//! a remote `quack_serve` instance instead of (or alongside) the local
//! session, so a locked dashboard can serve data that lives on another
//! DuckDB 2.0 process.
//!
//! **Quack never enters the serving instance or the user's session.** Every
//! build opens its own, throwaway staging instance (`Database::open`,
//! extension autoload/autoinstall off): `LOAD quack`, a `CREATE TEMPORARY
//! SECRET` (never `ATTACH … (TOKEN …)`, which leaks the token into
//! `duckdb_databases().options`), `ATTACH '<uri>' AS …`, then the snapshot
//! file itself `(READ_WRITE)`. Both are `DETACH`ed and the secret is dropped
//! (best effort) before the staging instance closes; the snapshot file then
//! goes through the existing `READ_ONLY` + `lock_configuration` path exactly
//! like a local database (`build_snapshot` in `serve.rs`). The locked,
//! serving instance itself never loads `quack` — see
//! `docs/secure-serving.md`, "Quack source".
//!
//! The token is read fresh for every build (`read_token`), never cached and
//! never logged: every error on the way out is passed through [`redact`]
//! first, and [`SourceOpts`]'s `Debug` never prints it (only the *name* of
//! the env var / file it comes from).
//!
//! `copy_from_quack` below has no timeout or cancellation yet (#352 step 3);
//! those land in the next step, alongside the watchdog.

use crate::db::{Conn, Database};
use crate::serve::{columns_by_table, fk_order, insertable_columns};
use anofox_visualization::host::serving::{quote_ident, sql_string_literal, valid_ident};
use std::path::{Path, PathBuf};
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

/// Build the snapshot file for a Quack source: a dedicated staging instance
/// that `LOAD`s `quack`, attaches the source under a temporary secret,
/// attaches the new snapshot file, copies schema + data (+ materialised
/// views), then tears both attachments and the secret down.
///
/// `dir`: the snapshot's own private directory (the file is created inside
/// it). No timeout or cancellation yet (#352 step 3) — a slow or wedged
/// remote query blocks the build; the watchdog lands in the next step.
///
/// Returns the database name to attach the file under (`opts.as`), the file
/// path, and non-fatal warnings (lost FKs/macros/sequences, skipped views,
/// schemas the source exposes but `options.source.schemas` didn't ask for).
pub fn copy_from_quack(
    dir: &Path,
    opts: &SourceOpts,
) -> Result<(String, PathBuf, Vec<String>), String> {
    let token = read_token(opts)?;
    build(dir, opts, &token).map_err(|e| redact(&e, &token))
}

fn build(
    dir: &Path,
    opts: &SourceOpts,
    token: &str,
) -> Result<(String, PathBuf, Vec<String>), String> {
    let mut warnings = Vec::new();
    // 1. A throwaway staging instance: nothing auto-loads or auto-installs,
    // and only `quack` (loaded explicitly, next) may load at all.
    let db = Database::open_quack_staging()?;
    let stg = db.connect()?;
    stg.exec("LOAD quack").map_err(|e| {
        format!(
            "could not LOAD quack in the staging instance — install it first (INSTALL quack \
             from the repository; it is never autoloaded or autoinstalled here): {}",
            e.lines().next().unwrap_or("")
        )
    })?;

    // 2. A named TEMPORARY SECRET (never `ATTACH … (TOKEN …)`, which leaks
    // the token into duckdb_databases().options) + the remote attachment.
    let alias = format!("__anofox_src_{}", sv_random());
    stg.exec(&format!(
        "CREATE TEMPORARY SECRET {} (TYPE quack, TOKEN {})",
        quote_ident(&alias),
        sql_string_literal(token)
    ))?;
    stg.exec(&format!(
        "ATTACH {} AS {}",
        sql_string_literal(&opts.uri),
        quote_ident(&alias)
    ))?;

    // 3. The snapshot file, attached READ_WRITE (dropped/detached below
    // regardless of outcome; the caller only sees it on success).
    let file = dir.join(format!("db_quack_{}.duckdb", sv_random()));
    let f = file.to_string_lossy().into_owned();
    let dst_alias = "__anofox_quack_dst";
    let result = stg
        .exec(&format!(
            "ATTACH {} AS {} (READ_WRITE)",
            sql_string_literal(&f),
            dst_alias
        ))
        .and_then(|_| copy_schema_and_data(&stg, &alias, dst_alias, opts, &mut warnings));

    // 4. Best-effort teardown either way: DETACH both, DROP the secret.
    let _ = stg.exec(&format!("DETACH {dst_alias}"));
    let _ = stg.exec(&format!("DETACH {}", quote_ident(&alias)));
    let _ = stg.exec(&format!("DROP SECRET {}", quote_ident(&alias)));

    result?;
    warnings.push(format!(
        "quack source '{}': Quack exposes no foreign keys, indexes, macros or sequences — none are copied",
        opts.r#as
    ));
    Ok((opts.r#as.clone(), file, warnings))
}

/// `(SCHEMA)` copy (a bare `COPY FROM DATABASE` fails on a Quack source:
/// "Multiple streaming scans … not implemented", #352 fact 1), then one
/// `INSERT` per table (reusing the local-database helpers: Quack has no
/// foreign keys, so `fk_order` is just the given order). Views are
/// materialised or skipped per `opts.views` (fact 1: `(SCHEMA)` doesn't copy
/// them). `opts.schemas` restricts what ends up in `dst`: any other schema
/// the source exposes is dropped after the `(SCHEMA)` copy.
fn copy_schema_and_data(
    stg: &Conn,
    src: &str,
    dst: &str,
    opts: &SourceOpts,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    stg.exec("BEGIN")?;
    let r = (|| -> Result<(), String> {
        stg.exec(&format!(
            "COPY FROM DATABASE {} TO {} (SCHEMA)",
            quote_ident(src),
            dst
        ))?;
        if !opts.schemas.is_empty() {
            drop_unwanted_schemas(stg, dst, &opts.schemas, warnings)?;
        }
        let tables = list(stg, src, "duckdb_tables()", "table_name", &opts.schemas)?;
        let columns = columns_by_table(stg, src)?;
        let edges: Vec<(String, String, String)> = Vec::new(); // Quack exposes no FKs.
        let empty = Vec::new();
        for i in fk_order(&tables, &edges) {
            let (schema, table) = &tables[i];
            let qual = |db: &str| format!("{}.{}.{}", db, quote_ident(schema), quote_ident(table));
            let (from, to) = (qual(src), qual(dst));
            let cols = insertable_columns(
                stg,
                &to,
                columns
                    .get(&(schema.clone(), table.clone()))
                    .unwrap_or(&empty),
            )?
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", ");
            stg.exec(&format!(
                "INSERT INTO {to} ({cols}) SELECT {cols} FROM {from}"
            ))
            .map_err(|e| format!("{schema}.{table}: {}", e.lines().next().unwrap_or("")))?;
        }
        let views = list(stg, src, "duckdb_views()", "view_name", &opts.schemas)?;
        for (schema, view) in views {
            match opts.views {
                Views::Skip => warnings.push(format!(
                    "quack source '{}': view {schema}.{view} skipped (options.source.views = \"skip\")",
                    opts.r#as
                )),
                Views::Materialize => {
                    let from = format!(
                        "{}.{}.{}",
                        quote_ident(src),
                        quote_ident(&schema),
                        quote_ident(&view)
                    );
                    let to = format!("{}.{}.{}", dst, quote_ident(&schema), quote_ident(&view));
                    stg.exec(&format!("CREATE TABLE {to} AS SELECT * FROM {from}"))
                        .map_err(|e| {
                            format!("view {schema}.{view}: {}", e.lines().next().unwrap_or(""))
                        })?;
                    warnings.push(format!(
                        "quack source '{}': view {schema}.{view} materialised as a table (its definition is not replayed)",
                        opts.r#as
                    ));
                }
            }
        }
        Ok(())
    })();
    match r {
        Ok(()) => stg.exec("COMMIT"),
        Err(e) => {
            let _ = stg.exec("ROLLBACK");
            Err(e)
        }
    }
}

/// `(schema, name)` pairs of `src`'s tables or views, restricted to
/// `schemas` when non-empty.
fn list(
    stg: &Conn,
    src: &str,
    from: &str,
    name_col: &str,
    schemas: &[String],
) -> Result<Vec<(String, String)>, String> {
    let lit = sql_string_literal(src);
    let rows = stg
        .query(&format!(
            "SELECT schema_name::VARCHAR, {name_col}::VARCHAR FROM {from} \
             WHERE database_name = {lit} AND NOT internal ORDER BY 1, 2"
        ))?
        .string_rows(100_000)?;
    Ok(rows
        .into_iter()
        .filter_map(|r| Some((r[0].clone()?, r[1].clone()?)))
        .filter(|(s, _)| schemas.is_empty() || schemas.iter().any(|w| w.eq_ignore_ascii_case(s)))
        .collect())
}

/// Drop every schema of `dst` that isn't in `wanted` (the source may expose
/// more than `options.source.schemas` asked for; `(SCHEMA)` already copied
/// all of them, so anything not wanted is removed again before any data
/// moves — never served, never a path around a policy on the wanted ones).
fn drop_unwanted_schemas(
    stg: &Conn,
    dst: &str,
    wanted: &[String],
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let rows = stg
        .query(&format!(
            "SELECT schema_name::VARCHAR FROM duckdb_schemas() \
             WHERE database_name = {} AND NOT internal AND schema_name <> 'main'",
            sql_string_literal(dst)
        ))?
        .string_rows(10_000)?;
    for r in rows {
        let Some(name) = r.first().cloned().flatten() else {
            continue;
        };
        if wanted.iter().any(|w| w.eq_ignore_ascii_case(&name)) {
            continue;
        }
        stg.exec(&format!(
            "DROP SCHEMA {}.{} CASCADE",
            dst,
            quote_ident(&name)
        ))?;
        warnings.push(format!(
            "quack source: schema '{name}' is not in options.source.schemas — dropped from the snapshot, never served"
        ));
    }
    // "main" itself, if not wanted: drop its tables individually (DuckDB
    // keeps one "main" schema per attached database; it cannot be dropped).
    if !wanted.iter().any(|w| w.eq_ignore_ascii_case("main")) {
        let tables = stg
            .query(&format!(
                "SELECT table_name::VARCHAR FROM duckdb_tables() WHERE database_name = {} AND schema_name = 'main'",
                sql_string_literal(dst)
            ))?
            .string_rows(10_000)?;
        let mut any = false;
        for r in tables {
            if let Some(t) = r.first().cloned().flatten() {
                stg.exec(&format!("DROP TABLE {}.main.{}", dst, quote_ident(&t)))?;
                any = true;
            }
        }
        if any {
            warnings.push(
                "quack source: schema 'main' is not in options.source.schemas — its tables were dropped from the snapshot"
                    .to_string(),
            );
        }
    }
    Ok(())
}

/// A short random suffix for staging names (aliases, file names) — delegates
/// to the serving module's CSPRNG-backed token so this module doesn't need
/// its own randomness source.
fn sv_random() -> String {
    anofox_visualization::host::serving::random_token()
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
