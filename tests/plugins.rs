//! #1213: runtime WebAssembly plugins — the example plugin draws and lints
//! through the module registry; the sandbox holds against plugins that
//! loop, import, overrun their memory, answer garbage or draw script.
#![cfg(feature = "plugins")]

use anofox_visualization::plugin::{self, Limits};
use anofox_visualization::{parse_role, render, Column, Role, Value};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

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

/// The example plugin, built to wasm32 for this test run.
fn hello_wasm() -> &'static [u8] {
    static WASM: OnceLock<Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| build_plugin("examples/plugins/hello/Cargo.toml", "hello_plugin.wasm"))
}

fn hello() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        assert_eq!(
            plugin::register_with(hello_wasm(), Limits::default(), true).unwrap(),
            "hello"
        );
    });
}

#[test]
fn a_plugin_role_parses_draws_and_lints() {
    hello();
    let role = parse_role("HELLO").expect("::HELLO parses");
    assert!(matches!(role, Role::Module(_)));
    assert_eq!(parse_role("greeting"), Some(role), "the alias");
    let col = |n: usize| {
        Column::new(
            "who",
            role,
            (0..n).map(|i| Value::Str(format!("<w{i}>"))).collect(),
        )
    };
    let svg = render(&[col(2)], 320, 120).unwrap();
    assert!(svg.contains(">Hello, &lt;w1&gt;!</text>"), "{svg}");
    // called again: the instance is reused, the fuel refilled
    for _ in 0..3 {
        assert!(render(&[col(1)], 320, 120).is_ok());
    }
    let m = anofox_visualization::module::for_columns(&[col(1)]).unwrap();
    assert!(m.lint(&[col(2)]).is_empty());
    assert_eq!(
        m.lint(&[col(11)]),
        vec!["::HELLO greets 11 rows; more than 10 will not fit"]
    );
}

#[test]
fn a_plugin_error_is_the_panel_error() {
    hello();
    let role = parse_role("HELLO").unwrap();
    let m = anofox_visualization::module::for_columns(&[Column::new("x", role, vec![])]).unwrap();
    let o = anofox_visualization::RenderOptions::default();
    let e = m
        .render(
            &[Column::new("who", role, vec![Value::Float(1.0)])],
            100,
            100,
            &o,
        )
        .unwrap_err();
    assert_eq!(e, "hello: ::HELLO takes text");
}

/// A WebAssembly text plugin around a render function body, with a working
/// alloc and a manifest of `name`.
fn wat_plugin(name: &str, render_body: &str, extra: &str) -> Vec<u8> {
    let manifest = format!(
        r#"{{"name":"{name}","roles":[{{"token":"{}","aliases":[],"summary":""}}]}}"#,
        name.to_uppercase()
    );
    let n = manifest.len();
    let data = manifest.replace('"', "\\22");
    wat::parse_str(format!(
        r#"(module
          {extra}
          (memory (export "memory") 2)
          (global $next (mut i32) (i32.const 4096))
          (data (i32.const 16) "{data}")
          (func (export "anofox_plugin_abi") (result i32) (i32.const 1))
          (func (export "anofox_plugin_alloc") (param $n i32) (result i32)
            (local $p i32)
            (local.set $p (global.get $next))
            (global.set $next (i32.add (global.get $next) (local.get $n)))
            (local.get $p))
          (func (export "anofox_plugin_dealloc") (param i32 i32))
          (func (export "anofox_plugin_manifest") (result i64)
            (i64.or (i64.shl (i64.const 16) (i64.const 32)) (i64.const {n})))
          (func (export "anofox_plugin_render") (param $p i32) (param $n i32) (result i64)
            {render_body}))"#
    ))
    .unwrap()
}

/// Register a WAT plugin and render a panel of its role.
fn draw(name: &str, render_body: &str, limits: Limits) -> Result<String, String> {
    plugin::register(&wat_plugin(name, render_body, ""), limits)?;
    let role = parse_role(&name.to_uppercase()).unwrap();
    let o = anofox_visualization::RenderOptions::default();
    anofox_visualization::module::for_columns(&[Column::new("x", role, vec![])])
        .unwrap()
        .render(
            &[Column::new("x", role, vec![Value::Integer(1)])],
            100,
            100,
            &o,
        )
}

#[test]
fn an_endless_loop_runs_out_of_fuel() {
    let small = Limits {
        fuel: 1_000_000,
        ..Limits::default()
    };
    let e = draw("spin", "(loop $l (br $l)) (i64.const 0)", small).unwrap_err();
    assert!(e.contains("ran out of fuel"), "{e}");
    // and again: a fresh instance, the same answer (no hang, no panic)
    let role = parse_role("SPIN").unwrap();
    assert!(render(&[Column::new("x", role, vec![Value::Integer(1)])], 100, 100).is_err());
}

#[test]
fn a_trap_is_an_error() {
    let e = draw("boom", "unreachable", Limits::default()).unwrap_err();
    assert!(e.contains("trapped"), "{e}");
}

