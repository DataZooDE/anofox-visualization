// The anofox-visualization website (#1017, #1020): Astro Starlight.
// SITE_BASE is where it is served on GitHub Pages: /anofox-visualization/next
// for the preview (P1), the root once it replaces the builder there.
import { defineConfig, passthroughImageService } from "astro/config";
import starlight from "@astrojs/starlight";

const base = process.env.SITE_BASE ?? "/anofox-visualization/next";

export default defineConfig({
  site: "https://datazoode.github.io",
  base,
  trailingSlash: "always",
  image: { service: passthroughImageService() },
  integrations: [
    starlight({
      title: "anofox-visualization",
      description: "Charts and dashboards for DuckDB, straight from SQL.",
      social: [{ icon: "github", label: "GitHub", href: "https://github.com/DataZooDE/anofox-visualization" }],
      editLink: { baseUrl: "https://github.com/DataZooDE/anofox-visualization/edit/master/website/" },
      customCss: ["./src/styles/theme.css"],
      lastUpdated: false,
      sidebar: [
        { label: "Start", items: [
          { label: "What it is", slug: "start/overview" },
          { label: "Install", slug: "start/install" },
          { label: "Your first dashboard", slug: "start/first-dashboard" },
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
        { label: "Builder ↗", link: "app/", attrs: { target: "_blank" } },
      ],
    }),
  ],
});
