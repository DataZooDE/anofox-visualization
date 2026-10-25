//! The one list of keys `anofox_serve_dashboards`'s `options` JSON accepts
//! (`duckext/src/serve.rs`'s `LockedOptions::parse`), annotated by how
//! `dashboard --site`'s `parse_site_options` (`src/bin/dashboard.rs`) treats
//! each one (#501). `--site` can't literally call `LockedOptions::parse` —
//! `duckext` depends on this crate, not the other way round — so this list
//! is the shared source both sides check an unknown key against, instead of
//! two hand-written key lists that only agree by accident. A server option
//! `duckext` adds without a matching entry here becomes an "unknown key" for
//! `--site` even though the server accepts it; add it here alongside the
//! `LockedOptions::parse` match arm that handles it.

/// How `--site` treats one `options` key the locked server also accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteOptionKind {
    /// Used by a published site exactly as by the locked server:
    /// `path_vars`, `nav_picker`, `app`, `home`, `theme`.
    Site,
    /// Makes no sense for a site with no server behind it, and — unlike a
    /// server-only limit — its *absence* is a security property (a site
    /// publishes the data its `auth`/`policy` would otherwise hide), so
    /// `--site` refuses it instead of silently ignoring it: `auth`, `policy`.
    Refused,
    /// A server-only tunable (row caps, timeouts, the listen address, …)
    /// with no effect on a static site — ignored, named once on stderr
    /// rather than refused, so a locked server's own `options` file can be
    /// handed to `--site` unedited.
    ServerOnly,
}

/// Every key `LockedOptions::parse` matches on, in that match's own order
/// (easy to diff against `duckext/src/serve.rs` by eye), each tagged with
/// how `--site` treats it.
pub const SERVER_OPTION_KEYS: &[(&str, SiteOptionKind)] = &[
    ("max_rows", SiteOptionKind::ServerOnly),
    ("timeout_ms", SiteOptionKind::ServerOnly),
    ("max_body_bytes", SiteOptionKind::ServerOnly),
    ("threads", SiteOptionKind::ServerOnly),
    ("warm", SiteOptionKind::ServerOnly),
    ("frame_ancestors", SiteOptionKind::ServerOnly),
    ("cache_mb", SiteOptionKind::ServerOnly),
    ("attach", SiteOptionKind::ServerOnly),
    ("load", SiteOptionKind::ServerOnly),
    ("theme", SiteOptionKind::Site),
    ("hosts", SiteOptionKind::ServerOnly),
    ("refresh_s", SiteOptionKind::ServerOnly),
    ("data", SiteOptionKind::ServerOnly),
    ("path_vars", SiteOptionKind::Site),
    ("nav_picker", SiteOptionKind::Site),
    ("auth", SiteOptionKind::Refused),
    ("audit", SiteOptionKind::ServerOnly),
    ("log", SiteOptionKind::ServerOnly),
    ("log_vars", SiteOptionKind::ServerOnly),
    ("bind", SiteOptionKind::ServerOnly),
    ("memory_limit", SiteOptionKind::ServerOnly),
    ("duckdb_threads", SiteOptionKind::ServerOnly),
    ("policy", SiteOptionKind::Refused),
    ("policy_key_env", SiteOptionKind::ServerOnly),
    ("max_inflight_mb", SiteOptionKind::ServerOnly),
    ("max_response_mb", SiteOptionKind::ServerOnly),
    ("max_queries", SiteOptionKind::ServerOnly),
    ("search_path", SiteOptionKind::ServerOnly),
    ("app", SiteOptionKind::Site),
    ("home", SiteOptionKind::Site),
    ("source", SiteOptionKind::ServerOnly),
    // #517 S2: per-viewer append-only state needs a real server behind it
    // (the write endpoint, the store) — a static site has neither.
    ("state", SiteOptionKind::Refused),
];

/// `None` for a key that isn't one of `anofox_serve_dashboards`'s options at
/// all (a typo, not a real option under any name).
pub fn lookup(key: &str) -> Option<SiteOptionKind> {
    SERVER_OPTION_KEYS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, kind)| *kind)
}

/// `duckext/src/serve.rs`'s unknown-key error lists every key it accepts;
/// built from [`SERVER_OPTION_KEYS`] so that message and this list can't
/// drift apart either.
pub fn all_keys_joined() -> String {
    SERVER_OPTION_KEYS
        .iter()
        .map(|(k, _)| *k)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_distinct() {
        let mut keys: Vec<&str> = SERVER_OPTION_KEYS.iter().map(|(k, _)| *k).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate key in SERVER_OPTION_KEYS");
    }

    #[test]
    fn lookup_finds_known_keys_and_rejects_typos() {
        assert_eq!(lookup("max_rows"), Some(SiteOptionKind::ServerOnly));
        assert_eq!(lookup("theme"), Some(SiteOptionKind::Site));
        assert_eq!(lookup("auth"), Some(SiteOptionKind::Refused));
        assert_eq!(lookup("max_rowz"), None);
    }
}