#[test]
fn a_buffer_outside_memory_is_refused() {
    // ptr 0x7fff0000, len 0x100000: past the 2 pages
    let e = draw("wild", "(i64.const 0x7fff000000100000)", Limits::default()).unwrap_err();
    assert!(e.contains("outside its memory"), "{e}");
}

#[test]
fn growing_past_the_memory_cap_fails_inside_the_plugin() {
    // memory.grow by 2048 pages (128 MiB) returns -1 under a 64 MiB cap:
    // the plugin sees the failure; here it then traps
    let body = "(if (i32.lt_s (memory.grow (i32.const 2048)) (i32.const 0)) (then unreachable)) (i64.const 0)";
    let e = draw("hog", body, Limits::default()).unwrap_err();
    assert!(e.contains("trapped"), "{e}");
}

#[test]
fn malformed_json_is_an_error() {
    // answers 4 bytes of the manifest's middle: not JSON
    let e = draw("junk", "(i64.const 0x0000001400000004)", Limits::default()).unwrap_err();
    assert!(e.contains("malformed JSON"), "{e}");
}

/// A plugin that answers `{"ok": svg}` with `svg` stored at offset 1024.
fn answering(name: &str, svg: &str) -> Result<String, String> {
    let body = format!(r#"{{"ok":"{}"}}"#, svg.replace('"', "\\\""));
    let data = body.replace('\\', "\\5c").replace('"', "\\22");
    let n = body.len();
    let manifest = format!(
        r#"{{"name":"{name}","roles":[{{"token":"{}","aliases":[],"summary":""}}]}}"#,
        name.to_uppercase()
    );
    let wat = format!(
        r#"(module
          (memory (export "memory") 2)
          (global $next (mut i32) (i32.const 8192))
          (data (i32.const 16) "{m}")
          (data (i32.const 1024) "{data}")
          (func (export "anofox_plugin_abi") (result i32) (i32.const 1))
          (func (export "anofox_plugin_alloc") (param $n i32) (result i32)
            (local $p i32)
            (local.set $p (global.get $next))
            (global.set $next (i32.add (global.get $next) (local.get $n)))
            (local.get $p))
          (func (export "anofox_plugin_dealloc") (param i32 i32))
          (func (export "anofox_plugin_manifest") (result i64)
            (i64.or (i64.shl (i64.const 16) (i64.const 32)) (i64.const {mn})))
          (func (export "anofox_plugin_render") (param i32 i32) (result i64)
            (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const {n}))))"#,
        m = manifest.replace('"', "\\22"),
        mn = manifest.len(),
    );
    plugin::register(&wat::parse_str(wat).unwrap(), Limits::default())?;
    let role = parse_role(&name.to_uppercase()).unwrap();
    let o = anofox_visualization::RenderOptions::default();
    anofox_visualization::module::for_columns(&[Column::new("x", role, vec![])])
        .unwrap()
        .render(&[Column::new("x", role, vec![])], 100, 100, &o)
}

#[test]
fn a_plugin_svg_may_only_draw() {
    assert_eq!(
        answering("fine", "<rect width='5' height='5'/>").unwrap(),
        "<rect width='5' height='5'/>"
    );
    for (name, svg) in [
        ("script", "<script>alert(1)</script>"),
        ("handler", "<rect onload='alert(1)'/>"),
        ("extlink", "<a href='https://example.com'><rect/></a>"),
        ("foreign", "<foreignObject><div/></foreignObject>"),
        ("unclosed", "<g>"),
    ] {
        let e = answering(name, svg).unwrap_err();
        assert!(
            e.starts_with(&format!("plugin {name}: its SVG")),
            "{name}: {e}"
        );
    }
}

#[test]
fn bad_plugins_are_refused_at_load() {
    let refuse = |wasm: &[u8]| {
        plugin::load(wasm, Limits::default())
            .map(|_| ())
            .unwrap_err()
    };
    assert!(refuse(b"not wasm").contains("not a valid module"));
    // an import of any kind
    let e = refuse(&wat_plugin(
        "net",
        "(i64.const 0)",
        r#"(import "env" "fetch" (func))"#,
    ));
    assert!(e.contains("imports env::fetch"), "{e}");
    // another interface version
    let v2 = wat::parse_str(
        r#"(module (memory (export "memory") 1)
             (func (export "anofox_plugin_abi") (result i32) (i32.const 2)))"#,
    )
    .unwrap();
    assert!(refuse(&v2).contains("speaks plugin interface 2"));
    // a bad name or token
    assert!(refuse(&wat_plugin("Bad Name", "(i64.const 0)", "")).contains("manifest: name"));
}

#[test]
fn a_plugin_cannot_take_a_built_in_role() {
    let e = plugin::register(
        &wat_plugin("barchart", "(i64.const 0)", ""),
        Limits::default(),
    )
    .unwrap_err();
    assert!(e.contains("::BARCHART is a role already"), "{e}");
}

