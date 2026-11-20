//! Runtime plugins (#1213): a module compiled to one WebAssembly file,
//! loaded at run time instead of linked in (see [`crate::module`]).
//!
//! A plugin registers through the same [`crate::module::Module`] registry as
//! a linked module, so `::REQUIRES`, its roles and `--check` work the same.
//!
//! **Sandbox.** The plugin gets no imports: no file system, network, clock
//! or randomness — a module that imports anything is refused. Every call
//! runs on a fuel budget (CPU) and the plugin's memory is capped. A plugin
//! that traps, runs out of fuel or answers malformed JSON gives the panel an
//! error; its instance is then thrown away and made fresh for the next call.
//!
//! **Interface, version 1** (the guest side is the `anofox-viz-plugin`
//! crate, `export_plugin!`). Byte buffers cross as `(ptr, len)` in the
//! plugin's memory; a returned buffer is packed into one `i64`,
//! `ptr << 32 | len`, and freed by the host with `dealloc`.
//!
//! | export | signature | |
//! |---|---|---|
//! | `memory` | memory | |
//! | `anofox_plugin_abi` | `() -> i32` | `1` |
//! | `anofox_plugin_alloc` | `(len: i32) -> i32` | a buffer the host fills |
//! | `anofox_plugin_dealloc` | `(ptr: i32, len: i32)` | |
//! | `anofox_plugin_manifest` | `() -> i64` | `{"name", "roles": [{"token", "aliases", "summary"}]}` |
//! | `anofox_plugin_render` | `(ptr: i32, len: i32) -> i64` | in: [`render_input`]; out: `{"ok": svg_body}` or `{"err": message}` |
//! | `anofox_plugin_lint` | `(ptr: i32, len: i32) -> i64` | in: `{"abi", "columns"}`; out: `["warning", …]` |
//! | `anofox_plugin_render_contract` | `(ptr: i32, len: i32) -> i64` | optional; in: `{"abi", "plot", "rows", "options", "width", "height", "theme"}`; out: `{"ok"}` / `{"err"}` |
//! | `anofox_plugin_asset` | `(ptr: i32, len: i32) -> i64` | optional; in: `{"path"}`; out: the file's bytes |
//!
//! **Macros** (step 4): the manifest's `"macros"` declare contract macros —
//! `{"plot", "named": [[name, default], …], "options": [{"key", "param",
//! "fallback_var"?}], "description", "example"}`. The host writes every
//! macro's body itself, `anofox_plot_<plot>(tbl, …)` calling
//! `anofox_render(json_object('plot', …, 'options', json_object(…)))` like a
//! built-in contract macro: a plugin never ships SQL. A default is a literal
//! (`NULL`, a number, `'text'`, `true`/`false`) or `getvariable('name')`.
//!
//! **Browser files** (`"assets"`: their paths): run in the page, outside
//! the sandbox, so they load only when the operator allows them
//! ([`register_with`]'s `assets`).
//!
//! The plugin owns (and frees) the input buffer it is given.
//!
//! **Unstable**, like the module interface.

use crate::module::ModuleRole;
use crate::{Column, RenderOptions};
use serde_json::{json, Map, Value as Json};

#[cfg(all(feature = "plugins", not(target_arch = "wasm32")))]
mod native;
#[cfg(all(feature = "plugins", not(target_arch = "wasm32")))]
pub use native::*;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
mod browser;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub use browser::register_manifest;

/// The interface version this host speaks.
pub const ABI: i32 = 1;

/// A plugin's role token or alias: upper case letters, digits and `_`.
fn token_ok(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 64
        && t.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && !t.as_bytes()[0].is_ascii_digit()
}

/// A plugin's name: lower case letters, digits, `-` and `_` (it is a path
/// segment, `/modules/<name>/`).
fn name_ok(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// A checked manifest.
struct Manifest {
    name: &'static str,
    roles: &'static [ModuleRole],
    macros: &'static [crate::macros::MacroSpec],
    plots: Vec<&'static str>,
    asset_paths: Vec<String>,
}

/// An identifier the host writes into SQL: a–z, 0–9 and `_`, not first a digit.
fn ident_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && !s.as_bytes()[0].is_ascii_digit()
}

