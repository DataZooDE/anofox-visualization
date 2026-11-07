// #1022: the chart catalogue — one page per chart kind (Grafana-style: what it
// is, a picture, the data it needs, a minimal panel to edit in the builder, the
// examples that use it, related kinds), generated at build time from
// docs/DOCS.md's role table (itself generated from src/roles.rs), the data
// shapes in chart-shapes.mjs and the builder's examples (web/examples/*.sql).
// Output: src/content/docs/build/charts/ (not committed).
//
//   node scripts/gen-charts.mjs           write the pages
//   node scripts/gen-charts.mjs --check   every chart kind has a shape and a
//       picture, and every minimal panel lints without an error in the
//       dashboard CLI (DASHBOARD, else target/debug/dashboard)
//   node scripts/gen-charts.mjs --thumbs  re-render the pictures
//       (src/assets/charts/*.svg) with the dashboard CLI
import { execFileSync } from "child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "fs";
import os from "os";
import path from "path";
import { shapes } from "./chart-shapes.mjs";

const root = path.resolve(import.meta.dirname, "..", "..");
const site = path.resolve(import.meta.dirname, "..");
const out = path.join(site, "src", "content", "docs", "build", "charts");
const thumbs = path.join(site, "src", "assets", "charts");
const mode = process.argv[2] || "";

const doc = readFileSync(path.join(root, "docs", "DOCS.md"), "utf8");
const table = doc.slice(doc.indexOf("<!-- roles-table:start -->"), doc.indexOf("<!-- roles-table:end -->"));
const all = [];
for (const line of table.split("\n")) {
  const c = line.split(" | ");
  if (c.length < 4 || c[2] !== "chart") continue;
  all.push({ name: c[0].replace(/^\| `::|`$/g, ""), aliases: c[1].trim(), desc: c.slice(3).join(" | ").replace(/ \|$/, "").trim() });
}
// A companion (::LON beside ::LAT) shares its partner's page.
const companions = new Set(Object.values(shapes).map((s) => s.with).filter(Boolean));
const kinds = all.filter((k) => !companions.has(k.name));
const slug = (n) => n.toLowerCase().replace(/_/g, "-");

const families = [
  ["Bars", /^(BARCHART|WATERFALL|BULLET|FUNNEL|PARETO|GANTT|DUMBBELL)/],
  ["Lines and areas", /^(LINECHART|STEP|SMOOTH|AREACHART|SPARK|CANDLESTICK)/],
  ["Points", /^(SCATTER|BUBBLE|JITTER)/],
  ["Distributions", /^(HISTOGRAM|DENSITY|BOXPLOT|VIOLIN|QQ|RIDGELINE)/],
  ["Part of a whole and flows", /^(PIE|DONUT|TREEMAP|SANKEY|RADAR|GAUGE)/],
  ["Grids", /^(HEATMAP|CALENDAR)/],
  ["Maps", /^(MAP|REGION|LAT|ORIGIN|BASEMAP)/],
  ["Diagrams", /^DIAGRAM/],
];
const family = (n) => (families.find(([, re]) => re.test(n)) || ["Other"])[0];

const cli = () => {
  const bin = process.env.DASHBOARD || path.join(root, "target", "debug", "dashboard");
  if (!existsSync(bin)) throw new Error(`no dashboard CLI at ${bin} (cargo build --bin dashboard, or set DASHBOARD)`);
  return bin;
};