#[test]
fn a_plugin_brings_a_contract_macro_the_host_writes() {
    // step 4: the plugin names the macro's parts; the host writes its SQL
    hello();
    let m = anofox_visualization::macros::all()
        .into_iter()
        .find(|m| m.name == "anofox_plot_hello")
        .expect("the plugin's macro is listed");
    assert_eq!(m.params, ["tbl"]);
    assert!(m.named.contains(&("greeting", "'Hello'")) && m.named.contains(&("height", "200")));
    assert!(
        m.named.contains(&("theme", "NULL")) && m.named.contains(&("width", "640")),
        "the template's own"
    );
    assert!(
        m.body
            .starts_with("anofox_render(json_object('plot', 'hello'"),
        "{}",
        m.body
    );
    assert!(
        m.body
            .contains("'options', json_object('greeting', greeting)"),
        "{}",
        m.body
    );
    // anofox_render(…) of its plot draws through the plugin
    let svg = anofox_visualization::render_spec(
        r#"{"plot": "hello", "rows": [{"name": "<Ada>"}], "options": {"greeting": "Hi"}, "width": 300, "height": 120}"#,
    );
    assert!(svg.contains(">Hi, &lt;Ada&gt;!</text>"), "{svg}");
}

#[test]
fn a_plugin_s_browser_files_load_only_when_allowed() {
    hello();
    assert_eq!(
        anofox_visualization::module::asset("modules/hello/hello.css"),
        Some(&b".hello { font-style: italic; }\n"[..])
    );
    // the same plugin without the operator's leave: none
    use anofox_visualization::module::Module;
    let p = plugin::load(hello_wasm(), Limits::default()).unwrap();
    assert!(p.assets().is_empty());
}

/// A WAT plugin with this manifest (and a render_contract export unless
/// `contract` is false).
fn wat_manifest(manifest: &str, contract: bool) -> Vec<u8> {
    let data = manifest.replace('\\', "\\5c").replace('"', "\\22");
    let n = manifest.len();
    let rc = if contract {
        r#"(func (export "anofox_plugin_render_contract") (param i32 i32) (result i64) (i64.const 0))"#
    } else {
        ""
    };
    wat::parse_str(format!(
        r#"(module
          (memory (export "memory") 2)
          (global $next (mut i32) (i32.const 8192))
          (data (i32.const 16) "{data}")
          (func (export "anofox_plugin_abi") (result i32) (i32.const 1))
          (func (export "anofox_plugin_alloc") (param $n i32) (result i32)
            (local $p i32)
            (local.set $p (global.get $next))
            (global.set $next (i32.add (global.get $next) (local.get $n)))
            (local.get $p))
          (func (export "anofox_plugin_dealloc") (param i32 i32))
          (func (export "anofox_plugin_manifest") (result i64)
            (i64.or (i64.shl (i64.const 16) (i64.const 32)) (i64.const {n})))
          (func (export "anofox_plugin_render") (param i32 i32) (result i64) (i64.const 0))
          {rc})"#
    ))
    .unwrap()
}

