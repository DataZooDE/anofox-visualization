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

/// Windows only (#333/#335): `std::env::set_var` calls `SetEnvironmentVariableW`
/// (Win32), which updates the process's OS environment block but NOT the
/// CRT's own cached environment table — the one `getenv`/`_wgetenv` actually
/// read. On Unix there is exactly one environment, shared by every module in
/// the process (glibc/libSystem `setenv` updates the global `environ` that
/// every `getenv` call — ours or anyone else's sharing this process — reads),
/// so this split does not exist there.
///
/// Two separate CRT linkage models make this show up differently:
///
///   * **Dynamic UCRT** (`ucrtbase.dll`, Rust's own default for the
///     `*-pc-windows-msvc` targets unless built with `-C target-feature=
///     +crt-static`): one shared instance per process. Every module that
///     dynamically links it shares its cached table — but only `_putenv`/
///     `_wputenv_s` (not a raw `SetEnvironmentVariableW` from any module)
///     updates that shared cache, so a plain `std::env::set_var` is invisible
///     to every `getenv()` in the process, including this module's own,
///     until something calls the CRT setter.
///   * **Static CRT** (`/MT` — the `x64-windows-static-release` vcpkg triplet
///     the Main Extension Distribution Pipeline and the official DuckDB CLI
///     release both build with, confirmed from vcpkg's own triplet file:
///     `VCPKG_CRT_LINKAGE static`): each module gets its OWN private copy of
///     the CRT environment table, populated once from the OS block when that
///     module's CRT starts up (an EXE's at process start, a DLL's at
///     `LoadLibrary` time). A module that already finished that snapshot
///     before we call `set_var` — notably `duckdb.exe` itself, which starts
///     and snapshots long before it ever `LOAD`s an extension — can never see
///     our write via its own `getenv()`, no matter what we do inside our
///     module. A module loaded AFTER us (e.g. a sibling anofox extension
///     `LOAD`ed later in the same session) snapshots from the *already
///     updated* OS block and does see it — which is exactly why
///     `SetEnvironmentVariableW`/`std::env::set_var` alone is still worth
///     doing: it is the one write every later-loaded static-CRT module's own
///     snapshot will pick up.
///
/// `_wputenv_s` additionally propagates to the OS environment block itself
/// (documented CRT behaviour), so calling both is not redundant: it covers
/// "this module's own `getenv`" and "any OTHER module sharing a dynamic UCRT
/// with us" (neither of which `SetEnvironmentVariableW` alone reaches), while
/// `set_var`/`SetEnvironmentVariableW` covers "a static-CRT module loaded
/// after us" (which reads the OS block at its own startup, not our CRT
/// cache). Nothing short of controlling every other module's build can reach
/// a static-CRT module that already started before us (`duckdb.exe`) — see
/// `probe` and `duckext/scripts/build-native.sh` for how the Windows smoke
/// test works around that instead of asserting the impossible.
#[cfg(windows)]
mod windows_crt {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    // Declared directly against the UCRT rather than pulled in via an extra
    // crate: both are ordinary `<stdlib.h>` exports that every `*-pc-windows-
    // msvc` Rust build already links against (statically or dynamically,
    // matching however the rest of the binary/DLL was built — e.g. Corrosion
    // matches `crt-static` to the surrounding CMake/MSVC runtime setting for
    // the `csrc/` build), so no extra link setup is needed here.
    extern "C" {
        // int _wputenv_s(const wchar_t *name, const wchar_t *value);
        fn _wputenv_s(name: *const u16, value: *const u16) -> i32;
        // errno_t _wgetenv_s(size_t *len, wchar_t *buffer, size_t numberOfElements, const wchar_t *varname);
        fn _wgetenv_s(
            ret_len: *mut usize,
            buffer: *mut u16,
            number_of_elements: usize,
            var_name: *const u16,
        ) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// This process's own CRT view of `name` — what `getenv()`/`_wgetenv()`
    /// (and so DuckDB's SQL `getenv()`, when it runs inside a module sharing
    /// this same CRT cache) currently return. `None` when unset.
    pub fn crt_getenv(name: &str) -> Option<String> {
        let wname = wide(name);
        let mut len: usize = 0;
        // SAFETY: a null buffer with `number_of_elements == 0` asks
        // `_wgetenv_s` to report the required length (incl. the NUL
        // terminator) in `len` without writing through the buffer pointer —
        // documented UCRT behaviour, used here only to size the real call.
        let rc = unsafe { _wgetenv_s(&mut len, std::ptr::null_mut(), 0, wname.as_ptr()) };
        if rc != 0 || len == 0 {
            return None; // unset, or the CRT call itself failed
        }
        let mut buf = vec![0u16; len];
        // SAFETY: `buf` has exactly `len` elements, matching
        // `number_of_elements`; `wname` is NUL-terminated.
        let rc = unsafe { _wgetenv_s(&mut len, buf.as_mut_ptr(), buf.len(), wname.as_ptr()) };
        if rc != 0 || len == 0 {
            return None;
        }
        // `len` includes the NUL terminator on return; strip it before decoding.
        Some(String::from_utf16_lossy(&buf[..len - 1]))
    }

    /// Set `name` in this module's own CRT environment cache (and, as a
    /// documented side effect, the OS environment block too — harmless
    /// alongside the explicit `SetEnvironmentVariableW` call the caller also
    /// makes; see the module docs for why both calls are needed regardless).
    pub fn crt_putenv(name: &str, value: &str) {
        let wname = wide(name);
        let wvalue = wide(value);
        // SAFETY: both buffers are NUL-terminated UTF-16 and kept alive for
        // the duration of the call; `_wputenv_s` copies them internally
        // (documented UCRT behaviour) rather than retaining the pointers.
        unsafe {
            let _ = _wputenv_s(wname.as_ptr(), wvalue.as_ptr());
        }
    }
}

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
        // "Unset" is decided from BOTH views on Windows (see `windows_crt`'s
        // docs): the OS process environment block (`std::env::var`) and this
        // process's own CRT cache (`getenv`, via `_wgetenv_s`) can disagree
        // there, and either one holding a real, explicit value means some
        // earlier code already made a choice that must not be overridden.
        // On every other platform there is exactly one environment, so this
        // is just `std::env::var`.
        let os_val = std::env::var(VAR).ok();
        #[cfg(windows)]
        let crt_val = windows_crt::crt_getenv(VAR);
        #[cfg(not(windows))]
        let crt_val: Option<String> = None;
        let explicit = [os_val.as_deref(), crt_val.as_deref()]
            .into_iter()
            .flatten()
            .find(|s| !s.trim().is_empty());

