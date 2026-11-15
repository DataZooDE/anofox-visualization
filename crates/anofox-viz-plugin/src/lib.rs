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

    pub fn manifest(m: &Manifest) -> i64 {
        let roles: Vec<Json> = m
            .roles
            .iter()
            .map(|r| serde_json::json!({ "token": r.token, "aliases": r.aliases, "summary": r.summary }))
            .collect();
        give(
            serde_json::json!({ "name": m.name, "roles": roles })
                .to_string()
                .into_bytes(),
        )
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
                let t = &v["theme"];
                let input = RenderInput {
                    columns: columns(&v["columns"]),
                    width: v["width"].as_u64().unwrap_or(0) as u32,
                    height: v["height"].as_u64().unwrap_or(0) as u32,
                    theme: Theme {
                        dark: t["dark"].as_bool().unwrap_or(false),
                        palette: t["palette"]
                            .as_array()
                            .map(|a| a.iter().map(s).collect())
                            .unwrap_or_default(),
                        ink: s(&t["ink"]),
                        muted: s(&t["muted"]),
                        card: s(&t["card"]),
                        accent: s(&t["accent"]),
                    },
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
            $crate::__rt::manifest(&<$p as $crate::Plugin>::MANIFEST)
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
