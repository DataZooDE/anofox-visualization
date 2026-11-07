---
title: Embedding in your app
description: A dashboard inside your own web app — a web component, React, or a signed link.
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
