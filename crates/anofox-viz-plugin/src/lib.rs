//! Write an anofox-visualization runtime plugin (#1213): a module compiled
//! to one WebAssembly file (`--target wasm32-unknown-unknown`, crate type
//! `cdylib`), loaded at run time.
//!
//! ```ignore
//! use anofox_viz_plugin::{export_plugin, Manifest, Plugin, RenderInput, Role};
//!
//! struct Hello;
//! impl Plugin for Hello {
//!     const MANIFEST: Manifest = Manifest {
//!         name: "hello",
//!         roles: &[Role { token: "HELLO", aliases: &[], summary: "a greeting" }],
//!     };
//!     fn render(input: &RenderInput) -> Result<String, String> {
//!         let who = input.columns[0].str(0).unwrap_or("world");
//!         Ok(format!("<text x=\"10\" y=\"20\">Hello, {}</text>",
//!                    anofox_viz_plugin::escape(who)))
//!     }
//! }
//! export_plugin!(Hello);
//! ```
//!
//! A plugin runs in a sandbox: it imports nothing (no file system, network,
//! clock or randomness), each call has a CPU budget, its memory is capped,
//! and the SVG it returns may only draw (no script, event handlers or
//! external references). A panic ends the call with an error.
//!
//! **Unstable**: interface version [`ABI`] may change until a first real
//! plugin exists.

use serde_json::Value as Json;

pub use serde_json;

/// The interface version this crate speaks.
pub const ABI: i32 = 1;

/// One role the plugin brings (`::TOKEN`).
pub struct Role {
    /// Upper case letters, digits and `_`.
    pub token: &'static str,
    pub aliases: &'static [&'static str],
    /// One line for `dashboard --roles`.
    pub summary: &'static str,
}

/// The plugin's name and roles.
pub struct Manifest {
    /// Lower case letters, digits, `-` and `_`; what dashboards name in
    /// `'name'::REQUIRES`.
    pub name: &'static str,
    pub roles: &'static [Role],
}

/// One column of the panel's statement.
pub struct Column {
    pub name: String,
    /// The column's role token (`"HELLO"`, `"LABEL"`, …); empty for a plain
    /// column.
    pub role: String,
    /// One value per row: a number, a string, a bool, `null`, or
    /// `{"datetime": seconds since 1970}`.
    pub values: Vec<Json>,
}

impl Column {
    /// Row `i` as a number.
    pub fn f64(&self, i: usize) -> Option<f64> {
        self.values.get(i)?.as_f64()
    }
    /// Row `i` as a string.
    pub fn str(&self, i: usize) -> Option<&str> {
        self.values.get(i)?.as_str()
    }
}

/// The theme to paint with, as `#RRGGBB`.
pub struct Theme {
    pub dark: bool,
    pub palette: Vec<String>,
    pub ink: String,
    pub muted: String,
    pub card: String,
    pub accent: String,
}

/// What [`Plugin::render`] draws from.
pub struct RenderInput {
    pub columns: Vec<Column>,
    pub width: u32,
    pub height: u32,
    pub theme: Theme,
}

impl RenderInput {
    /// The first column with role `token`.
    pub fn role(&self, token: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.role == token)
    }
}

/// One option a contract macro passes to [`Plugin::render_contract`]:
/// `key` ← the macro's parameter `param`, else `getvariable(fallback_var)`.
pub struct ContractOption {
    pub key: &'static str,
    pub param: &'static str,
    pub fallback_var: Option<&'static str>,
}

/// A contract macro, `anofox_plot_<plot>(tbl, <named> …)`. The host writes
/// its SQL; a plugin only names its parts. A default is a literal (`NULL`,
/// a number, `'text'`, `true`/`false`) or `getvariable('name')`; `theme`,
/// `locale`, `width` and `height` are added when not named.
pub struct ContractMacro {
    pub plot: &'static str,
    pub named: &'static [(&'static str, &'static str)],
    pub options: &'static [ContractOption],
    pub description: &'static str,
    pub example: &'static str,
}

/// What [`Plugin::render_contract`] draws from: the table's rows as JSON
/// objects and the macro's options.
pub struct ContractInput {
    pub plot: String,
    pub rows: Vec<serde_json::Map<String, Json>>,
    pub options: serde_json::Map<String, Json>,
    pub width: u32,
    pub height: u32,
    pub theme: Theme,
}

/// One column of a [`ContractOutput::Columns`] answer: a role token the
/// host knows (`"XAXIS"`, `"BARCHART"`, `"DIAGRAM"`, …) and one value per row.
pub struct OutColumn {
    pub name: String,
    pub role: String,
    pub values: Vec<Json>,
}