#[test]
fn a_plugin_ships_no_sql() {
    let load = |macros: &str, contract: bool| {
        let m = format!(r#"{{"name": "m", "roles": [], "macros": [{macros}]}}"#);
        plugin::load(&wat_manifest(&m, contract), Limits::default()).map(|_| ())
    };
    let ok = r#"{"plot": "p", "named": [["n", "NULL"]], "options": [{"key": "n", "param": "n"}]}"#;
    assert!(load(ok, true).is_ok());
    for (macros, why) in [
        // a default that is SQL, not a literal
        (
            r#"{"plot": "p", "named": [["n", "(SELECT secret FROM t)"]]}"#,
            "must be a literal",
        ),
        (
            r#"{"plot": "p", "named": [["n", "read_text('/etc/passwd')"]]}"#,
            "must be a literal",
        ),
        (
            r#"{"plot": "p", "named": [["n", "getvariable('__claim_user')"]]}"#,
            "must be a literal",
        ),
        (
            r#"{"plot": "p", "named": [["n", "'a' || 'b'"]]}"#,
            "must be a literal",
        ),
        // names written into SQL: identifiers only
        (r#"{"plot": "p'); DROP TABLE t; --"}"#, "macro plot"),
        (r#"{"plot": "p", "named": [["n x", "NULL"]]}"#, "parameter"),
        (
            r#"{"plot": "p", "options": [{"key": "k", "param": "nope"}]}"#,
            "names no parameter",
        ),
        (
            r#"{"plot": "p", "named": [["n", "NULL"]], "options": [{"key": "k", "param": "n", "fallback_var": "__claim_user"}]}"#,
            "fallback variable",
        ),
    ] {
        let e = load(macros, true).unwrap_err();
        assert!(e.contains(why), "{macros}: {e}");
    }
    // macros without the export that draws them
    assert!(load(ok, false)
        .unwrap_err()
        .contains("exports no anofox_plugin_render_contract"));
}

#[test]
fn a_plugin_s_asset_paths_are_checked() {
    for path in [
        "../x.js",
        "/abs.js",
        "a/.hidden.js",
        "x.html",
        "x.exe",
        "a//b.js",
    ] {
        let m = format!(r#"{{"name": "m", "roles": [], "assets": ["{path}"]}}"#);
        let e = plugin::load(&wat_manifest(&m, false), Limits::default())
            .map(|_| ())
            .unwrap_err();
        assert!(e.contains("asset path"), "{path}: {e}");
    }
}

#[test]
fn parallel_calls_use_several_instances() {
    // #1213 step 2: a server renders in parallel; each call gets an
    // instance of its own (or waits for one), and every answer is right.
    hello();
    let role = parse_role("HELLO").unwrap();
    let threads: Vec<_> = (0..8)
        .map(|t| {
            std::thread::spawn(move || {
                for i in 0..10 {
                    let who = format!("t{t}i{i}");
                    let svg = render(
                        &[Column::new("who", role, vec![Value::Str(who.clone())])],
                        200,
                        80,
                    )
                    .unwrap();
                    assert!(svg.contains(&format!(">Hello, {who}!</text>")), "{svg}");
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
}

/// GET `path` from 127.0.0.1:`port`: (status, body), retried while the
/// server starts.
#[cfg(feature = "serve")]
fn get(port: u16, path: &str, tok: Option<&str>) -> (u16, String) {
    use std::io::{Read, Write};
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let r = (|| -> std::io::Result<String> {
            let mut c = std::net::TcpStream::connect(("127.0.0.1", port))?;
            c.set_read_timeout(Some(std::time::Duration::from_secs(60)))?;
            let t = tok
                .map(|t| format!("X-Anofox-Token: {t}\r\n"))
                .unwrap_or_default();
            write!(
                c,
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{t}Connection: close\r\n\r\n"
            )?;
            let mut s = String::new();
            c.read_to_string(&mut s)?;
            Ok(s)
        })();
        match r {
            Ok(s) if !s.is_empty() => {
                let status = s
                    .split(' ')
                    .nth(1)
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0);
                let body = s
                    .split_once("\r\n\r\n")
                    .map(|(_, b)| b.to_string())
                    .unwrap_or_default();
                return (status, body);
            }
            _ if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(200))
            }
            _ => panic!("no answer from the server on {port}"),
        }
    }
}

#[cfg(feature = "serve")]
#[test]
fn serve_loads_a_plugin_and_its_browser_files_only_when_allowed() {
    if std::process::Command::new("duckdb")
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("skipped: no duckdb on PATH");
        return;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("plugin-serve-{}", std::process::id()));
    let dash = dir.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    let wasm = dir.join("hello.wasm");
    std::fs::write(&wasm, hello_wasm()).unwrap();
    std::fs::write(
        dash.join("hi.sql"),
        "-- @title Hi\nSELECT 'hello'::REQUIRES;\nSELECT 'Ada' AS who ::HELLO, 'Greeting'::TITLE;\n",
    )
    .unwrap();
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill(); // our own child, by handle
            let _ = self.0.wait();
        }
    }
    // `--dashboards`: the static pages; else the builder. Both serve the
    // plugin's browser files at /modules/<name>/ with --plugin-assets (#1294)
    let serve = |dashboards: bool, assets: bool| -> (Kill, u16, Option<String>) {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_serve"));
        if dashboards {
            c.arg("--dashboards").arg(&dash);
        } else {
            c.arg(dir.join("b.duckdb"));
        }
        c.arg("--plugin").arg(&wasm);
        if assets {
            c.arg("--plugin-assets");
        }
        c.args(["--port", &port.to_string(), "--no-open"]);
        if dashboards {
            return (Kill(c.spawn().unwrap()), port, None);
        }
        // the builder prints its URL with the session token first
        c.stdout(std::process::Stdio::piped());
        let mut child = c.spawn().unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(child.stdout.take().unwrap()),
            &mut line,
        )
        .unwrap();
        let tok = line.split("token=").nth(1).map(|t| t.trim().to_string());
        assert!(tok.is_some(), "no token in {line:?}");
        (Kill(child), port, tok)
    };
    let (_s, port, _) = serve(true, false);
    let (status, page) = get(port, "/d/hi", None);
    assert_eq!(status, 200, "{page}");
    assert!(
        page.contains("Hello, Ada!"),
        "the plugin draws the panel: {page}"
    );
    // #1294: without --plugin-assets the dashboards pages load none of its
    // browser files, and the server has none
    assert!(!page.contains("/modules/hello/"), "{page}");
    assert_eq!(get(port, "/modules/hello/hello.js", None).0, 404);
    let (status, list) = get(port, "/plugins.json", None);
    assert_eq!(status, 200, "{list}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&list).unwrap(),
        serde_json::json!(["modules/hello/plugin.wasm"])
    );
    // … with it, the page loads them (as the builder does) and they are served
    let (_s2, port, _) = serve(true, true);
    let (status, page) = get(port, "/d/hi", None);
    assert_eq!(status, 200, "{page}");
    assert!(
        page.contains(r#"<script type="module" src="/modules/hello/hello.js"></script>"#)
            && page.contains(r#"<link rel="stylesheet" href="/modules/hello/hello.css">"#),
        "the page loads the plugin's browser files: {page}"
    );
    let (status, js) = get(port, "/modules/hello/hello.js", None);
    assert_eq!(
        (status, js.as_str()),
        (
            200,
            "document.documentElement.dataset.helloPlugin = \"loaded\";\n"
        )
    );
    let (status, list) = get(port, "/plugins.json", None);
    assert_eq!(status, 200, "{list}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&list).unwrap(),
        serde_json::json!([{"url": "modules/hello/plugin.wasm", "assets": true}])
    );
    let (_b, port, tok) = serve(false, true);
    let (status, css) = get(port, "/modules/hello/hello.css", tok.as_deref());
    assert_eq!(
        (status, css.as_str()),
        (200, ".hello { font-style: italic; }\n")
    );
    assert_eq!(get(port, "/modules/hello/hello.js", tok.as_deref()).0, 200);
    // #1290: plugins.json tells the page to load the plugin's browser files
    let (status, list) = get(port, "/plugins.json", tok.as_deref());
    assert_eq!(status, 200, "{list}");
    let list: serde_json::Value = serde_json::from_str(&list).unwrap();
    assert_eq!(
        list,
        serde_json::json!([{"url": "modules/hello/plugin.wasm", "assets": true}])
    );
    let (_b2, port2, tok2) = serve(false, false);
    assert_eq!(
        get(port2, "/modules/hello/hello.css", tok2.as_deref()).0,
        404,
        "no browser files without --plugin-assets"
    );
    let (status, list) = get(port2, "/plugins.json", tok2.as_deref());
    assert_eq!(status, 200, "{list}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&list).unwrap(),
        serde_json::json!(["modules/hello/plugin.wasm"]),
        "without --plugin-assets the page loads none"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_plugin_can_answer_with_columns_the_host_draws() {
    // #1218: hello_bars answers with ::XAXIS + ::BARCHART columns; the host
    // draws them as its own bar chart (its renderer, the viewer's theme)
    hello();
    let svg = anofox_visualization::render_spec(
        r#"{"plot": "hello_bars", "rows": [{"name": "Ada"}, {"name": "Maximilian"}], "width": 400, "height": 200}"#,
    );
    assert!(
        svg.contains("data-hello=\"bars\""),
        "the root attribute: {svg}"
    );
    assert!(
        svg.contains("data-x=\"Ada\"") && svg.contains("data-x=\"Maximilian\""),
        "{svg}"
    );
    assert!(
        svg.contains("data-value=\"10\""),
        "Maximilian's bar is 10 long: {svg}"
    );
}

#[test]
fn a_columns_answer_needs_known_roles() {
    use anofox_visualization::module::ContractAnswer;
    let bad = serde_json::json!({"columns": [{"name": "x", "role": "NOT_A_ROLE", "values": [1]}]});
    let e = anofox_visualization::plugin::contract_answer_for_test("p", &bad)
        .err()
        .unwrap();
    assert!(e.contains("no known role"), "{e}");
    let ok = serde_json::json!({"columns": [{"name": "x", "role": "XAXIS", "values": ["a"]}, {"name": "y", "role": "BARCHART", "values": [2]}],
                                "root_attrs": {"data-ok": "1", "onload": "x", "data-BAD": "y"}});
    match anofox_visualization::plugin::contract_answer_for_test("p", &ok).unwrap() {
        ContractAnswer::Columns {
            cols, root_attrs, ..
        } => {
            assert_eq!(cols.len(), 2);
            assert_eq!(
                root_attrs,
                vec![("data-ok".to_string(), "1".to_string())],
                "only data-… attributes"
            );
        }
        _ => panic!("columns"),
    }
}

/// #1218: factor graphs are a plugin (modules/factor-graph), built and
/// registered once.
fn factor_graph() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let wasm = build_plugin(
            "modules/factor-graph/Cargo.toml",
            "factor_graph_plugin.wasm",
        );
        assert_eq!(
            plugin::register_with(&wasm, Limits::default(), true).unwrap(),
            "factor-graph"
        );
    });
}

fn factor_graph_svg(rows: serde_json::Value, options: serde_json::Value) -> String {
    factor_graph();
    anofox_visualization::render_spec(
        &serde_json::json!({"plot": "factor_graph", "rows": rows, "options": options,
                            "width": 900, "height": 420})
        .to_string(),
    )
}

#[test]
fn the_factor_graph_plugin_draws_through_the_hosts_diagram() {
    // shapes, an undirected edge with the interface as its label, θ folded
    // into its equality node, the observation a terminal square (#1172)
    let node = |id: &str, kind: &str, label: &str, layer: &str| serde_json::json!({"id": id, "kind": kind, "label": label, "layer": layer});
    let edge =
        |v: &str, f: &str, i: &str| serde_json::json!({"variable": v, "factor": f, "interface": i});
    let svg = factor_graph_svg(
        serde_json::json!([
            node("v0", "data", "y_{0}", "obs"),
            node("v1", "variable", "\\theta", "param"),
            node("e1", "equality", "=", "param"),
            node("f0", "factor", "Beta", "param"),
            node("f1", "factor", "Bernoulli", "obs"),
            node("v2", "variable", "z", "param"),
        ]),
        serde_json::json!({"edges": [
            edge("v0", "f1", "out"),
            edge("e1", "f1", "p"),
            edge("v1", "e1", ""),
            edge("e1", "f0", "out"),
        ]}),
    );
    for want in ["data-diagram", "dp-node-circle", "dp-node-terminal", ">θ<"] {
        assert!(svg.contains(want), "{want}: {svg}");
    }
    assert!(!svg.contains("<polygon"), "{svg}");
}

#[test]
fn a_long_factor_graph_collapses_unless_all_slices_are_asked_for() {
    // #1152: data-slices="collapsed"; slices := 'all' draws every slice
    let mut rows = Vec::new();
    let mut edges = Vec::new();
    for t in 0..8 {
        rows.push(
            serde_json::json!({"id": format!("s{t}"), "kind": "variable",
            "label": format!("s_{{{t}}}"), "layer": "state", "slice": t}),
        );
    }
    for t in 1..8 {
        rows.push(serde_json::json!({"id": format!("f{t}"), "kind": "factor",
            "label": "Normal", "layer": "state", "slice": t}));
        edges.push(serde_json::json!({"variable": format!("s{}", t - 1), "factor": format!("f{t}"), "interface": "mean"}));
        edges.push(serde_json::json!({"variable": format!("s{t}"), "factor": format!("f{t}"), "interface": "out"}));
    }
    let rows = serde_json::Value::Array(rows);
    let svg = factor_graph_svg(rows.clone(), serde_json::json!({"edges": edges}));
    assert!(
        svg.contains("data-slices=\"collapsed\"") && svg.contains("more"),
        "{svg}"
    );
    let svg = factor_graph_svg(rows, serde_json::json!({"edges": edges, "slices": "all"}));
    assert!(
        svg.contains("data-slices=\"all\"") && !svg.contains('⋯'),
        "{svg}"
    );
}

#[test]
fn a_factor_graph_needs_node_ids() {
    let svg = factor_graph_svg(
        serde_json::json!([{"what": 1, "ever": 2}]),
        serde_json::json!({}),
    );
    assert!(svg.contains("an id column"), "{svg}");
}

/// #1275: Bayesian networks are a plugin (modules/bayes-net), built and
/// registered once.
fn bayes_net_svg(rows: serde_json::Value, options: serde_json::Value) -> String {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let wasm = build_plugin("modules/bayes-net/Cargo.toml", "bayes_net_plugin.wasm");
        assert_eq!(
            plugin::register_with(&wasm, Limits::default(), true).unwrap(),
            "bayes-net"
        );
    });
    anofox_visualization::render_spec(
        &serde_json::json!({"plot": "bayes_net", "rows": rows, "options": options,
                            "width": 900, "height": 480})
        .to_string(),
    )
}

