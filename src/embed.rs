//! The supported surface for embedding anofox-visualization as a library
//! (#1266): render a chart or a compiled dashboard to SVG, split and lint a
//! dashboard script, read the role registry, themes and locales. No server,
//! no authentication, no SQL execution — the host brings its own rows.
//!
//! Depend on the crate with `default-features = false`. This surface needs no
//! feature, and that build pulls in only `ggplot-rs`, `serde_json`,
//! `roxmltree` and `sha2`. CI keeps it that way (`scripts/check-embed-deps.sh`).
//!
//! **Compatibility.** Releases are CalVer (`vYYYY.M.D`), not semver. What
//! this module re-exports changes only in a release whose notes say so under
//! "Embedding API". A break is a removed or renamed item, a changed
//! signature, a new required field in a struct you build, or a value you
//! pass meaning something else. A new item, a new optional field, a new enum
//! variant, or different output (another SVG attribute, a new lint code) is
//! not a break. Everything outside this module may change in any release.
//! CI builds `crates/embed-host` against exactly this module.
//!
//! See `docs/embedding.md`.

/// One chart: JSON rows plus a role spec → SVG, within size limits.
pub use crate::host::{render_spec_checked, RenderLimits};

/// A compiled dashboard → one SVG; the host answers its queries.
pub use crate::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};

/// A dashboard script → its statements (author time).
pub use crate::sql::{plan, Panel};

/// The static lint, without a database (`run_query` may answer nothing).
pub use crate::lint::{check_opts, Diagnostic, LintOptions, Severity};

/// The role registry.
pub use crate::roles::{is_directive_panel, lookup, RoleSpec};
pub use crate::Role;

/// Themes and number/date locales.
pub use crate::format::Locale;
pub use crate::theme::{resolve_json, ResolvedTheme, ThemeSet, Variant};

/// A cell value (rows the host passes in).
pub use crate::Value;