/// What [`Plugin::render_contract`] answers: an SVG body, or columns the
/// host draws itself with its own renderer and the viewer's theme — the
/// way to reuse a built-in panel (a `::DIAGRAM`, a chart) from a plugin.
pub enum ContractOutput {
    Svg(String),
    Columns {
        columns: Vec<OutColumn>,
        /// `data-…` attributes for the panel's root `<svg>`.
        root_attrs: Vec<(String, String)>,
        warnings: Vec<String>,
    },
}

/// A plugin. Export it with [`export_plugin!`].
pub trait Plugin {
    const MANIFEST: Manifest;
    /// The panel's SVG body (inside an `<svg>` of `width` × `height`); an
    /// error is shown as the panel's error.
    fn render(input: &RenderInput) -> Result<String, String>;
    /// What `--check` should warn about a panel of the plugin's.
    fn lint(columns: &[Column]) -> Vec<String> {
        let _ = columns;
        Vec::new()
    }
    /// The contract macros it brings, drawn by [`Plugin::render_contract`].
    const MACROS: &'static [ContractMacro] = &[];
    /// Draw contract plot `input.plot` (one of [`Plugin::MACROS`]): the SVG
    /// body inside a `width` × `height` `<svg>`.
    fn render_contract(input: &ContractInput) -> Result<ContractOutput, String> {
        Err(format!("no contract plot {}", input.plot))
    }
    /// Browser files `(path, bytes)`, served at `/modules/<name>/<path>`
    /// only where the operator allows a plugin's browser files (they run in
    /// the page, outside the sandbox): `.js`, `.css`, `.json`, `.svg`,
    /// `.png`, `.woff2`.
    const ASSETS: &'static [(&'static str, &'static [u8])] = &[];
}

/// Escape text for SVG.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[doc(hidden)]
pub mod __rt {
    use super::*;

    pub fn alloc(len: i32) -> i32 {
        let b = vec![0u8; len.max(0) as usize].into_boxed_slice();
        Box::into_raw(b) as *mut u8 as usize as i32
    }

    /// # Safety
    /// `ptr`/`len` came from [`alloc`] or a returned buffer.
    pub unsafe fn dealloc(ptr: i32, len: i32) {
        let s = std::ptr::slice_from_raw_parts_mut(ptr as usize as *mut u8, len.max(0) as usize);
        drop(Box::from_raw(s));
    }

    /// # Safety
    /// `ptr`/`len` came from [`alloc`] (the input is taken and freed).
    pub unsafe fn take(ptr: i32, len: i32) -> Vec<u8> {
        let s = std::ptr::slice_from_raw_parts_mut(ptr as usize as *mut u8, len.max(0) as usize);
        Box::from_raw(s).into_vec()
    }

    pub fn give(bytes: Vec<u8>) -> i64 {
        let b = bytes.into_boxed_slice();
        let len = b.len() as u64;
        let ptr = Box::into_raw(b) as *mut u8 as usize as u64;
        ((ptr << 32) | len) as i64
    }

    pub fn manifest<P: Plugin>() -> i64 {
        let m = &P::MANIFEST;
        let roles: Vec<Json> = m
            .roles
            .iter()
            .map(|r| serde_json::json!({ "token": r.token, "aliases": r.aliases, "summary": r.summary }))
            .collect();
        let macros: Vec<Json> = P::MACROS
            .iter()
            .map(|c| {
                serde_json::json!({
                    "plot": c.plot,
                    "named": c.named.iter().map(|(n, d)| [n, d]).collect::<Vec<_>>(),
                    "options": c.options.iter().map(|o| serde_json::json!({
                        "key": o.key, "param": o.param, "fallback_var": o.fallback_var,
                    })).collect::<Vec<_>>(),
                    "description": c.description,
                    "example": c.example,
                })
            })
            .collect();
        let assets: Vec<&str> = P::ASSETS.iter().map(|(p, _)| *p).collect();
        give(
            serde_json::json!({ "name": m.name, "roles": roles, "macros": macros, "assets": assets })
                .to_string()
                .into_bytes(),
        )
    }

    fn theme(t: &Json) -> Theme {
        Theme {
            dark: t["dark"].as_bool().unwrap_or(false),
            palette: t["palette"]
                .as_array()
                .map(|a| a.iter().map(s).collect())
                .unwrap_or_default(),
            ink: s(&t["ink"]),
            muted: s(&t["muted"]),
            card: s(&t["card"]),
            accent: s(&t["accent"]),
        }
    }