#[test]
fn the_bayes_net_plugin_draws_through_the_hosts_diagram() {
    // directed top to bottom, a bar per state, the observed node's state,
    // the weak arc dashed with its strength on hover, the card's data
    let node = |id: &str, label: &str, kind: &str| serde_json::json!({"id": id, "label": label, "kind": kind, "states": ["yes", "no"]});
    let b = |n: &str, s: &str, p: f64| serde_json::json!({"node": n, "state": s, "probability": p, "prior": 0.5});
    let svg = bayes_net_svg(
        serde_json::json!([
            node("rain", "Rain", "chance"),
            node("sprinkler", "Sprinkler", "chance"),
            node("grass", "Wet grass", "observed"),
        ]),
        serde_json::json!({
            "arcs": [{"from": "rain", "to": "grass", "strength": 0.95},
                     {"from": "sprinkler", "to": "grass", "strength": 0.3}],
            "beliefs": [b("rain", "yes", 0.7), b("rain", "no", 0.3),
                        b("sprinkler", "yes", 0.4), b("sprinkler", "no", 0.6),
                        b("grass", "yes", 1.0), b("grass", "no", 0.0)],
            "evidence": [{"node": "grass", "state": "yes"}],
            "tables": [{"node": "grass", "parent_states": ["yes", "yes"], "state": "yes", "probability": 0.99}],
        }),
    );
    for want in [
        "data-diagram=\"TB\"",
        "dp-node-state-bar",
        ">70%<",
        "data-part=\"arrow\"",
        "Sprinkler → Wet grass: strength 0.3",
        "stroke-dasharray",
        "data-bn=",
        // #1276: what the evidence did leads the sentence
        "data-summary=\"Given Wet grass, Rain rose from 50% to 70% and Sprinkler fell from 50% to 40%. Bayesian network: 3 nodes, 2 arcs; evidence: Wet grass = yes;",
    ] {
        assert!(svg.contains(want), "{want}: {svg}");
    }
}

