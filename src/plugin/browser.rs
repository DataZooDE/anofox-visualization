//! The browser's plugin host (#1213 step 3): the page (`web/plugins.js`)
//! instantiates a plugin's WebAssembly with no imports and registers its
//! manifest here; drawing calls go back to the page through
//! `globalThis.anofoxPlugins.call(name, kind, input)`. The manifest is
//! checked, the macros written and the SVG held to the allow-list exactly
//! as natively. The browser has no fuel metering: a plugin that loops
//! stalls this tab (it still can't reach anything outside its memory).

use super::{checked_svg, columns, manifest, render_input, ABI};
use crate::module::{Module, ModuleRole};
use crate::{Column, RenderOptions};
use serde_json::{json, Map, Value as Json};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// `kind`: `render` | `lint` | `render_contract`; the plugin's answer,
    /// or a thrown error.
    #[wasm_bindgen(catch, js_namespace = anofoxPlugins, js_name = call)]
    fn js_call(name: &str, kind: &str, input: &str) -> Result<String, JsValue>;
}

/// A plugin the page runs.
struct JsPlugin {
    name: &'static str,
    roles: &'static [ModuleRole],
    macros: &'static [crate::macros::MacroSpec],
    plots: Vec<&'static str>,
}

impl JsPlugin {
    fn call(&self, kind: &str, input: &Json) -> Result<Json, String> {
        let out = js_call(self.name, kind, &input.to_string()).map_err(|e| {
            format!(
                "plugin {}: {}",
                self.name,
                e.as_string().unwrap_or_else(|| "failed".into())
            )
        })?;
        serde_json::from_str(&out)
            .map_err(|e| format!("plugin {}: answered malformed JSON ({e})", self.name))
    }
}

impl Module for JsPlugin {
    fn name(&self) -> &'static str {
        self.name
    }
    fn roles(&self) -> &'static [ModuleRole] {
        self.roles
    }
    fn render(
        &self,
        cols: &[Column],
        width: u32,
        height: u32,
        o: &RenderOptions,
    ) -> Result<String, String> {
        let out = self.call("render", &render_input(cols, width, height, o))?;
        checked_svg(self.name, &out)
    }
    fn lint(&self, cols: &[Column]) -> Vec<String> {
        match self.call("lint", &json!({ "abi": ABI, "columns": columns(cols) })) {
            Ok(Json::Array(a)) => a
                .iter()
                .take(50)
                .filter_map(Json::as_str)
                .map(|s| s.chars().take(500).collect())
                .collect(),
            Ok(_) => vec![format!("plugin {}: lint answered no list", self.name)],
            Err(e) => vec![e],
        }
    }
    fn macros(&self) -> &'static [crate::macros::MacroSpec] {
        self.macros
    }
    fn render_contract(
        &self,
        plot: &str,
        rows: &[Map<String, Json>],
        options: &Map<String, Json>,
        width: u32,
        height: u32,
        o: &RenderOptions,
    ) -> Option<Result<String, String>> {
        if !self.plots.contains(&plot) {
            return None;
        }
        let mut input = render_input(&[], width, height, o);
        input["plot"] = json!(plot);
        input["rows"] = json!(rows);
        input["options"] = json!(options);
        if let Some(m) = input.as_object_mut() {
            m.remove("columns");
        }
        Some(
            self.call("render_contract", &input)
                .and_then(|out| checked_svg(self.name, &out)),
        )
    }
}

/// Register the plugin whose manifest the page read (its `abi` already
/// checked there): its name, or the reason it was refused. Its browser
/// files are the page's business (`/modules/<name>/`), not registered here.
pub fn register_manifest(manifest_json: &str) -> Result<&'static str, String> {
    let m: Json = serde_json::from_str(manifest_json)
        .map_err(|e| format!("manifest: malformed JSON ({e})"))?;
    let man = manifest(&m)?;
    let p: &'static JsPlugin = Box::leak(Box::new(JsPlugin {
        name: man.name,
        roles: man.roles,
        macros: man.macros,
        plots: man.plots,
    }));
    crate::module::register(p)?;
    Ok(p.name)
}
