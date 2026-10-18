// Node test for the build version in the sidebar footer (#216): app.js shows
// the wasm's version() as is, and version() is the build string —
// "v2026.10.18" for a release tag build, "v2026.10.18+37 · 739aba4" (commits
// since the tag · short sha, "-dirty" with uncommitted changes) otherwise,
// "v<Cargo.toml version>" without git. On CI it must equal what git describe
// says about this checkout.
import { readFileSync, existsSync } from "fs";
import { execFileSync } from "child_process";
import { fileURLToPath } from "url";
import assert from "assert/strict";

const root = new URL("../../", import.meta.url);
const src = readFileSync(new URL("web/app.js", root), "utf8");
const html = readFileSync(new URL("web/index.html", root), "utf8");
assert.match(html, /<span class="side-ver" id="side-ver"><\/span>/);
assert.match(src, /sideVer\.textContent = wasmVersion\(\);/, "app.js must show version() as is");
assert.ok(!/"v" \+ wasmVersion\(\)/.test(src), "app.js must not prefix another v");

const pkg = new URL("web/pkg/anofox_visualization.js", root);
const wasm = new URL("web/pkg/anofox_visualization_bg.wasm", root);
assert.ok(existsSync(pkg) && existsSync(wasm), "web/pkg missing: run wasm-pack build first");
const m = await import(pkg.href);
m.initSync({ module: readFileSync(wasm) });
const v = m.version();
assert.match(v, /^v\d{4}\.\d{1,2}\.\d{1,2}(\+\d+ · [0-9a-f]{7,40}(-dirty)?)?$/, v);

if (process.env.CI) {
  // The same rule as src/build_version.rs, against this checkout.
  const cargo = readFileSync(new URL("Cargo.toml", root), "utf8").match(/^version = "(.*)"/m)[1];
  let want = "v" + cargo;
  try {
    const d = execFileSync("git", ["describe", "--tags", "--match", "v*", "--long", "--dirty", "--abbrev=7"],
      { cwd: fileURLToPath(root), encoding: "utf8" }).trim();
    const p = d.match(/^(v.+)-(\d+)-g([0-9a-f]+)(-dirty)?$/);
    if (p) want = p[2] === "0" && !p[4] ? p[1] : `${p[1]}+${p[2]} · ${p[3]}${p[4] || ""}`;
  } catch { /* no git or no tag: the Cargo.toml version */ }
  // npm install may touch tracked files after the wasm build: compare without "-dirty".
  const clean = (s) => s.replace(/-dirty$/, "");
  assert.equal(clean(v), clean(want));
}

console.log(`version_test: ok (${v})`);
