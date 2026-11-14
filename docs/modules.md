# Writing a module

A **module** is a feature built in its own crate, public or private, that plugs into anofox-visualization without being part of this repository (#1156). It brings roles (`::MY_PANEL`), the panels they draw, SQL macros, `--check` warnings and browser files.

Modules are linked at build time. There is no runtime plug-in loading. A **distribution** is a small crate that depends on `anofox-visualization` and its modules. It registers the modules once at start-up and builds its own dashboard CLI, server, DuckDB extension and browser bundle. The public builds include no modules.

> **Unstable.** The module interface is `#[doc(hidden)]` and may change without a semver bump until a first real module exists.

`examples/hello_module.rs` is a complete, trivial module (`::HELLO`). The examples below are cut from it.

## The trait

```rust
use anofox_visualization::module::{Module, ModuleRole};
use anofox_visualization::{Column, RenderOptions};

pub struct Hello;

impl Module for Hello {
    /// A short, unique name: the module's id in ::REQUIRES and /modules/<name>/.
    fn name(&self) -> &'static str { "hello" }

    /// The roles it brings. A statement carrying one is drawn by `render`.
    fn roles(&self) -> &'static [ModuleRole] {
        &[ModuleRole { token: "HELLO", aliases: &["GREETING"], summary: "a greeting" }]
    }

    /// The panel's SVG body (inside an <svg> of width × height).
    fn render(&self, cols: &[Column], width: u32, height: u32, o: &RenderOptions)
        -> Result<String, String> { /* … */ }

    // Optional:
    // fn lint(&self, cols: &[Column]) -> Vec<String>          --check warnings (module/lint)
    // fn macros(&self) -> &'static [MacroSpec]                 SQL macros
    // fn render_contract(&self, plot, rows, options, w, h, o)  draws a contract macro's plot
    // fn assets(&self) -> &'static [(&'static str, &'static [u8])]   browser files
}

pub static HELLO: Hello = Hello;
```

Register a module before anything renders:

```rust
anofox_visualization::module::register(&HELLO)?;
```

Registration is refused when the module's name, one of its role tokens or aliases, or one of its macro names is already taken, by a built-in or by another module. A refused module adds nothing.

## Roles and panels

A module's tokens parse to `Role::Module(id)`. They appear in `dashboard --roles`, the role list the browser uses, and `--check`. `SELECT 'world'::HELLO` is drawn by the module's `render`, which returns the SVG body. Escape any text you put in it (`anofox_visualization::format::escape_xml`).

## `::REQUIRES`

A dashboard declares the modules it needs:

```sql
SELECT 'hello'::REQUIRES;          -- or 'hello, maps'
```

The names are read from the string literal before anything runs. A build without the module says so instead of failing on the module's roles as unknown DuckDB types:

| Host | What happens |
|---|---|
| `dashboard` CLI | stops with *This dashboard needs the module 'hello', which this build does not include.* (exit 2) |
| `dashboard --check` | one `module/missing` error, nothing else |
| servers | the dashboard is not served; a start-up line names the file and the module |
| browser | the message instead of the dashboard |

With the module present, `::REQUIRES` is a directive that draws nothing.

## SQL macros and contract plots

A module can bring macros next to the built-in `anofox_plot_*` ones. `anofox_visualization::macros::all()` is the built-in table plus the modules'. The DuckDB extension creates them at LOAD, and the CLI, the servers and the browser plan their calls in the same way. A contract macro (`anofox_plot_<plot>`) builds its body with `contract_body!`, and the module draws the plot in `render_contract`:

```rust
fn macros(&self) -> &'static [MacroSpec] {
    &[MacroSpec {
        name: "anofox_plot_hello",
        params: &["tbl"],
        named: &[("greeting", "'Hello'"), ("theme", "NULL"), ("locale", "NULL"),
                 ("width", "640"), ("height", "200")],
        body: anofox_visualization::contract_body!("hello", "'greeting', greeting"),
        description: "Greets every row's name.",
        example: "SELECT anofox_plot_hello(t) FROM (VALUES ('Ada'), ('Max')) t(name)",
        tags: &[],
    }]
}

fn render_contract(&self, plot: &str, rows: &[Map<String, Value>], options: &Map<String, Value>,
                   width: u32, height: u32, o: &RenderOptions) -> Option<Result<String, String>> {
    if plot != "hello" { return None; }
    // … Some(Ok(svg_body))
}
```

## Browser files

`assets()` returns `(path, bytes)` pairs. The builder server (`serve`) and the extension's servers serve them at `/modules/<name>/<path>`. The page loads each `.js` as a module script and each `.css` as a stylesheet after `app.js`. They come from the same origin, so the page's CSP is unchanged. The static dashboard pages (`serve --dashboards`) serve no assets. A static `web/` build (`dashboard --site`, GitHub Pages) copies the files to `web/modules/<name>/<path>`.

## Building a distribution

Each public binary is a thin wrapper around a library function, so a distribution writes the same wrapper with its modules registered first:

| Build | Distribution `main` / entry point | Example |
|---|---|---|
| dashboard CLI | `register(&M)?; anofox_visualization::cli::dashboard::main()` | `examples/hello_dashboard.rs` |
| server | `register(&M)?; anofox_visualization::cli::serve::main()` (feature `serve`) | `examples/hello_serve.rs` |
| DuckDB extension | its own `<name>_init_c_api`: `register(&M)`, then `anofox_visualization_ext::init_c_api(info, access)` | `duckext/examples/hello_ext.rs` |
| browser (`web/pkg`) | a `#[wasm_bindgen(start)]` that registers; build for wasm32, bind with `wasm-bindgen --out-name anofox_visualization` | `examples/hello_wasm.rs` |

The extension is packaged like the public one (`duckext/scripts/append_extension_metadata.py`) under the distribution's own name. The DuckDB entry point is `<name>_init_c_api`. The C++/FFI community build takes no modules.
