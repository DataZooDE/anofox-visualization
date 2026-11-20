# Embedding anofox-visualization as a library

A Rust program can render charts and dashboards with anofox-visualization
without its servers. It brings its own rows; nothing here runs SQL against a
database, opens a port or checks a login.

```toml
[dependencies]
anofox-visualization = { git = "https://github.com/DataZooDE/anofox-visualization", tag = "v2026.11.21", default-features = false }
```

Use only `anofox_visualization::embed`. Everything outside it may change in
any release.

## What you can call

| Item | Use |
|---|---|
| `render_spec_checked(spec, &RenderLimits)` | One chart: JSON rows plus a role spec (`{"rows": […], "roles": [[0, "XAXIS", "k"], [1, "BARCHART", "v"]], "width": 480, "height": 320}`), or a contract plot (`{"plot": "terms", …}`), to SVG within size limits. |
| `render_dashboard_svg(script, &mut impl DataProvider, &DashboardOptions)` | A dashboard script to one SVG. The host answers each statement's query through `DataProvider`. |
| `plan(script) -> Vec<Panel>` | A script split into its statements and roles (author time). |
| `check_opts(script, run_query, LintOptions) -> Vec<Diagnostic>` | The lint. `run_query` may answer nothing, for a static check without a database. `Severity` grades each finding. |
| `lookup(token) -> Option<&RoleSpec>`, `is_directive_panel(roles)`, `Role` | The role registry. |
| `resolve_json(&theme) -> ThemeSet`, `ThemeSet::pick(Variant)`, `ResolvedTheme` | Themes (the `theme.json` format). |
| `Locale::from_tag`, `Locale::TAGS` | Number and date locales. |
| `Value` | A cell value. |

`crates/embed-host` is a small host that uses all of these. CI builds and
tests it with `default-features = false` on every pull request.

## What counts as a breaking change

Releases are CalVer (`vYYYY.M.D`), not semver, so the version number doesn't
tell you. Instead:

- **Breaking:** a removed or renamed item in `embed`, a changed function
  signature, a new required field in a struct you build, or a value you pass
  meaning something else. The release notes say so under **Embedding API**.
- **Not breaking:** a new item, a new optional field, a new enum variant, or
  different output (another SVG attribute, a new lint code, a changed default
  colour).

A change that makes `crates/embed-host` fail to compile is breaking by
definition. The pull request that does it says so, and so do the notes of the
next release.

## Dependencies

With `default-features = false` the crate depends only on `ggplot-rs`,
`serde_json`, `roxmltree` and `sha2`, plus their small dependencies.
`scripts/check-embed-deps.sh` fails in CI if one of these joins that build:

- the HTTP server or TLS (`tiny_http`, `rustls`, `ureq`);
- auth (`jsonwebtoken`, `aws-lc-rs`);
- the plugin runtime (`wasmi`);
- raster or PDF export (`resvg`, `svg2pdf`, `png`);
- the browser glue (`wasm-bindgen`).

PNG and PDF export live in the DuckDB extension crate (`duckext/`), not here.