/// A macro parameter's default: a literal or `getvariable('name')` (a name
/// a dashboard sets, never a claim or a server variable).
fn default_ok(d: &str) -> bool {
    let d = d.trim();
    let lower = d.to_ascii_lowercase();
    if matches!(lower.as_str(), "null" | "true" | "false") {
        return true;
    }
    let num = d.strip_prefix('-').unwrap_or(d);
    if !num.is_empty()
        && num.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && num.bytes().filter(|b| *b == b'.').count() <= 1
        && num.as_bytes()[0].is_ascii_digit()
    {
        return true;
    }
    if let Some(inner) = d.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')) {
        return d.len() >= 2 && !inner.replace("''", "").contains('\'') && inner.len() <= 200;
    }
    lower
        .strip_prefix("getvariable('")
        .and_then(|x| x.strip_suffix("')"))
        .is_some_and(|v| ident_ok(v) && !v.starts_with("__"))
}

/// The body of contract macro `plot`, as `contract_body!` writes it.
fn contract_body(plot: &str, opts: &[(String, String)]) -> String {
    let mut s = format!(
        "anofox_render(json_object('plot', '{plot}', 'rows', to_json(list(tbl ORDER BY tbl))"
    );
    if !opts.is_empty() {
        let o: Vec<String> = opts.iter().map(|(k, e)| format!("'{k}', {e}")).collect();
        s.push_str(&format!(", 'options', json_object({})", o.join(", ")));
    }
    s.push_str(
        ", 'theme', coalesce(theme, getvariable('anofox_theme')), 'locale', coalesce(locale, \
         getvariable('anofox_locale')), 'width', width, 'height', height))",
    );
    s
}

/// The manifest's contract macros, as the host writes them.
fn macros(list: &[Json]) -> Result<(Vec<crate::macros::MacroSpec>, Vec<&'static str>), String> {
    if list.len() > 16 {
        return Err("manifest: more than 16 macros".into());
    }
    let (mut out, mut plots) = (Vec::new(), Vec::new());
    for m in list {
        let plot = m["plot"].as_str().unwrap_or("");
        if !ident_ok(plot) {
            return Err(format!("manifest: macro plot {plot:?} (a–z, 0–9 and _)"));
        }
        let mut named: Vec<(&'static str, &'static str)> = Vec::new();
        for n in m["named"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let (Some(k), Some(d)) = (n[0].as_str(), n[1].as_str()) else {
                return Err(format!(
                    "manifest: anofox_plot_{plot}: a named parameter isn't [name, default]"
                ));
            };
            if !ident_ok(k) || k == "tbl" || named.iter().any(|(x, _)| *x == k) {
                return Err(format!("manifest: anofox_plot_{plot}: parameter {k:?}"));
            }
            if !default_ok(d) {
                return Err(format!(
                    "manifest: anofox_plot_{plot}: default of {k} ({d:?}) must be a literal or getvariable('name')"
                ));
            }
            named.push((leak(k), leak(d.trim())));
        }
        for (k, d) in [
            ("theme", "NULL"),
            ("locale", "NULL"),
            ("width", "640"),
            ("height", "400"),
        ] {
            if !named.iter().any(|(x, _)| *x == k) {
                named.push((k, d));
            }
        }
        if named.len() > 40 {
            return Err(format!("manifest: anofox_plot_{plot}: too many parameters"));
        }
        let mut opts = Vec::new();
        for o in m["options"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let key = o["key"].as_str().unwrap_or("");
            let param = o["param"].as_str().unwrap_or("");
            if !ident_ok(key) || !named.iter().any(|(x, _)| *x == param) {
                return Err(format!(
                    "manifest: anofox_plot_{plot}: option {key:?} names no parameter of the macro"
                ));
            }
            let expr = match o["fallback_var"].as_str() {
                None => param.to_string(),
                Some(v) if ident_ok(v) && !v.starts_with("__") => {
                    format!("coalesce({param}, getvariable('{v}'))")
                }
                Some(v) => {
                    return Err(format!(
                        "manifest: anofox_plot_{plot}: fallback variable {v:?}"
                    ))
                }
            };
            opts.push((key.to_string(), expr));
        }
        let text = |k: &str, max: usize| -> &'static str {
            leak(
                &m[k]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(max)
                    .collect::<String>(),
            )
        };
        let plot: &'static str = leak(plot);
        plots.push(plot);
        out.push(crate::macros::MacroSpec {
            name: leak(&format!("anofox_plot_{plot}")),
            params: &["tbl"],
            named: Box::leak(named.into_boxed_slice()),
            body: leak(&contract_body(plot, &opts)),
            description: text("description", 2000),
            example: text("example", 1000),
            tags: &[],
        });
    }
    Ok((out, plots))
}