/// #1276: interactive evidence through the host — the change since the
/// last click after each bar, an intervention's cut arc and hand, the
/// active paths on the root, what the evidence did under the graph.
#[test]
fn a_bayes_net_draws_evidence_changes_do_and_paths() {
    let node = |id: &str, label: &str| serde_json::json!({"id": id, "label": label, "kind": "chance", "states": ["yes", "no"]});
    let b = |n: &str, s: &str, p: f64, q: f64, was: f64| serde_json::json!({"node": n, "state": s, "probability": p, "prior": q, "previous": was});
    let svg = bayes_net_svg(
        serde_json::json!([
            node("cloudy", "Cloudy"),
            node("sprinkler", "Sprinkler on"),
            node("rain", "Rain")
        ]),
        serde_json::json!({
            "arcs": [{"from": "cloudy", "to": "sprinkler"}, {"from": "cloudy", "to": "rain"}],
            "beliefs": [b("cloudy", "yes", 0.5, 0.5, 0.17), b("cloudy", "no", 0.5, 0.5, 0.83),
                        b("sprinkler", "yes", 1.0, 0.3, 1.0), b("sprinkler", "no", 0.0, 0.7, 0.0),
                        b("rain", "yes", 0.5, 0.5, 0.3), b("rain", "no", 0.5, 0.5, 0.7)],
            // the selection's variable as it is: JSON text
            "evidence": r#"[{"node": "sprinkler", "state": "yes", "mode": "do"}]"#,
            "paths": [{"path_id": 1, "seq": 1, "from": "cloudy", "to": "rain"}],
        }),
    );
    for want in [
        ">▲33<",
        "dp-node-state-delta",
        "data-cut=\"1\"",
        "dp-edge-cut",
        "dp-node-icon",
        "data-bn-paths=\"[{&quot;id&quot;:&quot;1&quot;,&quot;steps&quot;:[[&quot;cloudy&quot;,&quot;rain&quot;]]}]\"",
        "data-summary=\"Given Sprinkler on set to yes, no belief moved by more than 2%. The last change moved Cloudy most: from 17% to 50%.",
    ] {
        assert!(svg.contains(want), "{want}: {svg}");
    }
}

