// The anofox-visualization website (#1017, #1020): Astro Starlight.
// SITE_BASE is where it is served on GitHub Pages: /anofox-visualization/next
// for the preview (P1), the root once it replaces the builder there.
import { defineConfig, passthroughImageService } from "astro/config";
import starlight from "@astrojs/starlight";
import { existsSync, readFileSync, statSync } from "fs";
import path from "path";

const base = process.env.SITE_BASE ?? "/anofox-visualization/next";

// `npm run dev` only: the builder (../web, with web/pkg built) at <base>/app/,
// where Pages puts master's builder — so the site's "open in the builder"
// links work locally too. The production build never includes it.
const types = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css",
  ".json": "application/json", ".wasm": "application/wasm", ".svg": "image/svg+xml", ".png": "image/png",
  ".sql": "text/plain", ".txt": "text/plain", ".woff2": "font/woff2", ".geojson": "application/json" };
const builderInDev = {
  name: "builder-in-dev",
  hooks: {
    "astro:server:setup": ({ server }) => {
      const web = path.resolve(import.meta.dirname, "..", "web");
      const prefix = base.replace(/\/$/, "") + "/app/";
      server.middlewares.use((req, res, next) => {
        // the dev server hands middlewares the path with or without the base
        let url = decodeURIComponent((req.url || "").split(/[?#]/)[0]);
        if (url.startsWith(prefix)) url = url.slice(prefix.length - "/app/".length);
        if (!url.startsWith("/app/")) return next();
        let file = path.join(web, url.slice("/app/".length));
        if (!file.startsWith(web)) return next();
        if (existsSync(file) && statSync(file).isDirectory()) file = path.join(file, "index.html");
        if (!existsSync(file)) return next();
        res.setHeader("Content-Type", types[path.extname(file)] || "application/octet-stream");
        res.setHeader("Cache-Control", "no-store");
        res.end(readFileSync(file));
      });
    },
  },
};

export default defineConfig({
  site: "https://datazoode.github.io",
  base,
  trailingSlash: "always",
  devToolbar: { enabled: false },
  image: { service: passthroughImageService() },
  integrations: [
    builderInDev,
    starlight({
      title: "anofox-visualization",
      logo: { src: "./src/assets/logo.svg" },
      favicon: "/favicon.svg",
      // the site's typefaces: Instrument Sans, Instrument Serif (titles), JetBrains Mono
      head: [
        { tag: "link", attrs: { rel: "preconnect", href: "https://fonts.googleapis.com" } },
        { tag: "link", attrs: { rel: "preconnect", href: "https://fonts.gstatic.com", crossorigin: "" } },
        { tag: "link", attrs: { rel: "stylesheet", href: "https://fonts.googleapis.com/css2?family=Instrument+Sans:wght@400..700&family=Instrument+Serif:ital@0;1&family=JetBrains+Mono:wght@400;500&display=swap" } },
      ],
      description: "Charts and dashboards for DuckDB, straight from SQL.",
      social: [{ icon: "github", label: "GitHub", href: "https://github.com/DataZooDE/anofox-visualization" }],
      editLink: { baseUrl: "https://github.com/DataZooDE/anofox-visualization/edit/master/website/" },
      customCss: ["./src/styles/theme.css", "./src/styles/landing.css"],
      lastUpdated: false,
      sidebar: [
        { label: "Start", items: [
          { label: "What it is", slug: "start/overview" },
          { label: "Why SQL for AI-made charts", slug: "start/why-sql" },
          { label: "Install", slug: "start/install" },
          { label: "Your first dashboard", slug: "start/first-dashboard" },
        ] },
        { label: "Gallery", items: [
          { label: "What you can build", slug: "gallery" },
          { label: "Inputs and filters", slug: "gallery/inputs" },
          { label: "KPIs", slug: "gallery/kpis" },
          { label: "Tables", slug: "gallery/tables" },
          { label: "Maps", slug: "gallery/maps" },
          { label: "Interactions and features", slug: "gallery/features" },
          { label: "All 42 chart kinds", link: "/build/charts/" },
          { label: "What each extension draws", collapsed: true, items: [
            { label: "anofox-statistics", slug: "gallery/statistics" },
            { label: "anofox-forecast", slug: "gallery/forecast" },
            { label: "anofox-tabular", slug: "gallery/tabular" },
            { label: "anofox-optimize", slug: "gallery/optimize" },
            { label: "anofox-decide", slug: "gallery/decide" },
          ] },
        ] },
        { label: "Build", items: [
          { label: "The annotation model", slug: "build" },
          { label: "Charts", collapsed: true, items: [{ autogenerate: { directory: "build/charts" } }] },
          { label: "KPIs and tables", slug: "build/kpis-tables" },
          { label: "Inputs and interactivity", slug: "build/inputs" },
          { label: "Layout and tabs", slug: "build/layout" },
          { label: "Models and forecasts", slug: "build/model-output" },
          { label: "Themes and branding", slug: "build/theming" },
          { label: "Design guide", slug: "build/design" },
        ] },
        { label: "Connect", items: [
          { label: "Where the data comes from", slug: "connect" },
          { label: "Files", slug: "connect/files" },
          { label: "HTTPS, S3, GCS", slug: "connect/remote" },
          { label: "Databases", slug: "connect/databases" },
        ] },
        { label: "Deploy", items: [
          { label: "Which way to run it", slug: "deploy" },
          { label: "In the browser", slug: "deploy/browser", badge: { text: "Browser", variant: "success" } },
          { label: "CLI, HTML and CI", slug: "deploy/cli", badge: { text: "CLI", variant: "note" } },
          { label: "A static website", slug: "deploy/static-site", badge: { text: "CLI", variant: "note" } },
          { label: "Authoring on your data", slug: "deploy/authoring", badge: { text: "Full toolkit", variant: "note" } },
          { label: "The locked server", slug: "deploy/locked-server", badge: { text: "Locked server", variant: "tip" } },
          { label: "Login and row policies", slug: "deploy/security", badge: { text: "Locked server", variant: "tip" } },
          { label: "Embedding in your app", slug: "deploy/embedding", badge: { text: "Locked server", variant: "tip" } },
          { label: "Agents and MCP", slug: "deploy/agent", badge: { text: "Locked server", variant: "tip" } },
          { label: "Platform recipes", slug: "deploy/recipes" },
          { label: "Checklist", slug: "deploy/checklist" },
        ] },
        { label: "Integrate", items: [
          { label: "The anofox extensions", slug: "integrate" },
          { label: "anofox-statistics", slug: "integrate/statistics" },
          { label: "anofox-forecast", slug: "integrate/forecast" },
          { label: "anofox-tabular", slug: "integrate/tabular" },
          { label: "anofox-optimize", slug: "integrate/optimize" },
          { label: "anofox-decide", slug: "integrate/decide" },
        ] },
        { label: "Reference", items: [
          { label: "Overview and CLI", slug: "reference" },
          { label: "Roles", slug: "reference/roles" },
        ] },
        { label: "Releases", items: [
          { label: "Releases and upgrades", slug: "releases" },
          { label: "FAQ and troubleshooting", slug: "releases/faq" },
        ] },
        { label: "Builder ↗", link: "/app/", attrs: { target: "_blank" } },
      ],
    }),
  ],
});
