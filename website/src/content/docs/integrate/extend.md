---
title: Your own features — modules and plugins
description: Add your own roles, panels and checks — built into your own distribution, or as a sandboxed WebAssembly plugin loaded at run time.
---

Not every feature belongs in the public build. A **module** brings your own `::ROLE`s, the panels they draw and the `--check` warnings for them — from a separate crate, public or private. Dashboards that need one say so:

```sql
SELECT 'hello'::REQUIRES;
SELECT 'Ada' AS who ::HELLO, 'A greeting'::TITLE;
```

A build without the module stops with *This dashboard needs the module 'hello'* instead of failing on an unknown role.

## Two ways to deliver one

| | Module (linked) | Plugin (WebAssembly) |
|---|---|---|
| What it is | a Rust crate in your own distribution build | one `.wasm` file, loaded at run time |
| How it's loaded | built in: your own `dashboard`, `serve`, DuckDB extension and browser bundle | `dashboard --plugin hello.wasm`, `serve --plugin hello.wasm` |
| Can bring | roles, panels, `--check` warnings, SQL macros, contract plots, browser files | roles, panels, `--check` warnings, contract macros (the host writes their SQL), browser files (only where the operator allows them: they run in the page) |
| Runs | with the rights of the program | in a sandbox: no files, network, clock or randomness; a CPU budget per call; 64 MiB of memory |
| Licence of the kit | — | the plugin SDK is MIT OR Apache-2.0: ship your plugin under any licence |
| Status | available, interface may change | new: the `dashboard` CLI, `serve` (builds with the `plugins` feature) and the browser builder (`plugins.json`); the DuckDB extension follows ([#1213](https://github.com/DataZooDE/anofox-visualization/issues/1213)) |

## A plugin in a few lines

A plugin is a small Rust crate built for `wasm32-unknown-unknown` with the SDK (`crates/anofox-viz-plugin`):

```rust
use anofox_viz_plugin::{escape, export_plugin, Manifest, Plugin, RenderInput, Role};

struct Hello;

impl Plugin for Hello {
    const MANIFEST: Manifest = Manifest {
        name: "hello",
        roles: &[Role { token: "HELLO", aliases: &["GREETING"], summary: "a greeting per row" }],
    };
    fn render(input: &RenderInput) -> Result<String, String> {
        let who = input.role("HELLO").ok_or("no ::HELLO column")?;
        Ok(format!("<text x=\"16\" y=\"32\" fill=\"{}\">Hello, {}!</text>",
                   escape(&input.theme.accent), escape(who.str(0).unwrap_or("world"))))
    }
}

export_plugin!(Hello);
```

```sh
cargo build --release --target wasm32-unknown-unknown --manifest-path examples/plugins/hello/Cargo.toml
dashboard --plugin examples/plugins/hello/target/wasm32-unknown-unknown/release/hello_plugin.wasm --svg hello.sql
```

`render` gets the statement's columns, the panel's size and the theme's colours, and returns the panel's SVG. The SVG may only draw — script, event handlers and external links are refused — and a plugin that crashes, loops or runs out of memory only turns its own panel into an error.

The full guide — the trait for linked modules, `::REQUIRES` in every host, macros, browser files, the plugin interface: [Writing a module or a plugin](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/modules.md).