        if let Some(v) = default_value(explicit) {
            // SAFETY: `ENV_ONCE` makes this the only `set_var` call this
            // process will ever make for this variable, and callers run it
            // before spawning threads of their own (see the function docs).
            // Edition 2021 does not require `unsafe` here (set_var is a safe
            // fn in editions < 2024), but the block documents the real
            // precondition and keeps this call site unchanged if the crate
            // ever moves to edition 2024.
            unsafe { std::env::set_var(VAR, v) };
            // Windows only: also write through the CRT so THIS module's own
            // `getenv()` (and any other module sharing a dynamic UCRT with
            // us) sees "1" too — `SetEnvironmentVariableW` alone (what
            // `set_var` just did) does not reach either (see `windows_crt`'s
            // docs for why both calls are needed).
            #[cfg(windows)]
            windows_crt::crt_putenv(VAR, v);
        }
    });
}

/// Diagnostic only — not part of the public SQL surface, no stability
/// promise, and intentionally not documented in `docs/DOCS.md` beyond a
/// pointer to this doc comment. Exists because `getenv()` (DuckDB's own SQL
/// function) cannot prove the Windows fix works: it runs inside `duckdb.exe`,
/// whose statically-linked CRT snapshots the OS environment block at process
/// startup — before any `LOAD` — so it can never reflect a write any
/// extension makes afterwards, fixed or not (see `windows_crt`'s docs). This
/// reports what THIS process (the extension that just called
/// [`ensure_default_once`]) sees through each view instead, so a smoke test
/// running *inside* this extension (`duckext/scripts/build-native.sh`, via
/// the `anofox_telemetry_probe()` SQL function duckext registers) can
/// assert the fix actually wrote both: the OS block (reaches a later-loaded
/// static-CRT sibling) and this module's own CRT cache (reaches this same
/// module's `getenv`, and any sibling sharing a dynamic UCRT with it).
///
/// Format: `"os=<value> crt=<value>"`, each `<value>` the literal current
/// value or empty if unset. On a non-Windows target `crt` always equals
/// `os` (one shared environment; see the module docs) rather than being
/// probed separately.
pub fn probe() -> String {
    let os = std::env::var(VAR).ok().unwrap_or_default();
    #[cfg(windows)]
    let crt = windows_crt::crt_getenv(VAR).unwrap_or_default();
    #[cfg(not(windows))]
    let crt = os.clone();
    format!("os={os} crt={crt}")
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

    // windows_crt round-trips through the real UCRT, so it shares process
    // environment with every other test in this binary — each test below
    // picks a variable name no other test touches, rather than serialising
    // on a lock, since `cargo test` runs this file's tests concurrently in
    // one process.
    #[cfg(windows)]
    #[test]
    fn crt_roundtrip_matches_what_was_set() {
        windows_crt::crt_putenv("ANOFOX_TELEMETRY_CRT_TEST_SET", "banana");
        assert_eq!(
            windows_crt::crt_getenv("ANOFOX_TELEMETRY_CRT_TEST_SET").as_deref(),
            Some("banana"),
            "crt_getenv must see what crt_putenv just wrote"
        );
        assert_eq!(
            windows_crt::crt_getenv("ANOFOX_TELEMETRY_CRT_TEST_DEFINITELY_UNSET"),
            None,
            "crt_getenv must report None for a variable nothing ever set"
        );
    }

    #[cfg(windows)]
    #[test]
    fn crt_putenv_also_updates_the_os_block() {
        // Documented `_wputenv_s` behaviour (see the `windows_crt` module
        // docs): its write is visible through `std::env::var` too, not only
        // through `crt_getenv` — this is what makes calling it alongside
        // `set_var` non-redundant rather than contradictory.
        windows_crt::crt_putenv("ANOFOX_TELEMETRY_CRT_TEST_OSBLOCK", "1");
        assert_eq!(
            std::env::var("ANOFOX_TELEMETRY_CRT_TEST_OSBLOCK")
                .ok()
                .as_deref(),
            Some("1"),
            "crt_putenv's write must reach std::env::var (the OS block) too"
        );
    }

    // ensure_default_once() itself touches real process environment, which
    // every other test in this process shares — exercised instead by the
    // dashboard binary's own test (src/bin/dashboard.rs) and the CLI
    // integration test (tests/telemetry_default.rs), each in its own process.
}