/// #1278: the causal views through the host — an adjustment set (X, Y, a
/// ring, a blocked back-door path, the legend), a twin network (lanes, the
/// story across the worlds), a structure diff (the distance on the root).
#[test]
fn a_bayes_net_draws_adjustment_sets_twins_and_structure_diffs() {
    let node = |id: &str, label: &str| serde_json::json!({"id": id, "label": label, "kind": "chance", "states": ["yes", "no"]});
    let nodes = serde_json::json!([
        node("z", "Engaged"),
        node("x", "Discount"),
        node("y", "Renewed")
    ]);
    let arcs = serde_json::json!([{"from": "z", "to": "x"}, {"from": "z", "to": "y"}, {"from": "x", "to": "y"}]);
    let svg = bayes_net_svg(
        nodes.clone(),
        serde_json::json!({
            "arcs": arcs,
            "adjust": [{"treatment": "x", "outcome": "y", "set_id": "minimal", "node": "z", "kind": "minimal"}],
        }),
    );
    for want in [
        "dp-node-ring",
        ">X<",
        ">Y<",
        "dp-edge-block",
        ">back-door path, blocked<",
        ">causal path<",
        "Discount ← Engaged → Renewed: blocked at Engaged",
    ] {
        assert!(svg.contains(want), "{want}: {svg}");
    }
    // the twin: beliefs in two worlds, a do on the treatment
    let b = |n: &str, p: f64, w: &str| {
        [
            serde_json::json!({"node": n, "state": "yes", "probability": p, "world": w}),
            serde_json::json!({"node": n, "state": "no", "probability": 1.0 - p, "world": w}),
        ]
    };
    let mut beliefs = Vec::new();
    for (n, f, c) in [("z", 0.8, 0.8), ("x", 1.0, 0.0), ("y", 1.0, 0.8)] {
        beliefs.extend(b(n, f, "factual"));
        beliefs.extend(b(n, c, "counterfactual"));
    }
    let svg = bayes_net_svg(
        nodes.clone(),
        serde_json::json!({
            "arcs": arcs,
            "beliefs": beliefs,
            "evidence": [{"node": "x", "state": "yes"}, {"node": "y", "state": "yes"},
                         {"node": "x", "state": "no", "mode": "do"}],
        }),
    );
    for want in [
        "data-lane=\"Factual world\"",
        "data-lane=\"Counterfactual world\"",
        "data-lane=\"Shared by both worlds\"",
        "If Discount had been set to no: Renewed yes 80% (as it happened 100%)",
        "data-cut=\"1\"",
    ] {
        assert!(svg.contains(want), "{want}: {svg}");
    }
    // the diff: one reversed, one missing
    let svg = bayes_net_svg(
        nodes,
        serde_json::json!({
            "arcs": [{"from": "x", "to": "z", "strength": 0.9}, {"from": "x", "to": "y", "strength": 0.8}],
            "reference": arcs,
        }),
    );
    for want in [
        "data-bn-shd=\"2\"",
        ">reversed (1)<",
        ">missing (1)<",
        ">matching (1)<",
        ">structural Hamming distance 2<",
    ] {
        assert!(svg.contains(want), "{want}: {svg}");
    }
}

