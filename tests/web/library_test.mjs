// Node test for the sidebar's search, tag chips, recents, and the .sql /
// .zip export and import in web/app.js (sidebar plan, PR 3). The .zip is
// cross-checked with Python's zipfile when python3 is on the PATH (a zip it
// writes uncompressed imports; a deflated one is refused; ours it reads), and
// the exported folder is read with the wasm dashboard_meta — the servers'
// parser. Skips the wasm part without web/pkg.
import { readFileSync, readdirSync, existsSync, mkdtempSync, writeFileSync, rmSync } from "fs";
import { execFileSync } from "child_process";
import { tmpdir } from "os";
import path from "path";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const fn = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const fold = src.match(/^const foldText = [\s\S]*?;$/m);
assert.ok(fold, "foldText not found");
const names = ["slugify", "isHeaderLine", "setHeaderSql", "importName", "pushRecent", "searchTerms", "dashMatches", "tagsInView", "exportFiles", "crc32", "zipStored", "unzipStored"];
const lib = new Function(`${fold[0]}\n${names.map(fn).join("\n")}\nreturn { ${names.join(", ")} };`)();
const { setHeaderSql, importName, pushRecent, searchTerms, dashMatches, tagsInView, exportFiles, zipStored, unzipStored } = lib;
const enc = new TextEncoder();
const dec = new TextDecoder();

// ── header lines: a new @title goes first; it replaces an old one ──
assert.equal(setHeaderSql("-- note\nSELECT 1;", "title", "Sales"), "-- @title Sales\n-- note\nSELECT 1;");
assert.equal(setHeaderSql("-- @group G\n-- @title Old\nSELECT 1;", "title", "New"), "-- @group G\n-- @title New\nSELECT 1;");
assert.equal(setHeaderSql("-- @title T\nSELECT 1;", "tags", "a, b"), "-- @title T\n-- @tags a, b\nSELECT 1;");

// ── names, recents ──
assert.equal(importName("Sales", ["Sales"]), "Sales (2)");
assert.equal(importName("Sales", ["Sales", "Sales (2)"]), "Sales (3)");
assert.equal(importName("New", ["Sales"]), "New");
assert.deepEqual(pushRecent(["a", "b", "c"], "b"), ["b", "a", "c"]);
assert.deepEqual(pushRecent(["a", "b", "c", "d", "e"], "f"), ["f", "a", "b", "c", "d"]);
assert.deepEqual(pushRecent(undefined, "a"), ["a"]);

// ── search: every word, in name / title / group / tags, accents folded ──
const e = { name: "Weekly sales by region", title: "Umsätze je Region", group: "Sales / EMEA", tags: ["weekly", "kpi"] };
assert.deepEqual(searchTerms("  Weekly   EMEA "), ["weekly", "emea"]);
for (const q of ["weekly", "WEEKLY emea", "umsatze", "kpi region", "sales / emea"]) assert.ok(dashMatches(e, searchTerms(q), []), q);
for (const q of ["monthly", "weekly apac"]) assert.ok(!dashMatches(e, searchTerms(q), []), q);
assert.ok(dashMatches(e, [], ["kpi"]));
assert.ok(dashMatches(e, [], ["kpi", "weekly"]));
assert.ok(!dashMatches(e, [], ["kpi", "forecast"]), "all selected tags");
// chips: selected first, then by use, at most 8 (more when more are selected)
const many = [{ tags: ["a", "b"] }, { tags: ["b", "c"] }, { tags: ["b"] }, { tags: ["c", "z"] }];
assert.deepEqual(tagsInView(many, []), ["b", "c", "a", "z"]);
assert.deepEqual(tagsInView(many, ["z"]), ["z", "b", "c", "a"]);
assert.deepEqual(tagsInView(many, [], 2), ["b", "c"]);

// ── export: a served folder ──
const list = [
  { id: "weekly-sales", name: "Weekly sales", sql: "-- @group Sales\n-- @tags kpi\nSELECT 1;" },
  { id: "q3", name: "Q3 'review'", sql: "-- @title stale\nSELECT 2;" },
  { id: "x".repeat(200), name: "Long", sql: "SELECT 3;" },
];
const files = exportFiles(list);
assert.deepEqual(files.map((f) => f.name), ["weekly-sales.sql", "q3.sql", "x".repeat(120) + ".sql"]);
assert.equal(files[0].text, "-- @title Weekly sales\n-- @group Sales\n-- @tags kpi\nSELECT 1;");
assert.equal(files[1].text, "-- @title Q3 'review'\nSELECT 2;");
// every id passes the servers' valid_dashboard_id (src/host/serving.rs)
for (const f of files) assert.match(f.name.replace(/\.sql$/, ""), /^(?!\.)[A-Za-z0-9_.-]{1,128}$/);