if (mode === "--check" || mode === "--thumbs") {
  const bin = cli();
  const tmp = mkdtempSync(path.join(os.tmpdir(), "charts-"));
  const problems = [];
  for (const k of kinds) {
    const s = shapes[k.name];
    if (!s) { problems.push(`::${k.name}: no entry in chart-shapes.mjs`); continue; }
    const f = path.join(tmp, `${slug(k.name)}.sql`);
    writeFileSync(f, s.sql + ";\n");
    if (mode === "--thumbs") {
      mkdirSync(thumbs, { recursive: true });
      const svg = path.join(thumbs, `${slug(k.name)}.svg`);
      execFileSync(bin, ["--svg", f, svg, "--width", "560"], { stdio: ["ignore", "ignore", "inherit"] });
      // The headless render is a whole dashboard: a page background around
      // one card. Keep the card.
      const t = readFileSync(svg, "utf8");
      const card = /^<svg [^>]*><rect width="[\d.]+" height="[\d.]+" fill="[^"]+"\/>(<rect x="([\d.]+)" y="([\d.]+)" width="([\d.]+)" height="([\d.]+)")/.exec(t);
      if (card) {
        const [, , x, y, w, h] = card;
        writeFileSync(svg, t.replace(/^<svg ([^>]*?)width="[\d.]+" height="[\d.]+" viewBox="[^"]+"/, `<svg $1width="${w}" height="${h}" viewBox="${x} ${y} ${w} ${h}"`).replace(/<rect width="[\d.]+" height="[\d.]+" fill="[^"]+"\/>/, ""));
      }
      continue;
    }
    let r;
    try {
      r = JSON.parse(execFileSync(bin, ["--check", f, "--json"], { encoding: "utf8" }));
    } catch (e) {
      r = JSON.parse(e.stdout || '{"diagnostics":[{"severity":"error","message":"no output"}]}');
    }
    for (const d of r.diagnostics || []) if (d.severity === "error") problems.push(`::${k.name}: ${d.code} — ${d.message.split("\n")[0]}`);
    if (!existsSync(path.join(thumbs, `${slug(k.name)}.svg`))) problems.push(`::${k.name}: no picture (node scripts/gen-charts.mjs --thumbs)`);
  }
  for (const n of Object.keys(shapes)) if (!kinds.some((k) => k.name === n)) problems.push(`chart-shapes.mjs: ::${n} is not a chart kind in docs/DOCS.md`);
  rmSync(tmp, { recursive: true, force: true });
  if (problems.length) {
    console.error(problems.join("\n"));
    process.exit(1);
  }
  console.log(`charts: ${kinds.length} kinds ${mode === "--thumbs" ? "rendered" : "ok"}`);
  process.exit(0);
}

// An example's id in the builder's `?dashboard=`: its title's slug
// (web/app.js slugify), not its file name.
const slugify = (name) =>
  String(name).normalize("NFKD").replace(/[\u0300-\u036f]/g, "").toLowerCase().replace(/ß/g, "ss")
    .replace(/&/g, " and ").replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "dashboard";

