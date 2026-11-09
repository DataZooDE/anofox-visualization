// #1044: the anofox arm's reference, as the builder's assistant sends it — the
// role reference (from the wasm build's role registry) and the guide
// (web/assist/guide.md). The assistant's own instructions (web/assist/
// instructions.md) are left out: they describe its tools, and the benchmark
// has its own.
//
//   node bench/viz-tokens/anofox-reference.mjs [--pkg web/pkg] > ref.md
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
process.stdout.write(`# Role reference\n\n${roleReference(wasm.roles_json())}\n\n# Guide\n\n${guide.trim()}\n`);
