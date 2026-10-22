// Node test for the served nav's grouping in web/app.js (sidebar plan, PR 4):
// servedNavGroups orders a served folder's dashboards like the server's `/`
// list — the ungrouped first, then the groups by name — and keeps the
// server's order inside a group. The nav turns into a searchable menu from
// SERVED_NAV_MENU_MIN dashboards, the same threshold as the list page
// (src/page.rs LIST_SEARCH_MIN). Also navLabel (#355): the compact
// `-- @nav` label a nav link, a start-page card and the examples sidebar
// all fall back through the same way.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const fn = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { servedNavGroups, navLabel } = new Function(`${fn("servedNavGroups")}\n${fn("navLabel")}\nreturn { servedNavGroups, navLabel };`)();

// -- @nav when set, else the title, else the fallback (the file id, or "")
assert.equal(navLabel("Supplier risk", "Which products depend on this supplier?", "s"), "Supplier risk");
assert.equal(navLabel(undefined, "Which products depend on this supplier?", "s"), "Which products depend on this supplier?");
assert.equal(navLabel(undefined, undefined, "s"), "s");
assert.equal(navLabel(undefined, undefined, undefined), "");
assert.equal(navLabel("", "", ""), "");

const nav = [
  { id: "a", title: "A", group: "sales" },
  { id: "b", title: "B", group: null },
  { id: "c", title: "C", group: "Ops" },
  { id: "d", title: "D", group: "Sales" },
  { id: "e", title: "E" },
  { id: "f", title: "F", group: "sales" },
];
assert.deepEqual(
  servedNavGroups(nav).map((g) => [g.group, g.items.map((d) => d.id).join("")]),
  [["", "be"], ["Ops", "c"], ["Sales", "d"], ["sales", "af"]]
);
assert.deepEqual(servedNavGroups([]), []);
assert.deepEqual(servedNavGroups(undefined), []);

const js = Number(src.match(/^const SERVED_NAV_MENU_MIN = (\d+);/m)?.[1]);
const rs = Number(readFileSync(new URL("../../src/page.rs", import.meta.url), "utf8").match(/pub const LIST_SEARCH_MIN: usize = (\d+);/)?.[1]);
assert.ok(js > 0 && js === rs, `menu threshold ${js} vs list search ${rs}`);
console.log("served_nav_test: ok");
