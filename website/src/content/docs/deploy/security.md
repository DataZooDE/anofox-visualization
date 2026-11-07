---
title: Login and row policies
description: Who may open a dashboard, and which rows and columns they see.
---

A locked server knows its reader in one of these ways (`options.auth`):

| Mode | When |
|---|---|
| **Auth proxy** | an existing SSO in front (oauth2-proxy, Pomerium, Cloudflare Access): trusted identity headers plus a shared secret |
| **Built-in login** | users with passwords in a file, sessions by the server |
| **OIDC** | sign-in with your identity provider (Entra ID, Okta, Keycloak, Google) |
| **JWT bearer** | machine callers and apps with tokens from your issuer |
| **API keys** | scripts, exports, ingest, previews — scoped to dashboards and variables |
| **Signed embed links** | dashboards inside another app ([Embedding](../embedding/)) |

The identity becomes **claims** (`__claim_user`, `__claim_groups`, `__claim_<name>`). A **row and column policy** (`options.policy`, JSON) rewrites every query: a table gets a row filter on the claims, columns are hidden or masked — for every panel, export and API alike. Without a policy a server with a login refuses to start unless told `"policy": "none"`.

Details: [login via a proxy](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/secure-serving.md#login-via-an-auth-proxy-access-control-p1), [built-in login](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/secure-serving.md#built-in-login-access-control-p3), [policies](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/secure-serving.md#row-and-column-policies-access-control-p2), [what remains the operator's job](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/secure-serving.md#what-remains-the-operators-job).
