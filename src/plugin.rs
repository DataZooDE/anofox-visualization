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

use crate::module::{Module, ModuleRole};
use crate::{Column, RenderOptions};
use serde_json::{json, Map, Value as Json};
use std::sync::Mutex;
use wasmi::{
    Config, Engine, Instance, Linker, Memory, Store, StoreLimits, StoreLimitsBuilder, TypedFunc,
};

/// The interface version this host speaks.
pub const ABI: i32 = 1;

/// What one plugin may use.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Fuel per call (about one unit per WebAssembly instruction).
    pub fuel: u64,
    /// The largest the plugin's memory may grow, in bytes.
    pub memory: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            fuel: 2_000_000_000,
            memory: 64 << 20,
        }
    }
}

/// The plugin's file is at most this large.
const MAX_FILE: usize = 32 << 20;

/// A loaded plugin: its manifest and its (lazily re-made) instance.
pub struct Plugin {
    name: &'static str,
    roles: &'static [ModuleRole],
    macros: &'static [crate::macros::MacroSpec],
    plots: Vec<&'static str>,
    assets: &'static [(&'static str, &'static [u8])],
    engine: Engine,
    module: wasmi::Module,
    limits: Limits,
    /// Instances, made when first needed: a server's parallel requests use
    /// several (#1213 step 2), each with its own memory and fuel.
    pool: Vec<Mutex<Option<Live>>>,
    next: std::sync::atomic::AtomicUsize,
}

/// At most this many instances of one plugin (each up to its memory cap).
const MAX_INSTANCES: usize = 4;

struct Live {
    store: Store<StoreLimits>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    dealloc: TypedFunc<(i32, i32), ()>,
    render: TypedFunc<(i32, i32), i64>,
    lint: Option<TypedFunc<(i32, i32), i64>>,
    render_contract: Option<TypedFunc<(i32, i32), i64>>,
    asset: Option<TypedFunc<(i32, i32), i64>>,
}

impl Plugin {
    fn instantiate(&self) -> Result<(Live, Instance), String> {
        instantiate(&self.engine, &self.module, self.limits)
    }

    /// Call `f` on the live instance (made fresh when there is none); on any
    /// error the instance is dropped, so a trap never leaves a broken one.
    fn with_live<R>(&self, f: impl FnOnce(&mut Live) -> Result<R, String>) -> Result<R, String> {
        // a free instance if there is one, else wait for the next in turn
        let mut slot = match self.pool.iter().find_map(|m| m.try_lock().ok()) {
            Some(s) => s,
            None => {
                let i = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.pool[i % self.pool.len()]
                    .lock()
                    .map_err(|_| "plugin lock poisoned")?
            }
        };
        if slot.is_none() {
            *slot = Some(self.instantiate()?.0);
        }
        let live = slot.as_mut().expect("just made");
        let r = f(live);
        if r.is_err() {
            *slot = None;
        }
        r
    }

    fn call_json(&self, which: Call, input: &Json) -> Result<Json, String> {
        let bytes = serde_json::to_vec(input).map_err(|e| e.to_string())?;
        let name = self.name;
        let fuel = self.limits.fuel;
        self.with_live(|live| {
            refuel(live, fuel)?;
            let f = match which {
                Call::Render => live.render,
                Call::Lint => match live.lint {
                    Some(f) => f,
                    None => return Ok(Json::Array(vec![])),
                },
                Call::RenderContract => live
                    .render_contract
                    .ok_or("exports no anofox_plugin_render_contract")?,
            };
            let out = call_bytes(live, f, &bytes).map_err(|e| format!("plugin {name}: {e}"))?;
            serde_json::from_slice(&out)
                .map_err(|e| format!("plugin {name}: answered malformed JSON ({e})"))
        })
    }
}

#[derive(Clone, Copy)]
enum Call {
    Render,
    Lint,
    RenderContract,
}