/// A browser file's path: `a-z0-9_-./`, no `..`, one of the page's types.
fn asset_path_ok(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 120
        && !p.starts_with('/')
        && !p
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == ".." || s.starts_with('.'))
        && p.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-./".contains(&b))
        && [".js", ".css", ".json", ".svg", ".png", ".woff2"]
            .iter()
            .any(|e| p.ends_with(e))
}

/// A root attribute's value (`root_attrs`), at most this many characters.
const MAX_ROOT_ATTR: usize = 64 << 10;

/// The browser files of a plugin, at most this many bytes together.
const MAX_ASSETS: usize = 8 << 20;

/// Parse and check a manifest.
fn manifest(m: &Json) -> Result<Manifest, String> {
    let (name, roles) = manifest_roles(m)?;
    let (macros, plots) = macros(m["macros"].as_array().map(Vec::as_slice).unwrap_or(&[]))?;
    let asset_paths: Vec<String> = m["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Json::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    if let Some(p) = asset_paths.iter().find(|p| !asset_path_ok(p)) {
        return Err(format!("manifest: asset path {p:?}"));
    }
    if asset_paths.len() > 64 {
        return Err("manifest: more than 64 assets".into());
    }
    Ok(Manifest {
        name,
        roles,
        macros: Box::leak(macros.into_boxed_slice()),
        plots,
        asset_paths,
    })
}

