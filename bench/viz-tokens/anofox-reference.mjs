// #1044: the anofox arm's reference, as the builder's assistant sends it — the
// role reference (from the wasm build's role registry) and the guide
// (web/assist/guide.md). The assistant's own instructions (web/assist/
// instructions.md) are left out: they describe its tools, and the benchmark
// has its own.
//
// --lean: only what one chart answering a question needs, by rule (fixed
// before any results). One role source, not two: the guide's Roles table
// (grouped, with usage notes) without its rows for inputs, viewer answers,
// chart interactions, layout / chrome and themes; the generated one-line-per-
// role list is left out (`--check` still flags an unknown cast). Of the rest
// of the guide: its introduction and the sections on design, tables, heatmaps
// and gotchas. Left out: the worked examples (dashboard features), inputs,
// diagrams, themes and templates.
//
//   node bench/viz-tokens/anofox-reference.mjs [--pkg web/pkg] [--lean] > ref.md
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { roleReference } from "../../web/assist.js";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const i = process.argv.indexOf("--pkg");
const pkg = path.resolve(root, i > 0 ? process.argv[i + 1] : "web/pkg");
const wasm = await import(path.join(pkg, "anofox_visualization.js"));
wasm.initSync({ module: readFileSync(path.join(pkg, "anofox_visualization_bg.wasm")) });
const guide = readFileSync(path.join(root, "web/assist/guide.md"), "utf8");
const LEAN_GUIDE = ["Roles", "Design contract", "Tables", "Heatmaps", "Gotchas"];
const DASHBOARD_ROWS = /^\| (Inputs|Recording|Chart interactions|Layout and chrome|Theme)/;

// The guide's text before its first "## " section, and the kept sections;
// the Roles table without its dashboard-only rows.
function leanGuide(text) {
  return text
    .split(/^(?=## )/m)
    .filter((p, i) => i === 0 || LEAN_GUIDE.some((h) => p.startsWith(`## ${h}`)))
    .map((p) => (p.startsWith("## Roles") ? p.split("\n").filter((l) => !DASHBOARD_ROWS.test(l)).join("\n") : p))
    .join("")
    .trim();
}

if (process.argv.includes("--lean")) {
  process.stdout.write(`# Guide\n\n${leanGuide(guide)}\n`);
} else {
  process.stdout.write(`# Role reference\n\n${roleReference(wasm.roles_json())}\n\n# Guide\n\n${guide.trim()}\n`);
}
