// #1213 step 3: runtime plugins in the browser. A plugin is one .wasm file
// (the anofox-viz-plugin SDK); the page lists them in plugins.json (an
// array of same-origin URLs, written by `serve --plugin` and by a static
// site's build). Each is compiled here with NO imports — a module that
// imports anything is refused — and its manifest registered in the core
// (`register_plugin`), which checks it, writes its macros and holds its SVG
// to the drawing-only allow-list exactly as the servers do. The core calls
// back through globalThis.anofoxPlugins.call(name, kind, input).
//
// Unlike the servers, the browser has no fuel metering: a plugin that loops
// stalls this tab (it still reaches nothing outside its own memory). Memory
// is checked after every call (64 MiB, as natively).

const ABI = 1;
const MAX_MEMORY = 64 << 20;
const EXPORTS = {
  render: "anofox_plugin_render",
  lint: "anofox_plugin_lint",
  render_contract: "anofox_plugin_render_contract",
};

const plugins = new Map(); // name → { module, instance }
const enc = new TextEncoder();
const dec = new TextDecoder();

async function instantiate(module) {
  // no imports given: none may be needed (checked before this)
  return WebAssembly.instantiate(module, {});
}

function needs(inst, name, kind) {
  const f = inst.exports[name];
  if (typeof f !== "function") throw new Error(`exports no ${name}`);
  return f;
}

// (ptr, len) packed into one i64 (a BigInt here), checked against memory.
function unpack(inst, packed) {
  const p = BigInt.asUintN(64, BigInt(packed));
  const ptr = Number(p >> 32n);
  const len = Number(p & 0xffffffffn);
  if (ptr + len > inst.exports.memory.buffer.byteLength) throw new Error("returned a buffer outside its memory");
  return [ptr, len];
}

// Copy `input` in, call `fn`, take the bytes it returns (and free them).
function callBytes(inst, fn, input) {
  const bytes = enc.encode(input);
  const ptr = needs(inst, "anofox_plugin_alloc")(bytes.length) >>> 0;
  const mem = inst.exports.memory;
  if (ptr + bytes.length > mem.buffer.byteLength) throw new Error("alloc returned a buffer outside its memory");
  new Uint8Array(mem.buffer, ptr, bytes.length).set(bytes);
  const [at, n] = unpack(inst, fn(ptr, bytes.length));
  const out = dec.decode(new Uint8Array(mem.buffer, at, n).slice());
  needs(inst, "anofox_plugin_dealloc")(at, n);
  if (mem.buffer.byteLength > MAX_MEMORY) throw new Error("grew past its memory cap");
  return out;
}

// #1306: the role tokens of the plugins the server keeps — a panel carrying
// one is drawn there (the page asks /api/panel for its picture).
const serverTokens = new Set();
export function serverRoleTokens() {
  return serverTokens;
}

globalThis.anofoxPlugins = {
  // The core's drawing call: the plugin's answer (JSON text), or a throw.
  call(name, kind, input) {
    const p = plugins.get(name);
    if (!p) throw new Error("not loaded");
    // #1306: its panels come drawn from the server; nothing runs here
    if (p.server) {
      if (kind === "lint") return "[]";
      throw new Error("drawn on the server");
    }
    const exp = EXPORTS[kind];
    if (!exp) throw new Error(`unknown call ${kind}`);
    try {
      const f = p.instance.exports[exp];
      if (typeof f !== "function") {
        if (kind === "lint") return "[]";
        throw new Error(`exports no ${exp}`);
      }
      return callBytes(p.instance, f, input);
    } catch (e) {
      // a trap or a broken answer: a fresh instance for the next call
      p.stale = true;
      throw new Error(e && e.message ? e.message : String(e));
    } finally {
      if (p.stale) {
        p.stale = false;
        instantiate(p.module).then((r) => (p.instance = r)).catch(() => plugins.delete(name));
      }
    }
  },
};

// A module's or plugin's browser files (paths, relative to the page): each
// .css as a stylesheet, each .js/.mjs as a module script (#1156 D5a).
// `headers` (an embedded or shared page's session, #1284): such a page has
// no cookie a <link>/<script src> could send, so each file is fetched with
// them and put inline — the page's CSP takes inline styles and a script
// carrying its nonce.
export function loadAssetFiles(paths, headers = {}) {
  if (Object.keys(headers).length) {
    for (const p of paths) {
      if (typeof p !== "string" || !/\.(css|m?js)$/i.test(p)) continue;
      // the session goes to this site only
      let url;
      try {
        url = new URL(p, document.baseURI);
      } catch (_) {
        continue;
      }
      if (url.origin !== location.origin) continue;
      inlineAssetFile(url.href, headers).catch((e) => console.warn(`${p}: ${e.message || e}`));
    }
    return;
  }
  for (const p of paths) {
    if (typeof p !== "string") continue;
    if (/\.css$/i.test(p)) {
      const l = document.createElement("link");
      l.rel = "stylesheet";
      l.href = p;
      document.head.appendChild(l);
    } else if (/\.m?js$/i.test(p)) {
      const sc = document.createElement("script");
      sc.type = "module";
      sc.src = p;
      document.head.appendChild(sc);
    }
  }
}

