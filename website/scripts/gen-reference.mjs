// #1017 P3: the Reference — every ::ROLE, grouped by category, generated from
// docs/DOCS.md's role table (itself generated from src/roles.rs). Written in
// place to src/content/docs/reference/roles.md (not committed).
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "fs";
import path from "path";

const root = path.resolve(import.meta.dirname, "..", "..");
const out = path.join(import.meta.dirname, "..", "src", "content", "docs", "reference");
const doc = readFileSync(path.join(root, "docs", "DOCS.md"), "utf8");
const table = doc.slice(doc.indexOf("<!-- roles-table:start -->"), doc.indexOf("<!-- roles-table:end -->"));
const rows = table.split("\n").filter((l) => l.startsWith("| `::")).map((l) => {
  const c = l.split(" | ");
  return { name: c[0].replace(/^\| /, ""), aliases: c[1].trim(), cat: c[2].trim(), desc: c.slice(3).join(" | ").replace(/ \|$/, "").trim() };
});
const order = ["chart", "encoding", "modifier", "annotation", "kpi", "table", "input", "layout", "chrome"];
const titles = { chart: "Chart kinds", encoding: "Encodings", modifier: "Modifiers", annotation: "Annotations", kpi: "KPIs and text", table: "Tables", input: "Inputs", layout: "Layout and directives", chrome: "Page chrome" };
const cats = [...new Set([...order, ...rows.map((r) => r.cat)])].filter((c) => rows.some((r) => r.cat === c));
const md = [
  "---",
  "title: Roles",
  `description: All ${rows.length} ::ROLE casts by category — generated from the role registry (src/roles.rs).`,
  "---",
  "",
  `All ${rows.length} roles, generated from the role registry (\`src/roles.rs\`). \`dashboard --roles\` prints the same list; \`dashboard --check\` flags an unknown one. Chart kinds have [a page each](../../build/charts/).`,
  "",
  ...cats.flatMap((c) => [
    `## ${titles[c] || c}`,
    "",
    "| Cast | Also | Meaning |",
    "|---|---|---|",
    ...rows.filter((r) => r.cat === c).map((r) => `| ${r.name} | ${r.aliases} | ${r.desc} |`),
    "",
  ]),
].join("\n");
mkdirSync(out, { recursive: true });
const f = path.join(out, "roles.md");
if (!existsSync(f) || readFileSync(f, "utf8") !== md) writeFileSync(f, md);
console.log(`reference: ${rows.length} roles`);
