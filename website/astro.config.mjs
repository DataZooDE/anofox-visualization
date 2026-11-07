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
        { label: "Deploy", items: [
          { label: "Which way to run it", slug: "deploy" },
          { label: "In the browser", slug: "deploy/browser" },
          { label: "CLI, HTML and CI", slug: "deploy/cli" },
          { label: "A static website", slug: "deploy/static-site" },
          { label: "Authoring on your data", slug: "deploy/authoring" },
          { label: "The locked server", slug: "deploy/locked-server" },
          { label: "Login and row policies", slug: "deploy/security" },
          { label: "Embedding in your app", slug: "deploy/embedding" },
          { label: "Agents and MCP", slug: "deploy/agent" },
          { label: "Checklist", slug: "deploy/checklist" },
        ] },
        { label: "Integrate", items: [
          { label: "The anofox extensions", slug: "integrate" },
        ] },
        { label: "Builder ↗", link: "app/", attrs: { target: "_blank" } },
      ],
    }),
  ],
});