// A request carrying the page session follows no redirect (the session and
// what is put inline stay with this site).
const noRedirect = (headers) => (Object.keys(headers).length ? "error" : "follow");

async function inlineAssetFile(p, headers) {
  const res = await fetch(p, { credentials: "same-origin", headers, redirect: "error" });
  if (!res.ok) throw new Error(`${res.status}`);
  const css = /\.css$/i.test(p);
  const el = document.createElement(css ? "style" : "script");
  if (!css) {
    el.type = "module";
    el.nonce = document.querySelector("script[nonce]")?.nonce || "";
  }
  el.dataset.src = p;
  el.textContent = await res.text();
  document.head.appendChild(el);
}

// Load one plugin from `url`: its name and manifest, or throws why not.
async function loadOne(url, register, headers) {
  const res = await fetch(url, { credentials: "same-origin", headers, redirect: noRedirect(headers) });
  if (!res.ok) throw new Error(`${res.status}`);
  const module = await WebAssembly.compile(await res.arrayBuffer());
  const imports = WebAssembly.Module.imports(module);
  if (imports.length) throw new Error(`imports ${imports[0].module}::${imports[0].name} — a plugin may import nothing`);
  const instance = await instantiate(module);
  const abi = needs(instance, "anofox_plugin_abi")();
  if (abi !== ABI) throw new Error(`speaks plugin interface ${abi}, this build ${ABI}`);
  for (const f of ["anofox_plugin_alloc", "anofox_plugin_dealloc", "anofox_plugin_render", "anofox_plugin_manifest"]) needs(instance, f);
  if (!(instance.exports.memory instanceof WebAssembly.Memory)) throw new Error("exports no memory");
  const [at, n] = unpack(instance, needs(instance, "anofox_plugin_manifest")());
  const manifest = dec.decode(new Uint8Array(instance.exports.memory.buffer, at, n).slice());
  let name = "";
  let assets = [];
  try {
    const m = JSON.parse(manifest);
    name = m.name;
    if (Array.isArray(m.assets)) assets = m.assets;
  } catch (_) {
    throw new Error("manifest: malformed JSON");
  }
  if (plugins.has(name)) throw new Error(`plugin ${name} is loaded already`);
  plugins.set(name, { module, instance });
  const why = register(manifest);
  if (why) {
    plugins.delete(name);
    throw new Error(why);
  }
  return { name, assets };
}

// The plugins plugins.json lists (same origin only), before the page reads
// its roles. No plugins.json (404) means none. An entry is the plugin's
// URL, or {url, assets: true} when the server serves its browser files
// (the operator's --plugin-assets / options.plugin_assets, #1290): then its
// manifest's `assets` load from modules/<name>/, like a linked module's.
// `register`: the core's register_plugin. `headers`: sent with every
// request (an embedded or shared page's session: plugin files need a
// credential, #1284). Returns the names loaded; a refused plugin is reported
// on the console and skipped.
export async function loadPlugins(register, listUrl = "plugins.json", headers = {}) {
  let urls = [];
  try {
    const res = await fetch(listUrl, { credentials: "same-origin", cache: "no-cache", headers, redirect: noRedirect(headers) });
    if (!res.ok) return [];
    urls = await res.json();
  } catch (_) {
    return [];
  }
  if (!Array.isArray(urls)) return [];
  const loaded = [];
  for (const entry of urls) {
    // #1306: a plugin the server keeps — its roles only, no code
    if (entry && typeof entry === "object" && entry.server === true && typeof entry.name === "string") {
      try {
        const why = register(JSON.stringify(entry.manifest || {}));
        if (why) throw new Error(why);
        plugins.set(entry.name, { server: true });
        for (const r of (entry.manifest && entry.manifest.roles) || []) {
          for (const t of [r.token, ...(r.aliases || [])]) if (typeof t === "string") serverTokens.add(t.toUpperCase());
        }
        loaded.push(entry.name);
      } catch (e) {
        console.warn(`plugin ${entry.name}: ${e.message || e}`);
      }
      continue;
    }
    const u = typeof entry === "string" ? entry : entry && typeof entry.url === "string" ? entry.url : null;
    if (u === null) continue;
    const withAssets = typeof entry === "object" && entry.assets === true;
    let url;
    try {
      url = new URL(u, document.baseURI);
    } catch (_) {
      continue;
    }
    if (url.origin !== location.origin) {
      console.warn(`plugin ${u}: not from this site, skipped`);
      continue;
    }
    try {
      const { name, assets } = await loadOne(url.href, register, headers);
      loaded.push(name);
      if (withAssets && assets.length) {
        const base = new URL(`modules/${encodeURIComponent(name)}/`, document.baseURI);
        const files = [];
        for (const a of assets) {
          if (typeof a !== "string") continue;
          const f = new URL(a, base);
          // the core checked the paths (no "..", no scheme); stay under base anyway
          if (f.origin === location.origin && f.href.startsWith(base.href)) files.push(f.href);
        }
        loadAssetFiles(files, headers);
      }
    } catch (e) {
      console.warn(`plugin ${u}: ${e.message || e}`);
    }
  }
  return loaded;
}