/// Parse and check a manifest's name and roles.
fn manifest_roles(m: &Json) -> Result<(&'static str, &'static [ModuleRole]), String> {
    let name = m["name"].as_str().ok_or("manifest: no name")?;
    if !name_ok(name) {
        return Err(format!("manifest: name {name:?} (a–z, 0–9, - and _)"));
    }
    let mut roles = Vec::new();
    for r in m["roles"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let token = r["token"]
            .as_str()
            .ok_or("manifest: a role without a token")?;
        let aliases: Vec<&str> = r["aliases"]
            .as_array()
            .map(|a| a.iter().filter_map(Json::as_str).collect())
            .unwrap_or_default();
        for t in std::iter::once(token).chain(aliases.iter().copied()) {
            if !token_ok(t) {
                return Err(format!("manifest: role ::{t} (A–Z, 0–9 and _)"));
            }
        }
        let summary: String = r["summary"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(200)
            .collect();
        let aliases: Vec<&'static str> = aliases.into_iter().map(leak).collect();
        roles.push(ModuleRole {
            token: leak(token),
            aliases: Box::leak(aliases.into_boxed_slice()),
            summary: leak(&summary),
        });
    }
    if roles.len() > 64 {
        return Err("manifest: more than 64 roles".into());
    }
    Ok((leak(name), Box::leak(roles.into_boxed_slice())))
}

fn value(v: &crate::Value) -> Json {
    use crate::Value as V;
    match v {
        V::Float(f) if f.is_finite() => json!(f),
        V::Float(_) | V::Na => Json::Null,
        V::Integer(i) => json!(i),
        V::Str(s) => json!(s),
        V::Bool(b) => json!(b),
        V::DateTime(t) => json!({ "datetime": t }),
    }
}

fn columns(cols: &[Column]) -> Json {
    Json::Array(
        cols.iter()
            .map(|c| {
                json!({
                    "name": c.name,
                    "role": c.role.token(),
                    "values": c.values.iter().map(value).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

/// The input of `anofox_plugin_render`.
pub fn render_input(cols: &[Column], width: u32, height: u32, o: &RenderOptions) -> Json {
    use crate::theme::hex;
    let t = &o.theme;
    let mut theme = Map::new();
    theme.insert(
        "dark".into(),
        json!(t.variant == crate::theme::Variant::Dark),
    );
    theme.insert(
        "palette".into(),
        json!(t.palette.iter().map(|c| hex(*c)).collect::<Vec<_>>()),
    );
    theme.insert("ink".into(), json!(hex(t.colors.ink)));
    theme.insert("muted".into(), json!(hex(t.colors.muted)));
    theme.insert("card".into(), json!(hex(t.colors.card)));
    theme.insert("accent".into(), json!(hex(t.colors.accent)));
    json!({
        "abi": ABI,
        "columns": columns(cols),
        "width": width,
        "height": height,
        "theme": theme,
    })
}

/// `{"ok": svg}` → the SVG, held to the drawing-only allow-list (the
/// body goes into the page: only drawing markup, never script, handlers or
/// external references — what `--site` checks published SVGs with).
pub(crate) fn checked_svg(name: &str, out: &Json) -> Result<String, String> {
    match (&out["ok"], &out["err"]) {
        (Json::String(svg), _) => {
            let doc = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" \
                 xmlns:xlink=\"http://www.w3.org/1999/xlink\">{svg}</svg>"
            );
            match crate::svg_safe::svg_active_reason(doc.as_bytes()) {
                None => Ok(svg.clone()),
                Some(why) => Err(format!("plugin {name}: its SVG {why}")),
            }
        }
        (_, Json::String(e)) => Err(format!("{name}: {e}")),
        _ => Err(format!("plugin {name}: answered neither ok nor err")),
    }
}

/// A plugin's answer for a contract plot: `{"ok": svg}` (held to the
/// allow-list) or `{"columns": [{"name", "role", "values"}], "root_attrs",
/// "warnings"}` (#1218) — columns the host draws itself, each with a role
/// token the host knows (no SVG from the plugin at all).
pub(crate) fn contract_answer(
    name: &str,
    out: &Json,
) -> Result<crate::module::ContractAnswer, String> {
    use crate::module::ContractAnswer;
    let Some(cols) = out["columns"].as_array() else {
        return checked_svg(name, out).map(ContractAnswer::Svg);
    };
    if cols.len() > 64 {
        return Err(format!("plugin {name}: more than 64 columns"));
    }
    let mut parsed = Vec::with_capacity(cols.len());
    for c in cols {
        let col = c["name"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(120)
            .collect::<String>();
        let token = c["role"].as_str().unwrap_or("");
        let role = crate::parse_role(token).ok_or_else(|| {
            format!("plugin {name}: column {col:?} has no known role ({token:?})")
        })?;
        let values: Vec<crate::Value> = c["values"]
            .as_array()
            .map(|v| v.iter().map(json_value).collect())
            .unwrap_or_default();
        parsed.push(Column::new(col, role, values));
    }
    let root_attrs = out["root_attrs"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| {
                    k.starts_with("data-")
                        && k.len() <= 40
                        && k.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                })
                .filter_map(|(k, v)| {
                    // #1276: room for a network's active paths (JSON)
                    v.as_str()
                        .map(|v| (k.clone(), v.chars().take(MAX_ROOT_ATTR).collect()))
                })
                .collect()
        })
        .unwrap_or_default();
    let warnings = out["warnings"]
        .as_array()
        .map(|w| {
            w.iter()
                .take(50)
                .filter_map(Json::as_str)
                .map(|s| s.chars().take(500).collect())
                .collect()
        })
        .unwrap_or_default();
    Ok(ContractAnswer::Columns {
        cols: parsed,
        root_attrs,
        warnings,
    })
}

/// A JSON value as a cell: numbers, text, bools, `null`, `{"datetime": s}`.
fn json_value(v: &Json) -> crate::Value {
    use crate::Value as V;
    match v {
        Json::Null => V::Na,
        Json::Bool(b) => V::Bool(*b),
        Json::Number(n) => n
            .as_i64()
            .map(V::Integer)
            .or_else(|| n.as_f64().map(V::Float))
            .unwrap_or(V::Na),
        Json::String(s) => V::Str(s.clone()),
        Json::Object(o) => o
            .get("datetime")
            .and_then(Json::as_i64)
            .map(V::DateTime)
            .unwrap_or(V::Na),
        Json::Array(_) => V::Na,
    }
}

#[doc(hidden)]
pub fn contract_answer_for_test(
    name: &str,
    out: &Json,
) -> Result<crate::module::ContractAnswer, String> {
    contract_answer(name, out)
}