// ── zip round trip ──
const zip = zipStored(files.map((f) => ({ name: f.name, data: f.text })));
const back = unzipStored(zip);
assert.deepEqual(back.map((f) => [f.name, dec.decode(f.data)]), files.map((f) => [f.name, f.text]));
assert.deepEqual(unzipStored(zipStored([{ name: "dir/ä.sql", data: "SELECT 'ü';" }])).map((f) => [f.name, dec.decode(f.data)]), [["dir/ä.sql", "SELECT 'ü';"]]);
assert.deepEqual(unzipStored(zipStored([])), []);
// damage is reported, never a crash
assert.throws(() => unzipStored(enc.encode("not a zip at all, sorry")), /not a \.zip/);
const broken = zip.slice();
broken[30 + "weekly-sales.sql".length + 3] ^= 0xff; // inside the first file's data
assert.throws(() => unzipStored(broken), /checksum|damaged/);
assert.throws(() => unzipStored(zip.slice(0, zip.length - 30)), /not a \.zip|damaged/);
// a deflated entry is refused with a hint
const deflated = zip.slice();
const dv = new DataView(deflated.buffer);
const cd = dv.getUint32(deflated.length - 22 + 16, true);
dv.setUint16(cd + 10, 8, true);
assert.throws(() => unzipStored(deflated), /compressed.*\.sql files|store/);

// ── cross-check with Python's zipfile ──
let py = true;
try {
  execFileSync("python3", ["-c", "import zipfile"]);
} catch (_) {
  py = false;
  console.log("python3 not found — skipping the zipfile cross-check");
}
if (py) {
  const dir = mkdtempSync(path.join(tmpdir(), "dp-zip-"));
  try {
    const ours = path.join(dir, "ours.zip");
    writeFileSync(ours, zip);
    const listed = execFileSync("python3", ["-c", "import sys,zipfile; z=zipfile.ZipFile(sys.argv[1]); assert z.testzip() is None; print('\\n'.join(i.filename+'|'+str(i.compress_type) for i in z.infolist()))", ours], { encoding: "utf8" });
    assert.deepEqual(listed.trim().split("\n"), files.map((f) => `${f.name}|0`));
    const mk = (out, method) =>
      execFileSync("python3", ["-c", `import sys,zipfile; z=zipfile.ZipFile(sys.argv[1],'w',${method}); z.writestr('team/a.sql','-- @title A\\nSELECT 1;'); z.writestr('b.sql','SELECT 2;'); z.writestr('team/',''); z.close()`, out]);
    const stored = path.join(dir, "stored.zip");
    mk(stored, "zipfile.ZIP_STORED");
    assert.deepEqual(unzipStored(new Uint8Array(readFileSync(stored))).map((f) => [f.name, dec.decode(f.data)]), [["team/a.sql", "-- @title A\nSELECT 1;"], ["b.sql", "SELECT 2;"]]);
    const defl = path.join(dir, "deflated.zip");
    mk(defl, "zipfile.ZIP_DEFLATED");
    assert.throws(() => unzipStored(new Uint8Array(readFileSync(defl))), /"team\/a\.sql" is compressed/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const pkg = new URL("../../web/pkg/", import.meta.url);
if (!existsSync(new URL("anofox_visualization_bg.wasm", pkg))) {
  console.log("web/pkg not built — skipping the wasm part of library_test");
  process.exit(0);
}
const wasm = await import(new URL("anofox_visualization.js", pkg));
wasm.initSync({ module: readFileSync(new URL("anofox_visualization_bg.wasm", pkg)) });
// The exported folder reads back with the servers' parser: the title is the
// name, @group / @tags are kept — for every example saved under a new name.
const examples = readdirSync(new URL("../../web/examples/", import.meta.url))
  .filter((f) => f.endsWith(".sql"))
  .map((f, i) => ({ id: f.replace(/\.sql$/, ""), name: `Mine ${i}: “quoted” & co`, sql: readFileSync(new URL(`../../web/examples/${f}`, import.meta.url), "utf8") }));
for (const f of exportFiles(examples)) {
  const d = examples.find((x) => `${x.id}.sql` === f.name);
  const before = JSON.parse(wasm.dashboard_meta(d.sql));
  const after = JSON.parse(wasm.dashboard_meta(f.text));
  assert.equal(after.title, d.name, f.name);
  assert.deepEqual([after.group, after.tags], [before.group, before.tags], f.name);
}
console.log("library_test: ok");
