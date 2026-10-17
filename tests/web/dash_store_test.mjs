// Node test for "My dashboards" in web/app.js (sidebar plan, PR 2): the store
// behind the storage interface (list / get / put / remove + the sidebar
// state), the v1 → v2 migration (with its backup), the `-- @group` rewrite
// (checked against the wasm dashboard_meta — the servers' parser), names for
// copies, group order and sorting. Skips the wasm part without web/pkg
// (`wasm-pack build --target web --out-dir web/pkg --no-default-features
// --features wasm`).
import { readFileSync, readdirSync, existsSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (re, what) => {
  const m = src.match(re);
  assert.ok(m, `${what} not found in app.js`);
  return m[0];
};
const fn = (name) => grab(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"), `function ${name}`);
const line = (name) => grab(new RegExp(`^const ${name} = .*$`, "m"), `const ${name}`);
const lib = new Function(
  [
    ...["DASH_KEY", "DASH_V1_KEY", "DASH_SORTS"].map(line),
    ...["slugify", "newDashId", "migrateDashStore", "makeDashStorage", "isHeaderLine", "setHeaderSql", "setGroupSql", "copyName", "dashGroupList", "sortDashes", "chromeStmt", "setChromeSql", "withTitleSql", "sqlToSave"].map(fn),
    "return { DASH_KEY, DASH_V1_KEY, newDashId, migrateDashStore, makeDashStorage, isHeaderLine, setGroupSql, copyName, dashGroupList, sortDashes, withTitleSql, sqlToSave };",
  ].join("\n")
)();
const { DASH_KEY, DASH_V1_KEY, migrateDashStore, makeDashStorage, setGroupSql, copyName, dashGroupList, sortDashes, withTitleSql } = lib;

// A localStorage stand-in.
const memory = (init = {}) => {
  const m = new Map(Object.entries(init));
  return { getItem: (k) => (m.has(k) ? m.get(k) : null), setItem: (k, v) => m.set(k, String(v)), m };
};

// ── the @group rewrite ──
assert.equal(setGroupSql("SELECT 1;", "Sales"), "-- @group Sales\nSELECT 1;");
assert.equal(setGroupSql("-- @title T\n-- about\nSELECT 1;", "Sales"), "-- @title T\n-- @group Sales\n-- about\nSELECT 1;");
assert.equal(setGroupSql("-- @title T\n-- @group  Old \nSELECT 1;", "New"), "-- @title T\n-- @group New\nSELECT 1;");
assert.equal(setGroupSql("-- @group A\n-- @group B\nSELECT 1;", "C"), "-- @group C\nSELECT 1;", "one line is left");
assert.equal(setGroupSql("-- @title T\n-- @group A\nSELECT 1;", ""), "-- @title T\nSELECT 1;");
assert.equal(setGroupSql("SELECT 1;", ""), "SELECT 1;");
assert.equal(setGroupSql("-- @groups x\n-- @group\nSELECT 1;", "G"), "-- @groups x\n-- @group G\nSELECT 1;", "only real @group lines");
assert.equal(setGroupSql("SELECT 1;\n-- @title late", "G"), "-- @group G\nSELECT 1;\n-- @title late", "a @title after SQL isn't a header");
assert.equal(setGroupSql("SELECT 1;", "a\nb"), "-- @group a b\nSELECT 1;", "no line breaks get in");

// ── names, groups, sorting ──
assert.equal(copyName("Sales", ["Sales"]), "Sales copy");
assert.equal(copyName("Sales", ["Sales", "Sales copy", "Sales copy 2"]), "Sales copy 3");
assert.deepEqual(dashGroupList(["B", "A"], ["", "C", "A", "Z", "C"]), ["B", "A", "C", "Z"]);
const ds = [
  { id: "a", name: "beta", updated: 1 },
  { id: "b", name: "Alpha 10", updated: 3 },
  { id: "c", name: "alpha 9", updated: 2 },
];
assert.deepEqual(sortDashes(ds, "manual").map((d) => d.id), ["a", "b", "c"]);
assert.deepEqual(sortDashes(ds, "name").map((d) => d.id), ["c", "b", "a"]);
assert.deepEqual(sortDashes(ds, "updated").map((d) => d.id), ["b", "c", "a"]);
// one name: ::DASHBOARD_TITLE follows when the SQL has one
assert.equal(withTitleSql("SELECT 'Old'::DASHBOARD_TITLE;\nSELECT 1;", "New"), "SELECT 'New'::DASHBOARD_TITLE;\nSELECT 1;");
assert.equal(withTitleSql("SELECT 1;", "New"), "SELECT 1;", "no title statement: none is added");

// ── migration v1 → v2 ──
const v1 = {
  items: {
    "Weekly sales": { sql: "SELECT 1;", group: "Sales" },
    "weekly sales!": { sql: "-- @title W\nSELECT 2;", group: "Sales" },
    Draft: { sql: "SELECT 3;", group: "" },
    Orphan: { sql: "SELECT 4;", group: "Gone" },
    Broken: { group: "Sales" },
  },
  groups: ["Ops", "Sales"],
  collapsed: { Ops: true },
};
const m = migrateDashStore(v1, 42);
assert.equal(m.v, 2);
assert.deepEqual(m.order, ["weekly-sales", "weekly-sales-2", "draft", "orphan"], "slug ids, unique; unreadable items dropped");
assert.deepEqual(m.items["weekly-sales"], { name: "Weekly sales", sql: "-- @group Sales\nSELECT 1;", updated: 42 });
assert.equal(m.items["weekly-sales-2"].sql, "-- @title W\n-- @group Sales\nSELECT 2;");
assert.equal(m.items.draft.sql, "SELECT 3;");
assert.equal(m.items.orphan.sql, "SELECT 4;", "a group v1 didn't list was shown ungrouped");
assert.deepEqual([m.groups, m.collapsed, m.sort], [["Ops", "Sales"], { Ops: true }, "manual"]);
// the older flat {name: sql}
assert.deepEqual(migrateDashStore({ A: "SELECT 1;", b: 5 }, 1).items, { a: { name: "A", sql: "SELECT 1;", updated: 1 } });
// nothing / junk → an empty v2 store
for (const junk of [null, 7, "x", [], { items: [] }]) {
  const e = migrateDashStore(junk, 0);
  assert.deepEqual([e.v, e.order, e.groups, e.sort], [2, [], [], "manual"], JSON.stringify(junk));
}
// a v2 store is cleaned up: order lists each item once, a bad sort is manual
const v2 = migrateDashStore({ v: 2, items: { a: { name: "A", sql: "s" }, b: { name: "B", sql: "t" }, c: { sql: 1 } }, order: ["b", "zz", "b"], sort: "?" }, 0);
assert.deepEqual([v2.order, v2.sort, Object.keys(v2.items)], [["b", "a"], "manual", ["a", "b"]]);

// ── the storage interface ──
let t = 100;
const ls = memory({ [DASH_KEY]: JSON.stringify(v1) });
const db = makeDashStorage(ls, () => ++t);
assert.deepEqual(db.list().map((d) => d.id), ["weekly-sales", "weekly-sales-2", "draft", "orphan"]);
assert.deepEqual(JSON.parse(ls.getItem(DASH_V1_KEY)), v1, "the v1 store is kept as a backup");
assert.equal(JSON.parse(ls.getItem(DASH_KEY)).v, 2, "migrated once, written back");
// a second migration never overwrites the backup
ls.setItem(DASH_KEY, JSON.stringify({ items: { X: { sql: "SELECT 9;", group: "" } } }));
assert.deepEqual(db.list().map((d) => d.name), ["X"]);
assert.deepEqual(JSON.parse(ls.getItem(DASH_V1_KEY)), v1);

const db2 = makeDashStorage(memory(), () => ++t);
const a = db2.put({ name: "Sales", sql: "SELECT 1;" });
const b = db2.put({ name: "Sales", sql: "SELECT 2;" });
assert.deepEqual([a, b], ["sales", "sales-2"]);
assert.equal(db2.get(a).sql, "SELECT 1;");
assert.equal(db2.get("nope"), null);
assert.equal(db2.get("__proto__"), null);
// an update keeps the id and the place, and stamps `updated`
const before = db2.get(a).updated;
db2.put({ id: a, name: "Sales (renamed)", sql: "SELECT 11;" });
assert.deepEqual(db2.list().map((d) => [d.id, d.name]), [["sales", "Sales (renamed)"], ["sales-2", "Sales"]]);
assert.ok(db2.get(a).updated > before);
// a new one at a place; remove returns the place, put(…, at) restores it (undo)
const c = db2.put({ name: "Middle", sql: "SELECT 3;" }, 1);
assert.deepEqual(db2.list().map((d) => d.id), ["sales", "middle", "sales-2"]);
const gone = db2.get(c);
const at = db2.remove(c);
assert.deepEqual([at, db2.list().map((d) => d.id)], [1, ["sales", "sales-2"]]);
db2.put(gone, at);
assert.deepEqual(db2.list().map((d) => d.id), ["sales", "middle", "sales-2"]);
assert.equal(db2.get(c).updated, gone.updated, "an undo keeps the time it was edited");
assert.equal(db2.remove("nope"), -1);
// the sidebar state
db2.setMeta({ groups: ["B", "A", "A", 3], collapsed: { A: true }, sort: "name", order: ["sales-2", "sales"] });
assert.deepEqual(db2.meta(), { order: ["sales-2", "sales", "middle"], groups: ["B", "A"], collapsed: { A: true }, sort: "name", recent: [], recentOpen: false });
db2.setMeta({ recent: ["my:sales", 4, "ex:a", "ex:b", "ex:c", "ex:d", "ex:e"], recentOpen: true });
assert.deepEqual([db2.meta().recent, db2.meta().recentOpen], [["my:sales", "ex:a", "ex:b", "ex:c", "ex:d"], true], "recent: strings, at most 5");
db2.setMeta({ sort: "bogus" });
assert.equal(db2.meta().sort, "manual");

const pkg = new URL("../../web/pkg/", import.meta.url);
if (!existsSync(new URL("anofox_visualization_bg.wasm", pkg))) {
  console.log("web/pkg not built — skipping the wasm part of dash_store_test");
  process.exit(0);
}
const wasm = await import(new URL("anofox_visualization.js", pkg));
wasm.initSync({ module: readFileSync(new URL("anofox_visualization_bg.wasm", pkg)) });
const groupOf = (sql) => JSON.parse(wasm.dashboard_meta(sql)).group;

// The rewrite is what the servers' parser reads — on every example and on
// odd headers; `@title` / `@tags` and the SQL are kept.
const odd = ["SELECT 1;", "-- @group A\n-- @group B\nSELECT 1;", "-- @title T\n-- @tags x, y\nSELECT 1;", "-- @group\nSELECT 1;"];
const examples = readdirSync(new URL("../../web/examples/", import.meta.url))
  .filter((f) => f.endsWith(".sql"))
  .map((f) => readFileSync(new URL(`../../web/examples/${f}`, import.meta.url), "utf8"));
for (const sql of [...odd, ...examples]) {
  const meta = JSON.parse(wasm.dashboard_meta(sql));
  for (const g of ["Sales", "Sales / EMEA", "Q3 & Q4 — ops"]) {
    const out = setGroupSql(sql, g);
    const after = JSON.parse(wasm.dashboard_meta(out));
    assert.equal(after.group, g, out.slice(0, 120));
    assert.equal(after.title, meta.title);
    assert.deepEqual(after.tags, meta.tags);
    const body = (s) => s.split("\n").filter((l) => !lib.isHeaderLine(l, "group"));
    assert.deepEqual(body(out), body(sql));
    assert.equal(setGroupSql(out, g), out, "setting the same group again changes nothing");
  }
  assert.equal(groupOf(setGroupSql(sql, "")), null);
}
// Saving an example as yours: its library area is not kept as a group (the
// title follows the name, the tags stay); your own dashboards keep theirs.
for (const sql of examples) {
  const meta = JSON.parse(wasm.dashboard_meta(sql));
  const saved = JSON.parse(wasm.dashboard_meta(lib.sqlToSave(sql, "Mine", true)));
  assert.equal(saved.group, null);
  assert.deepEqual([saved.title, saved.tags], [meta.title, meta.tags]);
  assert.equal(groupOf(lib.sqlToSave(sql, "Mine", false)), meta.group);
}
// a migrated v1 dashboard is in its group for the servers too
assert.equal(groupOf(m.items["weekly-sales-2"].sql), "Sales");

console.log("dash_store_test: ok");
