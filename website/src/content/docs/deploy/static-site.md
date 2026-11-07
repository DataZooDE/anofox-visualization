---
title: A static website
description: Publish a folder of dashboards as a website — GitHub Pages, S3 or any web host, no server.
---

```sh
dashboard --site dashboards/ out/ [--data data/] [--options locked.json] [--app <web dir|tarball>]
```

`out/` is a complete site: a start page grouped by `-- @group`, one page per dashboard, the client and its DuckDB-Wasm. Readers interact (inputs, filters, drill-down) entirely in their browser.

- **Data:** baked in at build time, or Parquet / CSV files next to the site fetched by the page (`--data`, local folder or URL).
- **Hosting:** any static host — GitHub Pages (this site is one), S3 + CloudFront, nginx.
- **Access:** whoever can open the site can read everything in it, data included. For per-reader rows use the [locked server](../locked-server/).
- **Ask on a site:** an operator's own model endpoint can answer questions about a published dashboard (`options.site_ask`, opt-in).

Details: [Publish as a static site](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/secure-serving.md#c-publish-as-a-static-site--dashboard---site-417).
