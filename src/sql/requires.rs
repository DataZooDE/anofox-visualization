//! `-- @requires` lines: the optional inputs of one statement.
//!
//! ```sql
//! -- @requires ${rd}/_anofox_meta_forecasts.parquet | ${rd}/forecasts.parquet
//! -- @requires ${rd}/weights.parquet, ${rd}/metrics.parquet
//! ```
//!
//! Each line names one or more **requirements** (separated by commas or
//! whitespace — all must be met); a requirement lists **alternatives**
//! separated by `|` (met when any of them exists). A path may contain `${name}`:
//! the value of the DuckDB variable `name` (`getvariable('name')`) where the
//! statement runs. After expansion a relative path resolves against the data
//! root (`getvariable('anofox_data')`) when that is set, else where DuckDB
//! resolves the statement's own relative paths (the working directory).
//!
//! On a locked server ([`confine`]) an expanded path must stay inside the
//! data folder.

/// The data root variable (set by the locked server, `dashboard --check
/// --data`): a relative path resolves against it.
pub const DATA_VAR: &str = "anofox_data";

/// One requirement: the alternative paths, any of which satisfies it.
pub type Requirement = Vec<String>;

/// The requirements named by the `-- @requires` lines of one raw statement
/// (quotes around a path are optional).
pub fn parse(raw: &str) -> Vec<Requirement> {
    lines(raw).flat_map(|l| parse_line(l).0).collect()
}

/// What is wrong with the `-- @requires` lines of one raw statement, as
/// `(lint code, message)`: `requires/empty-alternative` (`a |`, `| b`,
/// `a || b`, or a line naming no path) and `requires/bad-variable` (a
/// malformed `${…}`).
pub fn issues(raw: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for l in lines(raw) {
        let (reqs, errs) = parse_line(l);
        let empty = errs.is_empty() && reqs.is_empty();
        out.extend(errs.into_iter().map(|e| (EMPTY_ALTERNATIVE, e)));
        if empty {
            out.push((
                EMPTY_ALTERNATIVE,
                "`-- @requires` names no path".to_string(),
            ));
        }
        for p in reqs.iter().flatten() {
            if let Err(e) = vars(p) {
                out.push(("requires/bad-variable", e));
            }
        }
    }
    out
}

const EMPTY_ALTERNATIVE: &str = "requires/empty-alternative";

