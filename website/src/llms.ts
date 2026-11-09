// #1022: the text behind /llms.txt and /llms-full.txt — the docs pages in
// sidebar order (Start, Build, Deploy, Integrate), as Markdown without MDX
// imports and components.
import { getCollection } from "astro:content";

const site = "https://datazoode.github.io" + import.meta.env.BASE_URL.replace(/\/$/, "");
const sections: [string, string][] = [["start", "Start"], ["gallery", "Gallery"], ["build", "Build"], ["build/charts", "Charts"], ["connect", "Connect"], ["deploy", "Deploy"], ["integrate", "Integrate"], ["reference", "Reference"], ["releases", "Releases"]];
const section = (id: string) => [...sections].reverse().find(([p]) => id === p || id.startsWith(p + "/"))?.[1];

// The sidebar's order (astro.config.mjs); the chart pages follow their overview.
const ORDER = ["start/overview", "start/install", "start/first-dashboard", "gallery", "gallery/inputs", "gallery/kpis",
  "gallery/tables", "gallery/maps", "gallery/features", "gallery/statistics", "gallery/forecast", "gallery/tabular", "gallery/optimize",
  "gallery/decide", "build", "build/kpis-tables", "build/inputs", "build/layout",
  "build/model-output", "build/theming", "build/design", "build/charts", "deploy", "deploy/browser", "deploy/cli", "deploy/static-site",
  "deploy/authoring", "deploy/locked-server", "deploy/security", "deploy/embedding", "deploy/agent", "deploy/recipes", "deploy/checklist", "integrate",
  "integrate/statistics", "integrate/forecast", "integrate/tabular", "integrate/optimize", "integrate/decide", "connect", "connect/files", "connect/remote", "connect/databases", "reference", "reference/roles", "releases", "releases/faq"];

const clean = (body: string) =>
  body
    .replace(/^import .*$/gm, "")
    .replace(/<Badge text="([^"]*)"[^>]*\/>/g, "[$1]")
    .replace(/<[A-Z][^>]*\/>/g, "")
    .replace(/<img [^>]*\/>/g, "")
    .replace(/<\/?(div|details|summary|figure|figcaption)[^>]*>/g, "")
    .replace(/!\[[^\]]*\]\([^)]*\)\n?/g, "")
    .replace(/\n{3,}/g, "\n\n")
    .trim();

export async function pages(kind: "index" | "full") {
  const docs = (await getCollection("docs")).filter((d) => d.id !== "index" && section(d.id));
  const order = (id: string) => sections.findIndex(([, t]) => t === section(id));
  const rank = (id: string) => (ORDER.indexOf(id) + 1 || 1000);
  docs.sort((a, b) => order(a.id) - order(b.id) || rank(a.id) - rank(b.id) || a.id.localeCompare(b.id));
  const url = (id: string) => `${site}/${id.replace(/\/?index$/, "")}/`.replace(/\/+$/, "/");
  const head = [
    "# anofox-visualization",
    "",
    "> Charts and dashboards for DuckDB, straight from SQL: a dashboard is a .sql file whose SELECT columns carry ::ROLE casts (::XAXIS, ::BARCHART, ::DROPDOWN, …). It runs in the browser (DuckDB-Wasm), as a CLI, as a DuckDB extension and as a locked, read-only server.",
    "",
  ];
  if (kind === "full") {
    return [...head, ...docs.map((d) => `---\n\n# ${d.data.title}\n\nSource: ${url(d.id)}\n\n${clean(d.body ?? "")}\n`)].join("\n");
  }
  const out = [...head, `The whole documentation in one file: ${site}/llms-full.txt`, ""];
  let cur = "";
  for (const d of docs) {
    const s = section(d.id)!;
    if (s !== cur) out.push("", `## ${s}`, "");
    cur = s;
    out.push(`- [${d.data.title}](${url(d.id)})${d.data.description ? ": " + d.data.description : ""}`);
  }
  out.push("", "## Optional", "", "- [Role reference](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/DOCS.md#roles): every ::ROLE", "- [Claude Code plugin](https://github.com/DataZooDE/anofox-visualization/tree/master/plugins/anofox-visualization): skills that write dashboards from a plain request");
  return out.join("\n") + "\n";
}
