//! `DATAZOO_DISABLE_TELEMETRY` — the one switch shared by every anofox
//! extension (anofox_statistics, anofox_forecast, anofox_tabular,
//! anofox_optimize, anofox_decide) and checked, per `posthog-telemetry`,
//! before anything is sent: an exact match (byte-for-byte) against `"1"`,
//! `"true"` or `"yes"` disables; anything else — including `"0"`, `"false"`,
//! `"no"` or an empty string — leaves telemetry on. Owner decision (#333):
//! every anofox extension loaded through anofox-visualization — the
//! `dashboard`/`serve` binaries and the extension itself — defaults that
//! switch to **off**, by setting the variable to `"1"` if the user never set
//! it at all. An explicit value, whatever it is, is the user's own choice and
//! is left untouched.
//!
//! Every caller funnels through [`default_value`] (the pure decision, unit
//! tested directly) and [`ensure_default_once`] (the one place that actually
//! calls [`std::env::set_var`]).

#[cfg(not(target_arch = "wasm32"))]
use std::sync::Once;

/// The variable this whole family of extensions reads.
pub const VAR: &str = "DATAZOO_DISABLE_TELEMETRY";

/// Pure decision: given the variable's `current` value (`None` = unset),
/// what should this process set it to? `Some(v)` means "set it to `v`";
/// `None` means "leave it alone".
///
/// `current` being `Some(_)` — any string, including `"0"`/`"false"`/`"no"`
/// or `""` — means the user (or a parent process, e.g. this repo's own CI and
/// conformance scripts) already made a choice; we never override it. Only an
/// actually-unset variable gets our default.
pub fn default_value(current: Option<&str>) -> Option<&'static str> {
    match current {
        None => Some("1"),
        Some(_) => None,
    }
}

/// Is telemetry disabled under the exact parsing the anofox extensions use
/// (see the module docs)? Used to decide whether the best-effort
/// `SET anofox_telemetry_enabled = false` (for an anofox extension loaded
/// before ours, whose load event already went out) is this process's
/// intent, rather than forcing it off even when the user explicitly opted
/// back in.
pub fn is_disabled(current: Option<&str>) -> bool {
    matches!(current, Some("1") | Some("true") | Some("yes"))
}

#[cfg(not(target_arch = "wasm32"))]
static ENV_ONCE: Once = Once::new();

/// Set [`VAR`] to `"1"` if unset, once per process. Every embedding that can
/// load an anofox extension calls this first thing — before any thread or
/// DuckDB instance exists, in the `dashboard`/`serve` binaries' `main()`; as
/// early as possible in the extension's own load callback otherwise, where
/// other threads may already exist.
///
/// # Thread safety
/// [`std::env::set_var`] races with any concurrent `getenv` (libc has no
/// locking between them — this is why it is `unsafe fn` as of the 2024
/// edition; see the call sites' SAFETY comments). We cannot make the write
/// itself atomic with respect to other libraries' reads, so we minimise the
/// window instead: [`Once`] guarantees the process performs this write at
/// most once, as early in the embedding's lifecycle as it can — before any
/// thread is spawned in the `dashboard`/`serve` binaries, and as the first
/// action of the extension's load callback (which DuckDB runs on the
/// thread that issued `LOAD`/`INSTALL`, before that query's own worker
/// threads fan out) for the extension. A DuckDB build already running other
/// queries on a pre-existing thread pool when ours loads is the remaining
/// theoretical race; we accept it as the safest practical option left once
/// loading has to happen inside someone else's process, and recommend
/// loading anofox-visualization first (or setting the variable before any
/// extension loads) when that matters.
///
/// A no-op on `wasm32` (DuckDB-Wasm, including `duckext`'s experimental
/// side-module build): there is no real process environment there —
/// `std::env::set_var` is unconditionally unsupported and panics (`-C
/// panic=abort` for that build turns it into a hard abort, not a catchable
/// error) — and it is moot anyway, since the browser's anofox DuckDB-Wasm
/// builds have telemetry compiled out (see `docs/DOCS.md`).
pub fn ensure_default_once() {
    #[cfg(not(target_arch = "wasm32"))]
    ENV_ONCE.call_once(|| {
        if let Some(v) = default_value(std::env::var(VAR).ok().as_deref()) {
            // SAFETY: `ENV_ONCE` makes this the only `set_var` call this
            // process will ever make for this variable, and callers run it
            // before spawning threads of their own (see the function docs).
            // Edition 2021 does not require `unsafe` here (set_var is a safe
            // fn in editions < 2024), but the block documents the real
            // precondition and keeps this call site unchanged if the crate
            // ever moves to edition 2024.
            unsafe { std::env::set_var(VAR, v) };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_gets_the_default() {
        assert_eq!(default_value(None), Some("1"));
    }

    #[test]
    fn any_explicit_value_is_kept() {
        for v in ["0", "false", "no", "1", "true", "yes", "", "banana"] {
            assert_eq!(
                default_value(Some(v)),
                None,
                "value {v:?} must be left alone"
            );
        }
    }

    #[test]
    fn is_disabled_matches_the_exact_set() {
        assert!(is_disabled(Some("1")));
        assert!(is_disabled(Some("true")));
        assert!(is_disabled(Some("yes")));
        assert!(!is_disabled(Some("0")));
        assert!(!is_disabled(Some("false")));
        assert!(!is_disabled(Some("no")));
        assert!(!is_disabled(Some("")));
        assert!(!is_disabled(Some("TRUE"))); // case-sensitive, like posthog-telemetry
        assert!(!is_disabled(None));
    }

    // ensure_default_once() itself touches real process environment, which
    // every other test in this process shares — exercised instead by the
    // dashboard binary's own test (src/bin/dashboard.rs) and the CLI
    // integration test (tests/telemetry_default.rs), each in its own process.
}