    pub fn render_contract<P: Plugin>(input: &[u8]) -> i64 {
        let out = match serde_json::from_slice::<Json>(input) {
            Ok(v) => {
                let input = ContractInput {
                    plot: s(&v["plot"]),
                    rows: v["rows"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|r| r.as_object().cloned()).collect())
                        .unwrap_or_default(),
                    options: v["options"].as_object().cloned().unwrap_or_default(),
                    width: v["width"].as_u64().unwrap_or(0) as u32,
                    height: v["height"].as_u64().unwrap_or(0) as u32,
                    theme: theme(&v["theme"]),
                };
                match P::render_contract(&input) {
                    Ok(ContractOutput::Svg(svg)) => serde_json::json!({ "ok": svg }),
                    Ok(ContractOutput::Columns {
                        columns,
                        root_attrs,
                        warnings,
                    }) => serde_json::json!({
                        "columns": columns.into_iter().map(|c| serde_json::json!({
                            "name": c.name, "role": c.role, "values": c.values,
                        })).collect::<Vec<_>>(),
                        "root_attrs": root_attrs.into_iter().map(|(k, v)| (k, Json::String(v))).collect::<serde_json::Map<String, Json>>(),
                        "warnings": warnings,
                    }),
                    Err(e) => serde_json::json!({ "err": e }),
                }
            }
            Err(e) => serde_json::json!({ "err": format!("bad input: {e}") }),
        };
        give(out.to_string().into_bytes())
    }

    pub fn asset<P: Plugin>(input: &[u8]) -> i64 {
        let path = serde_json::from_slice::<Json>(input)
            .ok()
            .and_then(|v| v["path"].as_str().map(String::from))
            .unwrap_or_default();
        let bytes = P::ASSETS
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, b)| b.to_vec())
            .unwrap_or_default();
        give(bytes)
    }

    fn s(v: &Json) -> String {
        v.as_str().unwrap_or_default().to_string()
    }

    fn columns(v: &Json) -> Vec<Column> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .map(|c| Column {
                        name: s(&c["name"]),
                        role: s(&c["role"]),
                        values: c["values"].as_array().cloned().unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn render<P: Plugin>(input: &[u8]) -> i64 {
        let out = match serde_json::from_slice::<Json>(input) {
            Ok(v) => {
                let input = RenderInput {
                    columns: columns(&v["columns"]),
                    width: v["width"].as_u64().unwrap_or(0) as u32,
                    height: v["height"].as_u64().unwrap_or(0) as u32,
                    theme: theme(&v["theme"]),
                };
                match P::render(&input) {
                    Ok(svg) => serde_json::json!({ "ok": svg }),
                    Err(e) => serde_json::json!({ "err": e }),
                }
            }
            Err(e) => serde_json::json!({ "err": format!("bad input: {e}") }),
        };
        give(out.to_string().into_bytes())
    }

    pub fn lint<P: Plugin>(input: &[u8]) -> i64 {
        let cols = serde_json::from_slice::<Json>(input)
            .map(|v| columns(&v["columns"]))
            .unwrap_or_default();
        give(serde_json::json!(P::lint(&cols)).to_string().into_bytes())
    }
}

/// Export plugin `$p` (a type implementing [`Plugin`]) as the module's
/// interface functions.
#[macro_export]
macro_rules! export_plugin {
    ($p:ty) => {
        #[no_mangle]
        pub extern "C" fn anofox_plugin_abi() -> i32 {
            $crate::ABI
        }
        #[no_mangle]
        pub extern "C" fn anofox_plugin_alloc(len: i32) -> i32 {
            $crate::__rt::alloc(len)
        }
        #[no_mangle]
        pub unsafe extern "C" fn anofox_plugin_dealloc(ptr: i32, len: i32) {
            $crate::__rt::dealloc(ptr, len)
        }
        #[no_mangle]
        pub extern "C" fn anofox_plugin_manifest() -> i64 {
            $crate::__rt::manifest::<$p>()
        }
        #[no_mangle]
        pub unsafe extern "C" fn anofox_plugin_render_contract(ptr: i32, len: i32) -> i64 {
            $crate::__rt::render_contract::<$p>(&$crate::__rt::take(ptr, len))
        }
        #[no_mangle]
        pub unsafe extern "C" fn anofox_plugin_asset(ptr: i32, len: i32) -> i64 {
            $crate::__rt::asset::<$p>(&$crate::__rt::take(ptr, len))
        }
        #[no_mangle]
        pub unsafe extern "C" fn anofox_plugin_render(ptr: i32, len: i32) -> i64 {
            $crate::__rt::render::<$p>(&$crate::__rt::take(ptr, len))
        }
        #[no_mangle]
        pub unsafe extern "C" fn anofox_plugin_lint(ptr: i32, len: i32) -> i64 {
            $crate::__rt::lint::<$p>(&$crate::__rt::take(ptr, len))
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_covers_markup() {
        assert_eq!(escape("<a & 'b'>\""), "&lt;a &amp; &#39;b&#39;&gt;&quot;");
    }
}
