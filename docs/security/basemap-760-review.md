# Security review: an opt-in tile basemap behind `::MAP` (#760)

Status: review and design, before any code (the issue's precondition).
Date: 2026-11-03.

## What is asked

An optional raster (XYZ) or vector basemap behind a `::MAP`, from a tile or
style URL the **installation** sets (never a dashboard), with its attribution
line. Off by default: the built-in boundary pack keeps maps offline and
private.

## What exists

- ggplot-rs `annotation_map_tile(TileSource, fetcher)` (feature `tiles`):
  draws XYZ raster tiles under `geom_sf` layers in Web Mercator. The crate does
  no networking — a caller-supplied, **synchronous** `TileFetcher` returns the
  tile bytes, which end up as `data:` images in the SVG.
- Maps render in three places: the browser (wasm, builder and served pages),
  the locked server's exports (`/api/export`, native), and the static CLI
  (`dashboard --svg/--site`, native).
- Served-page CSP: `img-src 'self' data: blob: https:`, `connect-src 'self'`
  plus fixed hosts (DuckDB-Wasm CDN, extensions, USGS).

## Threats

| # | Threat | Where |
|---|--------|-------|
| T1 | **Viewer data leaks to the tile host.** Tile requests reveal the map's bounding box and zoom — i.e. *where the data is* (a customer cluster, a depot) — plus the viewer's IP, user agent and the page as Referer. | browser fetching tiles directly |
| T2 | **A dashboard author points maps at a host they control** (tile URL in SQL) to collect viewers' IPs / bounding boxes, or to exfiltrate data encoded in tile paths. | per-dashboard URLs |
| T3 | **CSP widening.** Letting the browser fetch from a tile host needs that host in `img-src`/`connect-src`; `https:` in `img-src` already allows images anywhere, so a direct `<image href>` would work without a CSP change — which is exactly why it must not be used. | browser |
| T4 | **SSRF / open proxy.** A server-side tile proxy that takes a URL, or unbounded `z/x/y`, can be turned against internal hosts or used as a free proxy / amplifier. | server proxy |
| T5 | **Cache poisoning / disk exhaustion.** Cached tiles keyed by request, unbounded cache, a tile host returning non-images or huge bodies. | server cache |
| T6 | **Static outputs phoning home.** A `--site` page or an exported HTML that loads tiles at view time leaks every reader's IP to the host, offline forever after. | static export |
| T7 | **Licence / attribution.** Tile providers require attribution; OSM's public servers forbid heavy use. | all |
| T8 | **Locked-server policy bypass.** Tiles carry no row data; the bounding box comes from data the viewer already sees. No bypass, but the tile proxy must not become a way to probe which areas *other* viewers look at (shared cache timing). | server cache |

## Design (mitigations)

1. **Installation-only source (T2).** The tile URL template lives in the
   server's / CLI's options (`options.basemap`, `--basemap`), never in a
   dashboard. A dashboard only opts in per map (`'tiles'::BASEMAP`); without
   the installation setting the role draws nothing and `--check` notes it.
   The template must be `https://` (or a local directory), contain `{z}`,
   `{x}`, `{y}` and nothing the request can fill.
2. **The browser never contacts the tile host (T1, T3, T6).** Tiles are fetched
   by the server (locked server: a same-origin `GET /api/tiles/{z}/{x}/{y}`;
   CLI: at render time) and embedded as `data:` images. The served page's CSP
   needs no new host. Because the wasm fetcher is synchronous, the page
   prefetches: wasm reports the tiles a map needs (`map_tiles(rows, roles,
   w, h, zoom)` → `[z,x,y]…`), the page fetches them from `/api/tiles`, then
   renders with the bytes passed in. Static `--site` / exported HTML embed the
   tiles at build time; nothing loads at view time.
3. **A narrow proxy (T4).** `/api/tiles/z/x/y` only: integers, `z ≤ max_zoom`
   (default 16), `x, y < 2^z`; the upstream URL is the configured template
   filled with those three numbers — no other input reaches it. Login and the
   credential scope as for `/api/panel`; per-user and global rate limits; only
   `image/png` / `image/jpeg` responses ≤ 512 KB accepted, re-served with
   `nosniff` and `Cache-Control: private`. No redirects followed to other
   hosts.
4. **A bounded cache (T5, T8).** On-disk cache keyed by `z/x/y` of the one
   configured source, LRU by `cache_mb` (default 256), files 0600 in a 0700
   directory (same path rules as `options.state`). Shared across viewers by
   design — tiles are public map imagery; timing reveals at most that *someone*
   on this server looked at an area, which the server's own request log
   already holds. Documented; an installation that cares sets `cache_mb: 0`.
5. **Attribution (T7).** `options.basemap.attribution` is required and drawn
   on every tiled map (ggplot-rs attribution line); the docs name the OSM tile
   usage policy and recommend a self-hosted or commercial tile server.
6. **Defaults.** Off everywhere. The builder (no server) shows `::BASEMAP` as
   a note ("needs a server with options.basemap"), never fetches.

## Not in scope here

Vector basemaps (style JSON + vector tiles) need a vector-tile renderer in
ggplot-rs first (upstream-first rule); raster XYZ covers the issue's first
need.

## Implementation phases (each a PR with this review's checks as tests)

1. Core (no ggplot-rs change needed — its `TileFetcher` is any closure; ggplot-rs
   feature `tiles` has no dependencies): a first render with a *recording*
   fetcher (returns `None`, records `z/x/y`) yields the tile list; the host
   fetches those; a second render takes a fetcher backed by the fetched bytes.
   `::BASEMAP` requires the map's `'mercator'` projection (`src/map/project.rs`)
   and adds `annotation_map_tile` under the map layers in `src/map/plot.rs`.
2. Native: `options.basemap` parse + start checks, the proxy endpoint, the
   cache, CLI `--basemap` (local dir or https), static export embedding.
3. Browser: `::BASEMAP` role, `map_tiles` wasm export, prefetch via
   `/api/tiles`, render with bytes; e2e that asserts no request leaves the
   origin.
