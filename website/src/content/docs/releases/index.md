---
title: Releases and upgrades
description: Where the releases are, how versions are numbered, and what to change when upgrading.
---

Releases are on [GitHub](https://github.com/DataZooDE/anofox-visualization/releases): the extension for Linux, macOS and Windows, the `dashboard` and `serve` CLIs, the web build, a signed `SHA256SUMS`. Versions are CalVer, `YYYY.M.D` (`v2026.11.9`). This site follows the latest release; [/next/](https://datazoode.github.io/anofox-visualization/next/) shows master.

## Upgrade notes

| From | Change |
|---|---|
| **v2026.11.9** | The website is the main page; the builder moved to `/app/` (old builder links forward there). In the browser, `current_date` is run as `today()`. |
| **v2026.11.7** | Agent mode: an agent config needs `agent.bundle` and `agent.digest` (from `dashboard --publish`). |

Each release's notes list every change and the issues it closes.