fn instantiate(
    engine: &Engine,
    module: &wasmi::Module,
    limits: Limits,
) -> Result<(Live, Instance), String> {
    let mut store = Store::new(
        engine,
        StoreLimitsBuilder::new()
            .memory_size(limits.memory)
            .memories(1)
            .tables(1)
            .instances(1)
            .build(),
    );
    store.limiter(|l| l);
    store.set_fuel(limits.fuel).map_err(|e| e.to_string())?;
    // no imports: an empty linker
    let linker = Linker::<StoreLimits>::new(engine);
    let instance = linker
        .instantiate_and_start(&mut store, module)
        .map_err(|e| format!("instantiate: {e}"))?;
    // the interface version first: another version's exports may differ
    let abi = instance
        .get_typed_func::<(), i32>(&store, "anofox_plugin_abi")
        .map_err(|_| "exports no anofox_plugin_abi")?
        .call(&mut store, ())
        .map_err(trap)?;
    if abi != ABI {
        return Err(format!("speaks plugin interface {abi}, this build {ABI}"));
    }
    let memory = instance
        .get_memory(&store, "memory")
        .ok_or("exports no memory")?;
    let func = |name: &str| format!("exports no {name} of the right type");
    let alloc = instance
        .get_typed_func::<i32, i32>(&store, "anofox_plugin_alloc")
        .map_err(|_| func("anofox_plugin_alloc"))?;
    let dealloc = instance
        .get_typed_func::<(i32, i32), ()>(&store, "anofox_plugin_dealloc")
        .map_err(|_| func("anofox_plugin_dealloc"))?;
    let render = instance
        .get_typed_func::<(i32, i32), i64>(&store, "anofox_plugin_render")
        .map_err(|_| func("anofox_plugin_render"))?;
    let lint = instance
        .get_typed_func::<(i32, i32), i64>(&store, "anofox_plugin_lint")
        .ok();
    let render_contract = instance
        .get_typed_func::<(i32, i32), i64>(&store, "anofox_plugin_render_contract")
        .ok();
    let asset = instance
        .get_typed_func::<(i32, i32), i64>(&store, "anofox_plugin_asset")
        .ok();
    Ok((
        Live {
            store,
            memory,
            alloc,
            dealloc,
            render,
            lint,
            render_contract,
            asset,
        },
        instance,
    ))
}

/// A packed `ptr << 32 | len` buffer, checked against the memory.
fn unpack(live: &Live, packed: i64) -> Result<(usize, usize), String> {
    let p = packed as u64;
    let (ptr, len) = ((p >> 32) as usize, (p & 0xFFFF_FFFF) as usize);
    let size = live.memory.data_size(&live.store);
    if ptr.checked_add(len).is_none_or(|end| end > size) {
        return Err("returned a buffer outside its memory".into());
    }
    Ok((ptr, len))
}

fn refuel(live: &mut Live, fuel: u64) -> Result<(), String> {
    live.store.set_fuel(fuel).map_err(|e| e.to_string())
}

fn trap(e: wasmi::Error) -> String {
    if e.as_trap_code() == Some(wasmi::TrapCode::OutOfFuel) {
        "ran out of fuel (too much work for one call)".into()
    } else {
        format!("trapped: {e}")
    }
}

/// Copy `input` into the plugin, call `f`, and take the bytes it returns.
fn call_bytes(
    live: &mut Live,
    f: TypedFunc<(i32, i32), i64>,
    input: &[u8],
) -> Result<Vec<u8>, String> {
    let len = i32::try_from(input.len()).map_err(|_| "input too large")?;
    let ptr = live.alloc.call(&mut live.store, len).map_err(trap)?;
    let start = usize::try_from(ptr).map_err(|_| "alloc returned a negative pointer")?;
    live.memory
        .write(&mut live.store, start, input)
        .map_err(|_| "alloc returned a buffer outside its memory")?;
    let packed = f.call(&mut live.store, (ptr, len)).map_err(trap)?;
    let (at, n) = unpack(live, packed)?;
    let out = live.memory.data(&live.store)[at..at + n].to_vec();
    live.dealloc
        .call(&mut live.store, (at as i32, n as i32))
        .map_err(trap)?;
    Ok(out)
}

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

/// Load a plugin from its bytes (not registered yet; see [`register`]),
/// without its browser files.
pub fn load(wasm: &[u8], limits: Limits) -> Result<&'static Plugin, String> {
    load_with(wasm, limits, false)
}