#[test]
fn a_bayes_net_needs_node_ids() {
    let svg = bayes_net_svg(serde_json::json!([{"what": 1}]), serde_json::json!({}));
    assert!(svg.contains("an id column"), "{svg}");
}

/// #1275: `--check` names what a plugin's contract plot warns about (a
/// cycle, beliefs that don't sum to 1) as `module/lint`.
#[test]
fn a_bayes_net_lints_through_check() {
    bayes_net_svg(serde_json::json!([{"id": "a"}]), serde_json::json!({}));
    let run = |sql: &str| -> Result<Vec<serde_json::Map<String, serde_json::Value>>, String> {
        let mut row = serde_json::Map::new();
        if sql.contains("anofox_plot_bayes_net") || sql.contains("to_json(list(") {
            row.insert(
                "c0".into(),
                serde_json::json!([{"id": "a", "states": ["yes", "no"]}, {"id": "b", "states": ["yes", "no"]}]),
            );
            row.insert(
                "c1".into(),
                serde_json::json!({"arcs": [{"from": "a", "to": "b"}, {"from": "b", "to": "a"}],
                                   "beliefs": [{"node": "a", "state": "yes", "probability": 0.5}]}),
            );
        }
        Ok(vec![row])
    };
    let d = anofox_visualization::lint::check(
        "SELECT 'bayes-net'::REQUIRES;\nSELECT anofox_plot_bayes_net(n, arcs := [{'from': 'a', 'to': 'b'}]) FROM n;",
        run,
    );
    let w: Vec<&str> = d
        .iter()
        .filter(|x| x.code == "module/lint")
        .map(|x| x.message.as_str())
        .collect();
    assert_eq!(w.len(), 2, "{d:?}");
    assert!(
        w[0].starts_with("anofox_plot_bayes_net: bayes_net/cycle: the arcs a → b → a"),
        "{w:?}"
    );
    assert!(w[1].contains("bayes_net/beliefs-sum"), "{w:?}");
}

/// #1279: an influence diagram — the decision's policy as a table in its
/// box, information arcs dotted, a utility's expected value in its diamond,
/// what to decide in the screen-reader text.
#[test]
fn a_bayes_net_draws_an_influence_diagrams_policy() {
    let svg = bayes_net_svg(
        serde_json::json!([
            {"id": "seismic", "label": "Seismic", "kind": "chance", "states": ["closed", "diffuse"]},
            {"id": "drill", "label": "Drill?", "kind": "decision", "states": ["drill", "walk away"]},
            {"id": "payoff", "label": "Payoff", "kind": "utility"}
        ]),
        serde_json::json!({
            "arcs": [{"from": "seismic", "to": "drill"}, {"from": "drill", "to": "payoff"}],
            "beliefs": [{"node": "seismic", "state": "closed", "probability": 0.0},
                        {"node": "seismic", "state": "diffuse", "probability": 1.0},
                        {"node": "payoff", "expected_utility": -10}],
            "evidence": [{"node": "seismic", "state": "diffuse"}],
            "policy": [
                {"decision": "drill", "parent_states": ["closed"], "option": "drill", "expected_utility": 87.5, "chosen": true},
                {"decision": "drill", "parent_states": ["closed"], "option": "walk away", "expected_utility": 0, "chosen": false},
                {"decision": "drill", "parent_states": ["diffuse"], "option": "drill", "expected_utility": -30.5, "chosen": false},
                {"decision": "drill", "parent_states": ["diffuse"], "option": "walk away", "expected_utility": 0, "chosen": true}
            ],
        }),
    );
    for want in [
        "dp-node-table-chosen",
        "dp-node-table-current",
        ">87.5<",
        ">-30.5<",
        "data-state=\"walk away\"",
        ">-10<",
        "decide Drill?: walk away (expected utility 0) given Seismic = diffuse",
        "known when deciding Drill?",
    ] {
        assert!(svg.contains(want), "{want}: {svg}");
    }
}