/// The text after `-- @requires` of each such comment line.
fn lines(raw: &str) -> impl Iterator<Item = &str> {
    raw.lines()
        .filter_map(|l| l.trim().strip_prefix("--"))
        .filter_map(|l| l.trim().strip_prefix("@requires"))
        .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// One line's requirements, and its empty alternatives. `|` binds tighter
/// than the separators: `a | b c, d` = `[a, b]`, `[c]`, `[d]`.
fn parse_line(rest: &str) -> (Vec<Requirement>, Vec<String>) {
    let mut reqs: Vec<Requirement> = Vec::new();
    let mut errs = Vec::new();
    for segment in rest.split(',') {
        let spaced = segment.replace('|', " | ");
        let mut or_next = false; // the last token was a `|`
        let mut fresh = true; // no path yet in this segment
        for tok in spaced.split_whitespace() {
            if tok == "|" {
                if or_next || fresh {
                    errs.push(empty_alternative(segment));
                }
                or_next = true;
                continue;
            }
            let p = tok.trim_matches(|c| c == '\'' || c == '"');
            if p.is_empty() {
                continue;
            }
            match reqs.last_mut() {
                Some(r) if or_next => r.push(p.to_string()),
                _ => reqs.push(vec![p.to_string()]),
            }
            or_next = false;
            fresh = false;
        }
        if or_next {
            errs.push(empty_alternative(segment));
        }
    }
    (reqs, errs)
}

fn empty_alternative(segment: &str) -> String {
    format!(
        "`-- @requires {}` has an empty alternative: every `|` needs a path on both sides",
        segment.trim()
    )
}

/// Is `s` a DuckDB variable name this syntax accepts (`[A-Za-z_][A-Za-z0-9_]*`)?
fn is_var_name(s: &str) -> bool {
    let mut b = s.bytes();
    b.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && b.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// The variables a path names (`${name}`, in order, repeats kept), or why
/// its `${…}` is malformed.
pub fn vars(path: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut rest = path;
    while let Some(i) = rest.find("${") {
        let after = &rest[i + 2..];
        let Some(end) = after.find('}') else {
            return Err(format!("`{path}`: `${{` without a closing `}}`"));
        };
        let name = &after[..end];
        if !is_var_name(name) {
            return Err(format!(
                "`{path}`: `${{{name}}}` is not a variable name ([A-Za-z_][A-Za-z0-9_]*)"
            ));
        }
        out.push(name.to_string());
        rest = &after[end + 1..];
    }
    Ok(out)
}

/// Why a path could not be expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unexpanded {
    /// `${name}` names a variable that has no value here.
    Unknown(String),
    /// The `${…}` syntax is malformed (see [`vars`]).
    Malformed(String),
}

/// `path` with each `${name}` replaced by `lookup(name)`.
pub fn expand(
    path: &str,
    mut lookup: impl FnMut(&str) -> Option<String>,
) -> Result<String, Unexpanded> {
    vars(path).map_err(Unexpanded::Malformed)?;
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let end = after.find('}').unwrap_or(after.len()); // (checked by vars)
        let name = &after[..end];
        out.push_str(&lookup(name).ok_or_else(|| Unexpanded::Unknown(name.to_string()))?);
        rest = &after[(end + 1).min(after.len())..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Is `path` absolute: `/…`, a URL (`s3://…`, `https://…`) or a Windows
/// drive / UNC path?
fn is_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with('/')
        || path.starts_with('\\')
        || path.contains("://")
        || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// An expanded path resolved against the data root: a relative path is
/// joined to `root` when there is one; anything else is returned as is.
pub fn resolve(path: &str, root: Option<&str>) -> String {
    match root.filter(|r| !r.is_empty()) {
        Some(r) if !is_absolute(path) => {
            let p = path.strip_prefix("./").unwrap_or(path);
            format!("{}/{p}", r.trim_end_matches('/'))
        }
        _ => path.to_string(),
    }
}

/// The locked server's rule for an expanded `@requires` path: resolved
/// against the data folder `root` (canonical, no trailing `/`), it must stay
/// inside it. Refused: a `..` segment, a `~` (DuckDB's home directory), a
/// backslash, a NUL or control character, a URL, and any absolute path
/// outside `root`. Glob patterns (`*`, `?`, `[…]`, `**`) are fine: they
/// match inside the folder only. Returns the resolved path.
pub fn confine(path: &str, root: &str) -> Result<String, String> {
    let root = root.trim_end_matches('/');
    if root.is_empty() {
        return Err("no data folder".into());
    }
    if path.is_empty() {
        return Err("empty path".into());
    }
    if path.chars().any(|c| c.is_control() || c == '\\') {
        return Err("control character or backslash".into());
    }
    if path.starts_with('~') || path.contains("://") {
        return Err("a home-directory path or a URL".into());
    }
    let full = resolve(path, Some(root));
    let Some(inner) = full.strip_prefix(root).and_then(|r| r.strip_prefix('/')) else {
        return Err("outside the data folder".into());
    };
    if inner.is_empty()
        || inner
            .split('/')
            .any(|seg| seg == ".." || seg.starts_with('~'))
    {
        return Err("a `..` or `~` segment".into());
    }
    Ok(full)
}

/// The query that counts the files matching `path` (`glob`, so patterns
/// work); its single row has the count, as text, in `n`.
pub fn glob_sql(path: &str) -> String {
    format!(
        "SELECT CAST(count(*) AS VARCHAR) AS n FROM glob('{}')",
        path.replace('\'', "''")
    )
}

/// The glob pattern characters (DuckDB's `*`, `?`, `[…]`, plus `{` for
/// safety).
pub const GLOB_CHARS: &[char] = &['*', '?', '[', ']', '{', '}'];

/// Does `s` contain a glob pattern character? On a locked server a pattern may
/// come only from the author's literal `@requires` text: a variable's value
/// with one is refused, so no viewer-influenced value is read as a pattern.
pub fn has_glob(s: &str) -> bool {
    s.contains(GLOB_CHARS)
}

/// At most this many matches of a pattern are looked at (canonicalised one by
/// one until one lies inside the data folder, see [`files_sql`]). A pattern
/// whose first this-many matches all lead out of the folder (symlinks) counts
/// as missing: it fails closed.
pub const MAX_MATCHES: usize = 1024;

/// The query that lists the files matching `path`, as text in column `f`:
/// one row is enough for a plain path, up to [`MAX_MATCHES`] for a pattern
/// (a match whose real path lies outside the data folder doesn't count).
pub fn files_sql(path: &str) -> String {
    let limit = if has_glob(path) { MAX_MATCHES } else { 1 };
    format!(
        "SELECT CAST(file AS VARCHAR) AS f FROM glob('{}') LIMIT {limit}",
        path.replace('\'', "''")
    )
}

/// Does a file `glob` listed lie inside the data folder once symlinks are
/// resolved? (`glob` lists through a link that points out of the folder;
/// DuckDB refuses only the read.) `root` must be canonical.
pub fn inside_canonical(file: &str, root: &std::path::Path) -> bool {
    std::fs::canonicalize(file).is_ok_and(|real| real.starts_with(root))
}

/// A short-lived memory of `@requires` answers (also "missing"), keyed by the
/// expanded path: a recursive pattern is not rescanned by every request.
/// Bounded: when full, expired entries go, then everything.
pub struct ExistsCache {
    ttl: std::time::Duration,
    cap: usize,
    map: std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, bool)>>,
}

impl ExistsCache {
    pub fn new(ttl: std::time::Duration, cap: usize) -> ExistsCache {
        ExistsCache {
            ttl,
            cap: cap.max(1),
            map: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// The cached answer for `path`, else `probe()`. Only a definite answer
    /// (`Some`) is remembered: a probe that failed or was interrupted (`None`,
    /// e.g. a viewer superseding their own request) counts as missing for
    /// this call only, never for other viewers.
    pub fn exists(&self, path: &str, probe: impl FnOnce() -> Option<bool>) -> bool {
        let now = std::time::Instant::now();
        {
            let map = self.map.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(&(at, yes)) = map.get(path) {
                if now.duration_since(at) < self.ttl {
                    return yes;
                }
            }
        }
        let Some(yes) = probe() else {
            return false;
        };
        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        if map.len() >= self.cap && !map.contains_key(path) {
            let ttl = self.ttl;
            map.retain(|_, (at, _)| now.duration_since(*at) < ttl);
            if map.len() >= self.cap {
                map.clear();
            }
        }
        map.insert(path.to_string(), (now, yes));
        yes
    }
}

/// The query that reads the variables `names` as text: one row, column `v{i}`
/// for `names[i]` (NULL when unset). Names must pass [`vars`] (identifiers).
pub fn vars_sql(names: &[String]) -> String {
    let cols: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(i, n)| format!("CAST(getvariable('{n}') AS VARCHAR) AS v{i}"))
        .collect();
    format!("SELECT {}", cols.join(", "))
}

/// Every distinct variable the requirements name (malformed paths skipped).
pub fn all_vars(reqs: &[Requirement]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in reqs.iter().flatten() {
        for v in vars(p).unwrap_or_default() {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(&v)) {
                out.push(v);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&[&str]]) -> Vec<Requirement> {
        v.iter()
            .map(|r| r.iter().map(|p| p.to_string()).collect())
            .collect()
    }

    #[test]
    fn requirements_and_alternatives() {
        assert_eq!(
            parse("-- @requires 'a.parquet', b.csv c.json\nSELECT 1"),
            s(&[&["a.parquet"], &["b.csv"], &["c.json"]])
        );
        assert_eq!(
            parse("-- @requires a.parquet | b.parquet, ${rd}/c|${rd}/d e"),
            s(&[&["a.parquet", "b.parquet"], &["${rd}/c", "${rd}/d"], &["e"]])
        );
        assert!(parse("-- @requiresnothing\nSELECT 1").is_empty());
        assert!(issues("-- @requires a | b, c").is_empty());
        for bad in [
            "-- @requires a |",
            "-- @requires | a",
            "-- @requires a || b",
            "-- @requires a |, b",
            "-- @requires",
        ] {
            let i = issues(bad);
            assert_eq!(i.len(), 1, "{bad}: {i:?}");
            assert_eq!(i[0].0, "requires/empty-alternative");
        }
        for bad in ["-- @requires ${run/x", "-- @requires ${1x}/x"] {
            let i = issues(bad);
            assert_eq!(i.len(), 1, "{bad}: {i:?}");
            assert_eq!(i[0].0, "requires/bad-variable");
        }
    }

    #[test]
    fn variables_expand_and_paths_resolve() {
        let look = |n: &str| (n == "run").then(|| "r1".to_string());
        assert_eq!(expand("${run}/f.parquet", look), Ok("r1/f.parquet".into()));
        assert_eq!(expand("x${run}y${run}", look), Ok("xr1yr1".into()));
        assert_eq!(expand("plain", look), Ok("plain".into()));
        assert_eq!(
            expand("${nope}/f", look),
            Err(Unexpanded::Unknown("nope".into()))
        );
        assert!(matches!(expand("${", look), Err(Unexpanded::Malformed(_))));
        assert_eq!(vars("${a}/${b_2}/x"), Ok(vec!["a".into(), "b_2".into()]));

        assert_eq!(resolve("r1/f", Some("/srv/runs")), "/srv/runs/r1/f");
        assert_eq!(resolve("./r1/f", Some("/srv/runs/")), "/srv/runs/r1/f");
        assert_eq!(resolve("/abs/f", Some("/srv/runs")), "/abs/f");
        assert_eq!(resolve("s3://b/f", Some("/srv/runs")), "s3://b/f");
        assert_eq!(resolve("r1/f", None), "r1/f");
    }

    #[test]
    fn a_locked_path_stays_in_the_data_folder() {
        let root = "/srv/runs";
        for ok in [
            "r1/f.parquet",
            "/srv/runs/r1/f.parquet",
            "*/x.parquet",
            "**/x.parquet",
            "r1/[ab].parquet",
            "a..b/f",
        ] {
            assert!(confine(ok, root).is_ok(), "{ok}");
        }
        assert_eq!(confine("r1/f", root).unwrap(), "/srv/runs/r1/f");
        for bad in [
            "",
            "../etc/passwd",
            "r1/../../etc/passwd",
            "/srv/runs/../etc",
            "/srv/runs",
            "/srv/runs2/f",
            "/etc/passwd",
            "~/x",
            "r1/~/x",
            "http://h/x",
            "s3://b/x",
            "r1\\..\\x",
            "r1/\0x",
            "r1/\nx",
            "C:/x",
        ] {
            assert!(confine(bad, root).is_err(), "{bad:?}");
        }
        assert!(confine("f", "").is_err());
    }

    #[test]
    fn patterns_matches_and_the_cache() {
        assert!(has_glob("r*") && has_glob("a?") && has_glob("[ab]") && has_glob("{a,b}"));
        assert!(!has_glob("r1/forecasts.parquet"));
        assert!(files_sql("/d/r1/f.parquet").ends_with("LIMIT 1"));
        assert!(files_sql("/d/*/f.parquet").ends_with(&format!("LIMIT {MAX_MATCHES}")));
        assert!(files_sql("/d/a'b").contains("glob('/d/a''b')"));

        // A link inside the folder that points out of it never counts.
        let base = std::env::temp_dir().join(format!("afx-req-{}", std::process::id()));
        let (data, outside) = (base.join("data"), base.join("outside"));
        std::fs::create_dir_all(data.join("r1")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(data.join("r1/f.parquet"), b"x").unwrap();
        std::fs::write(outside.join("secret.parquet"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, data.join("lnk")).unwrap();
        let root = std::fs::canonicalize(&data).unwrap();
        assert!(inside_canonical(
            &data.join("r1/f.parquet").to_string_lossy(),
            &root
        ));
        #[cfg(unix)]
        assert!(!inside_canonical(
            &data.join("lnk/secret.parquet").to_string_lossy(),
            &root
        ));
        assert!(!inside_canonical(
            &data.join("nope").to_string_lossy(),
            &root
        ));
        std::fs::remove_dir_all(&base).unwrap();

        // A miss is remembered for the TTL: no second scan.
        let cache = ExistsCache::new(std::time::Duration::from_secs(60), 2);
        let probes = std::cell::Cell::new(0);
        let probe = |yes: bool| {
            probes.set(probes.get() + 1);
            Some(yes)
        };
        assert!(!cache.exists("/d/**/x", || probe(false)));
        assert!(!cache.exists("/d/**/x", || probe(true)));
        assert_eq!(probes.get(), 1);
        // Bounded: a third key evicts, the cache keeps working.
        assert!(cache.exists("/d/a", || probe(true)));
        assert!(cache.exists("/d/b", || probe(true)));
        assert_eq!(probes.get(), 3);
        // A failed / interrupted probe is "missing" for that call only: the
        // next caller (another viewer) probes again and sees the file.
        let c = ExistsCache::new(std::time::Duration::from_secs(60), 8);
        assert!(!c.exists("/d/slow/**/x", || None));
        assert!(c.exists("/d/slow/**/x", || probe(true)));
        assert!(c.exists("/d/slow/**/x", || panic!("now cached")));
        let expired = ExistsCache::new(std::time::Duration::ZERO, 4);
        assert!(!expired.exists("/d/x", || probe(false)));
        assert!(expired.exists("/d/x", || probe(true)));
    }

    #[test]
    fn helper_queries_quote_their_input() {
        assert_eq!(
            glob_sql("a'b"),
            "SELECT CAST(count(*) AS VARCHAR) AS n FROM glob('a''b')"
        );
        assert_eq!(
            vars_sql(&["a".into(), "b".into()]),
            "SELECT CAST(getvariable('a') AS VARCHAR) AS v0, CAST(getvariable('b') AS VARCHAR) AS v1"
        );
        assert_eq!(
            all_vars(&s(&[&["${a}/x", "${b}/y"], &["${A}/z", "${"]])),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
