//! A distribution's browser build (#1156): the app's wasm bundle with its
//! modules registered at start-up. Built for wasm32 and bound with
//! wasm-bindgen, it is the app's `web/pkg` with `::HELLO` in it:
//!
//!     cargo build --example hello_wasm --target wasm32-unknown-unknown \
//!         --no-default-features --features wasm --profile wasm
//!     wasm-bindgen --target web --out-dir web/pkg --out-name anofox_visualization \
//!         target/wasm32-unknown-unknown/wasm/examples/hello_wasm.wasm
#[path = "hello_module.rs"]
mod hello_module;

use wasm_bindgen::prelude::wasm_bindgen;

/// Runs when the bundle is initialised, before the app plans anything.
#[wasm_bindgen(start)]
pub fn start() {
    let _ = anofox_visualization::module::register(&hello_module::HELLO);
}
