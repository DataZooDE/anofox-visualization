//! #1306: a plugin that stays on the server — the browser gets its roles,
//! never its WebAssembly or browser files; its panels are drawn natively.
//! (A process of its own: a plugin's name is registered once per process.)
#![cfg(feature = "plugins")]

use anofox_visualization::plugin::{self, Limits};
use std::path::PathBuf;
use std::process::Command;

/// A plugin crate built to wasm32 for this test run.
fn build_plugin(manifest: &str, wasm: &str) -> Vec<u8> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("plugin-target");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "--manifest-path",
        ])
        .arg(root.join(manifest))
        .arg("--target-dir")
        .arg(&target)
        .status()
        .expect("cargo runs");
    assert!(
        status.success(),
        "building {manifest} (needs `rustup target add wasm32-unknown-unknown`)"
    );
    std::fs::read(target.join("wasm32-unknown-unknown/release").join(wasm)).unwrap()
}

#[test]
fn a_server_only_plugin_never_reaches_the_browser() {
    let wasm = build_plugin("examples/plugins/hello/Cargo.toml", "hello_plugin.wasm");
    assert_eq!(
        plugin::register_server_only(&wasm, Limits::default()).unwrap(),
        "hello"
    );
    assert!(plugin::is_server_only("hello"));
    // plugins.json: its name and manifest (roles), no file to fetch
    let list: serde_json::Value = serde_json::from_str(&plugin::browser_list()).unwrap();
    let e = &list[0];
    assert_eq!(e["server"], true, "{list}");
    assert_eq!(e["name"], "hello");
    assert!(e.get("url").is_none(), "{list}");
    assert!(
        e["manifest"]["roles"]
            .as_array()
            .is_some_and(|r| !r.is_empty()),
        "{list}"
    );
    assert!(e["manifest"].get("assets").is_none(), "{list}");
    // nothing of it is served
    assert!(plugin::browser_wasm("/modules/hello/plugin.wasm").is_none());
    assert!(anofox_visualization::module::asset("/modules/hello/hello.js").is_none());
    // a panel with its role is drawn here, as the page would draw it
    let plan = anofox_visualization::sql::plan("SELECT 'Ada' AS who ::HELLO, 'Greeting'::TITLE;");
    let roles = plugin::server_drawn_roles(&plan[0]).expect("drawn on the server");
    let rows = r#"[{"c0": "Ada", "c1": "Greeting"}]"#;
    let svg = anofox_visualization::host::render_panel_rows(rows, &roles, 640, 320, "").unwrap();
    assert!(svg.contains("Hello, Ada!"), "{svg}");
    // a statement without its roles isn't
    let other = anofox_visualization::sql::plan("SELECT 1::METRIC;");
    assert!(plugin::server_drawn_roles(&other[0]).is_none());
}
