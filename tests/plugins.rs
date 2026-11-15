//! #1213: runtime WebAssembly plugins — the example plugin draws and lints
//! through the module registry; the sandbox holds against plugins that
//! loop, import, overrun their memory, answer garbage or draw script.
#![cfg(feature = "plugins")]

use anofox_visualization::plugin::{self, Limits};
use anofox_visualization::{parse_role, render, Column, Role, Value};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// The example plugin, built to wasm32 for this test run.
fn hello_wasm() -> &'static [u8] {
    static WASM: OnceLock<Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("plugin-hello");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = Command::new(cargo)
            .args([
                "build",
                "--release",
                "--target",
                "wasm32-unknown-unknown",
                "--manifest-path",
            ])
            .arg(root.join("examples/plugins/hello/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .status()
            .expect("cargo runs");
        assert!(
            status.success(),
            "building the example plugin (needs `rustup target add wasm32-unknown-unknown`)"
        );
        std::fs::read(target.join("wasm32-unknown-unknown/release/hello_plugin.wasm")).unwrap()
    })
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
    // `--dashboards`: the static pages (no module files there); else the
    // builder, which serves them at /modules/<name>/
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
    let (_b, port, tok) = serve(false, true);
    let (status, css) = get(port, "/modules/hello/hello.css", tok.as_deref());
    assert_eq!(
        (status, css.as_str()),
        (200, ".hello { font-style: italic; }\n")
    );
    let (_b2, port2, tok2) = serve(false, false);
    assert_eq!(
        get(port2, "/modules/hello/hello.css", tok2.as_deref()).0,
        404,
        "no browser files without --plugin-assets"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
