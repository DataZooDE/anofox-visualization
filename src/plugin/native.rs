//! The native plugin host (#1213): wasmi, no imports, fuel and a memory
//! cap per instance, an instance pool. See the parent module for the
//! interface.

use super::{columns, leak, manifest, render_input, ABI, MAX_ASSETS};
use crate::module::{Module, ModuleRole};
use crate::{Column, RenderOptions};
use serde_json::{json, Map, Value as Json};
use std::sync::Mutex;
use wasmi::{
    Config, Engine, Instance, Linker, Memory, Store, StoreLimits, StoreLimitsBuilder, TypedFunc,
};

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
    /// Its manifest, as the browser registers a server-only plugin (#1306):
    /// roles, plots and macros — no code.
    manifest: Json,
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
    let mut browser_manifest = m.clone();
    if let Some(o) = browser_manifest.as_object_mut() {
        o.remove("assets");
    }
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
        manifest: browser_manifest,
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

/// A registered plugin, as the servers hand it to the browser.
#[derive(Clone)]
struct Loaded {
    name: &'static str,
    /// Its WebAssembly, for the page to run — `None` for a server-only
    /// plugin (#1306): the browser never gets it.
    wasm: Option<&'static [u8]>,
    /// Its browser files are served (#1290).
    assets: bool,
    /// A server-only plugin's manifest (the page registers its roles).
    manifest: Option<&'static Json>,
}

/// The registered plugins' own files, for the browser (#1213 step 3).
static LOADED: Mutex<Vec<Loaded>> = Mutex::new(Vec::new());

/// [`register`]; `assets`: with its browser files (see [`load_with`]).
pub fn register_with(wasm: &[u8], limits: Limits, assets: bool) -> Result<&'static str, String> {
    let p = load_with(wasm, limits, assets)?;
    remember(Loaded {
        name: p.name,
        wasm: Some(Box::leak(wasm.to_vec().into_boxed_slice())),
        assets: !p.assets.is_empty(),
        manifest: None,
    });
    crate::module::register(p)?;
    Ok(p.name)
}

/// Load a plugin that stays on the server (#1306): its panels are drawn
/// here and sent as SVG; the browser gets its roles (its manifest) but
/// never its WebAssembly or browser files.
pub fn register_server_only(wasm: &[u8], limits: Limits) -> Result<&'static str, String> {
    let p = load_with(wasm, limits, false)?;
    remember(Loaded {
        name: p.name,
        wasm: None,
        assets: false,
        manifest: Some(&p.manifest),
    });
    crate::module::register(p)?;
    Ok(p.name)
}

/// [`register_server_only`] for the plugin file at `path`.
pub fn register_file_server_only(path: &std::path::Path) -> Result<&'static str, String> {
    let wasm = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    register_server_only(&wasm, Limits::default()).map_err(|e| format!("{}: {e}", path.display()))
}

fn remember(l: Loaded) {
    if let Ok(mut all) = LOADED.lock() {
        all.push(l);
    }
}

/// Is `name` a plugin the server keeps (#1306)?
pub fn is_server_only(name: &str) -> bool {
    LOADED
        .lock()
        .is_ok_and(|l| l.iter().any(|p| p.name == name && p.wasm.is_none()))
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
        super::checked_svg(self.name, &out)
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
    fn contract_answer(
        &self,
        plot: &str,
        rows: &[Map<String, Json>],
        options: &Map<String, Json>,
        width: u32,
        height: u32,
        o: &RenderOptions,
    ) -> Option<Result<crate::module::ContractAnswer, String>> {
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
                .and_then(|out| super::contract_answer(self.name, &out)),
        )
    }
}

/// The roles of a statement that a server-only plugin draws (#1306), as the
/// page's plan gives them (`[[column, "ROLE", name], …]`) — `None` when no
/// role of it belongs to one.
pub fn server_drawn_roles(p: &crate::sql::Panel) -> Option<String> {
    let uses = p.roles.iter().any(|(_, r)| match r {
        crate::Role::Module(id) => crate::module::name_of_role(*id).is_some_and(is_server_only),
        _ => false,
    });
    uses.then(|| {
        Json::from(
            p.roles
                .iter()
                .map(|(i, r)| {
                    let name = p
                        .names
                        .iter()
                        .find(|(j, _)| j == i)
                        .map_or("", |(_, n)| n.as_str());
                    json!([i, r.token(), name])
                })
                .collect::<Vec<_>>(),
        )
        .to_string()
    })
}

/// The browser's list of the registered plugins (`plugins.json`): their
/// files' paths, `modules/<name>/plugin.wasm` — the page runs them in its
/// own sandbox (step 3). Served without `--plugin-assets`: a plugin's
/// WebAssembly can't reach the page; its JS and CSS can. A plugin whose
/// browser files are served (`--plugin-assets`) is listed as
/// `{"url": …, "assets": true}`: the page then loads its manifest's
/// `assets` from `modules/<name>/` like a linked module's (#1290).
pub fn browser_list() -> String {
    let l = LOADED.lock().map(|l| l.clone()).unwrap_or_default();
    Json::from(
        l.iter()
            .map(|p| match (p.wasm, p.manifest) {
                // #1306: roles only — the page asks the server to draw
                (None, Some(m)) => json!({ "name": p.name, "server": true, "manifest": m }),
                _ => {
                    let url = format!("modules/{}/plugin.wasm", p.name);
                    if p.assets {
                        json!({ "url": url, "assets": true })
                    } else {
                        Json::from(url)
                    }
                }
            })
            .collect::<Vec<_>>(),
    )
    .to_string()
}

/// A registered plugin's WebAssembly by its served path
/// (`modules/<name>/plugin.wasm`, a leading `/` ignored).
pub fn browser_wasm(path: &str) -> Option<&'static [u8]> {
    let name = path
        .trim_start_matches('/')
        .strip_prefix("modules/")?
        .strip_suffix("/plugin.wasm")?;
    LOADED.lock().ok()?.iter().find(|p| p.name == name)?.wasm
}
