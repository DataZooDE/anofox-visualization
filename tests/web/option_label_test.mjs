// ::OPTION_LABEL (#170): an option's shown text; the variable keeps the value.
import { readFileSync } from "fs";
import assert from "assert/strict";
const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { inputOptions, optionLabel } = new Function(`${grab("inputOptions")}\n${grab("optionLabel")}\nreturn { inputOptions, optionLabel };`)();
const rows = [
  { run: "20260928164905", c1: "28 Sep 2026 16:49 — kinaxis_nodynlm (568k series)", c2: "Run" },
  { run: "kinaxis_nodynlm", c1: null, c2: "Run" },
  { run: "x", c1: "  ", c2: "Run" },
];
const m = inputOptions([[0, "DROPDOWN"], [1, "OPTION_LABEL"], [2, "LABEL"]], rows, "run");
assert.deepEqual(m.options, ["20260928164905", "kinaxis_nodynlm", "x"]);
assert.deepEqual(m.labels, ["28 Sep 2026 16:49 — kinaxis_nodynlm (568k series)", "kinaxis_nodynlm", "x"]);
assert.equal(m.hints, null);
assert.equal(optionLabel(m, 0), "28 Sep 2026 16:49 — kinaxis_nodynlm (568k series)");
const h = inputOptions([[0, "BUTTONS"], [1, "HINT"]], [{ z: "EU", c1: 3 }, { z: "US", c1: null }], "z");
assert.deepEqual(h, { options: ["EU", "US"], labels: null, hints: ["3", ""] });
assert.equal(optionLabel(h, 1), "US");
const mk = grab("makeControl");
const fso = grab("fillSelectOptions");
assert.ok(/opt\.value = o;/.test(fso) && /optionLabel\(meta, k\)/.test(fso), "dropdown: value o, shown label");
assert.ok(/fillSelectOptions\(input, meta\)/.test(mk), "dropdown <select> fills via fillSelectOptions");
assert.ok(/r\.l \+/.test(mk), "multiselect search reads the label");
const bt = grab("mkButtons");
assert.ok(/b\.textContent = optionLabel\(meta, k\)/.test(bt) && /dpVars\[meta\.varname\] = o;/.test(bt));
process.stdout.write("option_label_test: ok\n");
