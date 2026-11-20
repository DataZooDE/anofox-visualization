---
title: Embedding in your app
description: A dashboard inside your own web app (a web component, React, or a signed link), or the renderer inside your own Rust program.
---

```html
<script type="module" src="https://dash.example.com/sdk/anofox-embed.js"></script>
<anofox-dashboard src="https://dash.example.com/d/sales" token-callback="getToken"></anofox-dashboard>
<script>
  // your backend mints a short-lived token for the signed-in user
  window.getToken = () => fetch("/my-api/anofox-token").then((r) => r.text());
</script>
```

- **`<anofox-dashboard>`** (`web/sdk/anofox-embed.js`) frames a locked server's dashboard, sizes it to its content, and passes state and link events both ways; **`anofox-react.js`** wraps it for React.
- **Signed embed links** (`"mode": "embed"`): your backend signs a short-lived link carrying the reader's claims, so the server's row policy applies without a second login.
- `token-callback` names a function of your page that returns a token from your backend (signed embed token or API key); the dashboard refreshes it when it expires.

Details: [Embedding with `<anofox-dashboard>`](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/secure-serving.md#embedding-with-anofox-dashboard-771), [signed embed links](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/secure-serving.md#signed-embed-links-mode-embed).

## As a Rust library

A Rust program can render charts and dashboards to SVG without any server. Your program brings the rows; nothing opens a port, runs SQL or checks a login.

```toml
[dependencies]
anofox-visualization = { git = "https://github.com/DataZooDE/anofox-visualization", tag = "v2026.11.21", default-features = false }
```

```rust
use anofox_visualization::embed::{render_spec_checked, RenderLimits};

let spec = r#"{"rows": [{"c0": "a", "c1": 1}, {"c0": "b", "c1": 3}],
              "roles": [[0, "XAXIS", "k"], [1, "BARCHART", "v"]], "width": 480, "height": 320}"#;
let svg = render_spec_checked(spec, &RenderLimits::default())?;
```

Use only `anofox_visualization::embed`:
- one chart from rows and roles;
- a whole dashboard, with a `DataProvider` answering its queries;
- splitting a script into statements, and the lint without a database;
- the role registry, themes and locales.

Releases are CalVer, so a change to this module is announced in the release notes under **Embedding API**; anything outside it may change in any release. CI builds a small host against exactly this module with default features off, and checks that build stays lean: `ggplot-rs`, `serde_json`, `roxmltree`, `sha2`, and no server, TLS, plugin runtime or PDF stack.

Details: [docs/embedding.md](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/embedding.md).