/// [`load`]; `assets`: also take its browser files — they run in the page,
/// outside the sandbox, so only when the operator allows it.
pub fn load_with(wasm: &[u8], limits: Limits, assets: bool) -> Result<&'static Plugin, String> {
    if wasm.len() > MAX_FILE {
        return Err(format!("larger than {} MiB", MAX_FILE >> 20));
    }
    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    let module =
        wasmi::Module::new(&engine, wasm).map_err(|e| format!("not a valid module: {e}"))?;
    if let Some(i) = module.imports().next() {
        return Err(format!(
            "imports {}::{} — a plugin may import nothing",
            i.module(),
            i.name()
        ));
    }
    let (mut live, instance) = instantiate(&engine, &module, limits)?;
    let man = instance
        .get_typed_func::<(), i64>(&live.store, "anofox_plugin_manifest")
        .map_err(|_| "exports no anofox_plugin_manifest")?
        .call(&mut live.store, ())
        .map_err(trap)?;
    let (at, n) = unpack(&live, man)?;
    let m: Json = serde_json::from_slice(&live.memory.data(&live.store)[at..at + n])
        .map_err(|e| format!("manifest: malformed JSON ({e})"))?;
    let man = manifest(&m)?;
    if !man.macros.is_empty() && live.render_contract.is_none() {
        return Err("declares macros but exports no anofox_plugin_render_contract".into());
    }
    let mut files: Vec<(&'static str, &'static [u8])> = Vec::new();
    if assets && !man.asset_paths.is_empty() {
        let f = live
            .asset
            .ok_or("declares assets but exports no anofox_plugin_asset")?;
        let mut total = 0usize;
        for p in &man.asset_paths {
            refuel(&mut live, limits.fuel)?;
            let bytes = call_bytes(&mut live, f, json!({ "path": p }).to_string().as_bytes())
                .map_err(|e| format!("asset {p}: {e}"))?;
            total += bytes.len();
            if total > MAX_ASSETS {
                return Err(format!("assets larger than {} MiB", MAX_ASSETS >> 20));
            }
            files.push((leak(p), Box::leak(bytes.into_boxed_slice())));
        }
    }
    refuel(&mut live, limits.fuel)?;
    let (name, roles) = (man.name, man.roles);
    Ok(Box::leak(Box::new(Plugin {
        name,
        roles,
        macros: man.macros,
        plots: man.plots,
        assets: Box::leak(files.into_boxed_slice()),
        engine,
        module,
        limits,
        pool: {
            let n = std::thread::available_parallelism()
                .map_or(1, |n| n.get())
                .clamp(1, MAX_INSTANCES);
            let mut pool = vec![Mutex::new(Some(live))];
            pool.extend((1..n).map(|_| Mutex::new(None)));
            pool
        },
        next: std::sync::atomic::AtomicUsize::new(0),
    })))
}

/// Load a plugin and register it as a module (without its browser files).
pub fn register(wasm: &[u8], limits: Limits) -> Result<&'static str, String> {
    register_with(wasm, limits, false)
}

/// [`register`]; `assets`: with its browser files (see [`load_with`]).
pub fn register_with(wasm: &[u8], limits: Limits, assets: bool) -> Result<&'static str, String> {
    let p = load_with(wasm, limits, assets)?;
    crate::module::register(p)?;
    Ok(p.name)
}

/// Load the plugin file at `path` and register it (without its browser files).
pub fn register_file(path: &std::path::Path) -> Result<&'static str, String> {
    register_file_with(path, false)
}

/// [`register_file`]; `assets`: with its browser files (see [`load_with`]).
pub fn register_file_with(path: &std::path::Path, assets: bool) -> Result<&'static str, String> {
    let wasm = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    register_with(&wasm, Limits::default(), assets).map_err(|e| format!("{}: {e}", path.display()))
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

impl Module for Plugin {
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
        let out = self.call_json(Call::Render, &render_input(cols, width, height, o))?;
        self.checked_svg(&out)
    }
    fn lint(&self, cols: &[Column]) -> Vec<String> {
        let input = json!({ "abi": ABI, "columns": columns(cols) });
        match self.call_json(Call::Lint, &input) {
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
    fn assets(&self) -> &'static [(&'static str, &'static [u8])] {
        self.assets
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
            self.call_json(Call::RenderContract, &input)
                .and_then(|out| self.checked_svg(&out)),
        )
    }
}

impl Plugin {
    /// `{"ok": svg}` → the SVG, held to the drawing-only allow-list.
    fn checked_svg(&self, out: &Json) -> Result<String, String> {
        match (&out["ok"], &out["err"]) {
            (Json::String(svg), _) => {
                // The body goes into the page: only drawing markup, never
                // script, handlers or external references (the allow-list
                // `--site` checks published SVGs with).
                let doc = format!(
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" \
                     xmlns:xlink=\"http://www.w3.org/1999/xlink\">{svg}</svg>"
                );
                match crate::cli::dashboard::svg_active_reason(doc.as_bytes()) {
                    None => Ok(svg.clone()),
                    Some(why) => Err(format!("plugin {}: its SVG {why}", self.name)),
                }
            }
            (_, Json::String(e)) => Err(format!("{}: {e}", self.name)),
            _ => Err(format!("plugin {}: answered neither ok nor err", self.name)),
        }
    }
}
