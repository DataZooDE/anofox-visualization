//! `DATAZOO_DISABLE_TELEMETRY` — the one switch shared by every anofox
//! extension (anofox_statistics, anofox_forecast, anofox_tabular,
//! anofox_optimize, anofox_decide) and checked, per `posthog-telemetry`,
//! before anything is sent: an exact match (byte-for-byte) against `"1"`,
//! `"true"` or `"yes"` disables; anything else — including `"0"`, `"false"`,
//! `"no"` or an empty string — leaves telemetry on. Owner decision (#333):
//! every anofox extension loaded through anofox-visualization — the
//! `dashboard`/`serve` binaries and the extension itself — defaults that
//! switch to **off**, by setting the variable to `"1"` if the user never set
//! it to a non-empty value at all. An explicit non-empty value, whatever it
//! is, is the user's own choice and is left untouched; an empty or
//! whitespace-only value is treated the same as unset (templates such as a
//! compose `VAR=${VAR}` or a bare systemd `Environment=VAR=` leave the
//! variable set-but-empty for an operator who never touched it — that must
//! not read as an opt-in).
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
/// `current` being `Some(s)` with a non-empty, non-whitespace `s` — any
/// string, including `"0"`/`"false"`/`"no"` — means the user (or a parent
/// process, e.g. this repo's own CI and conformance scripts) already made a
/// choice; we never override it. An unset variable, or one set to `""` or
/// only whitespace, gets our default: a deployment template that renders the
/// variable empty when nobody set it (a compose `VAR=${VAR}`, a bare systemd
/// `Environment=VAR=`, an unset Helm value) must not be read as an explicit
/// opt-in — `posthog-telemetry` treats `""` as "telemetry on" (it only
/// matches `"1"`/`"true"`/`"yes"`), so leaving it alone here would too.
pub fn default_value(current: Option<&str>) -> Option<&'static str> {
    match current {
        None => Some("1"),
        Some(s) if s.trim().is_empty() => Some("1"),
        Some(_) => None,
    }
}

/// Is telemetry disabled under the exact parsing the anofox extensions use
/// (see the module docs)? Used to decide whether the best-effort
/// `SET anofox_telemetry_enabled = false` (for an anofox extension loaded
/// before ours, whose load event may already have been reported) is this
/// process's intent, rather than forcing it off even when the user
/// explicitly opted back in.
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
/// most once, as early as each embedding can manage — before any thread of
/// its own is spawned in the `dashboard`/`serve` binaries' `main()`, and as
/// the first action of the extension's load callback.
///
/// In the binaries that is the whole story: nothing has run yet. In the
/// extension, DuckDB's own worker threads (the `TaskScheduler`'s pool)
/// already exist by the time `LOAD` runs — they start with the database, not
/// with this call — but they are idle at this point and never call `getenv`
/// on their own, so they are not a realistic reader. The one concurrent
/// reader worth worrying about is a *different* anofox extension, loaded
/// earlier in the same process, whose own telemetry library may have a
/// background worker thread calling `getenv(DATAZOO_DISABLE_TELEMETRY)` at
/// send time (`posthog-telemetry`'s `PostHogProcessBatch`) at the same
/// moment we call `set_var`. The variable's value is never attacker-
/// controlled either way, and the write happens at most once per process;
/// we accept this as the safest practical option left once loading has to
/// happen inside someone else's process, and recommend loading
/// anofox-visualization first (or setting the variable before any extension
/// loads) when avoiding even that residual race matters.
///
/// A no-op on `wasm32-unknown-unknown` (`std::env::set_var` is
/// unconditionally unsupported there and panics — `duckext`'s experimental
/// side-module build uses `-C panic=abort`, which would turn that into a
/// hard abort, not a catchable error). `wasm32-unknown-emscripten` (the
/// target duckext's wasm build actually ships) does support real env vars,
/// but the no-op is gated on `target_arch` rather than threading the OS in
/// too, for a single simple rule covering both wasm32 targets; it is moot
/// either way, since the browser's anofox DuckDB-Wasm builds have telemetry
/// compiled out (see `docs/DOCS.md`).
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
    fn empty_or_whitespace_is_treated_as_unset() {
        // A deployment template that renders the variable empty when nobody
        // set it (compose `VAR=${VAR}`, systemd `Environment=VAR=`, an unset
        // Helm value) must get our default, not read as an opt-in (#333 L1).
        for v in ["", " ", "\t", "\n", "  \t\n "] {
            assert_eq!(
                default_value(Some(v)),
                Some("1"),
                "value {v:?} must be treated as unset"
            );
        }
    }

    #[test]
    fn any_non_empty_explicit_value_is_kept() {
        for v in ["0", "false", "no", "1", "true", "yes", "banana", " 1 "] {
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