// The examples: title, id, and their statements.
const exDir = path.join(root, "web", "examples");
const examples = readdirSync(exDir).filter((f) => f.endsWith(".sql")).sort().map((f) => {
  const sql = readFileSync(path.join(exDir, f), "utf8");
  const title = (/^-- @title (.+)$/m.exec(sql) || [, f])[1].trim();
  return { id: slugify(title), title, stmts: sql.split(/;\s*\n/) };
});
const code = (stmt) => stmt.split("\n").filter((l) => !/^\s*--/.test(l)).join("\n").trim();
const uses = (stmt, n) => new RegExp(`::${n}(?![A-Z0-9_])`).test(code(stmt));
const app = "../../../app/";
const roleCode = (t) => t.replace(/::([A-Z][A-Z0-9_]+)/g, "`::$1`").replace(/``/g, "`");
const cap = (t) => t.charAt(0).toUpperCase() + t.slice(1);
// MDX reads { } and < in prose as JSX: escape them outside `code` spans.
const prose = (t) => t.split(/(`[^`]*`)/).map((p, i) => (i % 2 ? p : p.replace(/[{}]/g, "\\$&").replace(/</g, "&lt;"))).join("");
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
for (const k of kinds) {
  const s = shapes[k.name] || {};
  const mate = s.with && all.find((o) => o.name === s.with);
  const ex = examples.filter((e) => e.stmts.some((st) => uses(st, k.name)));
  const first = ex[0];
  const lines = code(first?.stmts.find((st) => uses(st, k.name)) || "").split("\n");
  const related = kinds.filter((o) => o.name !== k.name && family(o.name) === family(k.name)).slice(0, 8);
  const aliases = [k.aliases, mate && `${"`::" + mate.name + "`"}${mate.aliases ? ` (${mate.aliases})` : ""}`].filter(Boolean).join(" · ");
  const md = [
    "---",
    `title: ${JSON.stringify(s.title || "::" + k.name)}`,
    `description: ${JSON.stringify(cap(k.desc.replace(/`/g, "")))}`,
    "sidebar:",
    `  label: "${k.name.toLowerCase()}"`,
    "---",
    "",
    'import { Badge } from "@astrojs/starlight/components";',
    "",
    '<Badge text="Dashboards: browser · CLI · servers" variant="success" /> <Badge text="Community extension: anofox_render" variant="note" />',
    "",
    prose(cap(roleCode(k.desc)) + "." + (mate ? ` With ${"`::" + mate.name + "`"}: ${roleCode(mate.desc)}.` : "")),
    "",
    aliases ? `**Also spelled:** ${aliases}` : "",
    "",
    existsSync(path.join(thumbs, `${slug(k.name)}.svg`)) ? `![The minimal panel below, rendered](../../../../assets/charts/${slug(k.name)}.svg)` : "",
    "",
    "## The data it needs",
    "",
    prose(s.shape || ""),
    "",
    s.sql ? `\`\`\`sql\n${s.sql};\n\`\`\`\n\n[Edit it in the builder ↗](${app}?mode=edit#sql=${b64(s.sql + ";\n")})` : "",
    "",
    first ? `## In an example\n\nFrom *${first.title}* — [open it in the builder ↗](${app}?dashboard=${first.id}):\n\n\`\`\`sql\n${lines.slice(0, 16).join("\n")}${lines.length > 16 ? "\n-- …" : ""}\n\`\`\`` : "",
    "",
    ex.length > 1 ? `Also in: ${ex.slice(1, 7).map((e) => `[${e.title}](${app}?dashboard=${e.id})`).join(" · ")}` : "",
    "",
    related.length ? `## Related\n\n${related.map((o) => `[${shapes[o.name]?.title ? "`" + shapes[o.name].title + "`" : "`::" + o.name + "`"}](../${slug(o.name)}/)`).join(" · ")}` : "",
    "",
  ].join("\n");
  writeFileSync(path.join(out, `${slug(k.name)}.mdx`), md.replace(/\n{3,}/g, "\n\n"));
}

// The overview: by family, a picture per kind.
const idx = [
  "---",
  "title: Charts",
  "description: Every chart kind, grouped by what it shows — one page each with the data it needs and a panel to edit in the builder.",
  "sidebar:",
  "  label: Overview",
  "  order: 0",
  "---",
  "",
  `${kinds.length} chart kinds. A chart is the cast on the measure column; the other columns of the \`SELECT\` say where and how each row is drawn. Pick by the question: compare amounts → bars; change over time → lines; a relationship → points; spread → distributions; shares and flows → part of a whole; geography → maps.`,
  "",
];
for (const [fam] of [...families, ["Other"]]) {
  const ks = kinds.filter((k) => family(k.name) === fam);
  if (!ks.length) continue;
  idx.push(`## ${fam}`, "", '<div class="chart-grid">', "");
  for (const k of ks) {
    const t = shapes[k.name]?.title || "::" + k.name;
    const pic = existsSync(path.join(thumbs, `${slug(k.name)}.svg`));
    idx.push(`<a href="${slug(k.name)}/">${pic ? `<img src={pic_${k.name}} alt="" loading="lazy" />` : ""}<code>${t}</code><span>${k.desc.replace(/`/g, "").replace(/[{}<>]/g, "")}</span></a>`);
  }
  idx.push("", "</div>", "");
}
const imports = kinds.filter((k) => existsSync(path.join(thumbs, `${slug(k.name)}.svg`))).map((k) => `import pic_${k.name} from "../../../../assets/charts/${slug(k.name)}.svg?url";`);
idx.splice(idx.indexOf("---", 1) + 1, 0, "", ...imports);
writeFileSync(path.join(out, "index.mdx"), idx.join("\n"));
console.log(`charts: ${kinds.length} pages`);
