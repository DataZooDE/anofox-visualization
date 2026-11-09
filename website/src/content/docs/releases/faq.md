---
title: FAQ and troubleshooting
description: Unsigned extensions, the 421 status, what runs in the browser, and other common questions.
---

**DuckDB refuses to load the release extension.** Release builds are unsigned: start DuckDB with `-unsigned` or `SET allow_unsigned_extensions = true`. The community extension (`INSTALL anofox_visualization FROM community`) is signed, but renders charts only — no servers or builder.

**The server answers `421`.** It serves only requests whose `Host` it knows (DNS-rebinding protection). Behind a proxy, list the public name in `hosts`; in a container keep the published port equal to the server's port, or list `127.0.0.1:<port>`.

**Which anofox extensions run in the browser?** anofox-statistics and anofox-forecast run live (DuckDB-Wasm builds). Tabular, optimize and decide results are precomputed for the browser and run live natively.

**A query works in the CLI but not in the browser.** DuckDB-Wasm cannot reach local paths, sends browser requests (CORS applies to remote files), and has less memory. Read remote files once into a table; use `dashboard --check` locally.

**How do I know a dashboard is right before sharing it?** `dashboard --check file.sql` reports broken SQL, empty panels and design problems; `--strict` fails on warnings in CI.

**Where do I report a bug?** [GitHub issues](https://github.com/DataZooDE/anofox-visualization/issues), with the dashboard's SQL (or a small reproduction) and the version from the builder's footer.
