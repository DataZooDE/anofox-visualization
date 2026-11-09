// Every internal link and image of the built site resolves (website/dist,
// built with the default SITE_BASE). The builder (app/) is a separate build.
//   npm run build && node scripts/check-links.mjs
import { existsSync, readdirSync, readFileSync, statSync } from "fs";
import path from "path";

const dist = path.join(import.meta.dirname, "..", "dist");
const base = (process.env.SITE_BASE ?? "/anofox-visualization/next").replace(/\/?$/, "/");
const files = readdirSync(dist, { recursive: true }).filter((f) => f.endsWith(".html"));
const bad = new Set();
for (const f of files) {
  const html = readFileSync(path.join(dist, f), "utf8");
  const dir = path.posix.join(base, path.dirname(f).replace(/^\.$/, ""), "/");
  for (const [, u] of html.matchAll(/(?:href|src)="([^"#?]+)[^"]*"/g)) {
    if (/^(https?:|mailto:|data:|\/\/)/.test(u)) continue;
    const p = u.startsWith("/") ? u : path.posix.join(dir, u);
    if (!p.startsWith(base)) { bad.add(`${f} → ${u}`); continue; }
    const rel = p.slice(base.length);
    if (rel.startsWith("app/")) continue;
    let fp = path.join(dist, rel);
    if (existsSync(fp) && statSync(fp).isDirectory()) fp = path.join(fp, "index.html");
    if (!existsSync(fp)) bad.add(`${f} → ${u}`);
  }
}
if (bad.size) { console.error([...bad].join("\n")); process.exit(1); }
console.log(`links: ${files.length} pages, all internal links resolve`);
