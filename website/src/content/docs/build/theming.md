---
title: Themes and branding
description: One theme styles the charts, the page and every export; built-ins, your own JSON theme, a logo and a locale.
---

One theme styles the whole dashboard: the charts (palette, fills, ink), the page (header, cards, tabs, tiles, tables, inputs) and every output — the browser, a locked server, the CLI's HTML and SVG, and the PNG / HTML exports.

```sql
SELECT 'dark'::THEME;                                      -- built-in: light, dark, print, colorblind, high-contrast
SELECT 'acme+dark'::THEME;                                 -- your theme with a dark variant
SELECT content::THEME FROM read_text('themes/acme.json');  -- a corporate theme from a file
SELECT 'de-DE'::LOCALE;                                    -- numbers and dates: 1.234,5 · 7. Nov. 2026
SELECT 'Sales cockpit'::DASHBOARD_TITLE;
SELECT 'ACME Analytics'::BRAND;
SELECT 'assets/acme.svg'::LOGO;
```

A theme file sets colours (brand, palette, sequential and diverging ramps, positive / negative, the chart's grid and axis apart), fonts and the page chrome. `"lift": true` raises brand colours that are hard to see on the card to WCAG contrast. A viewer's dark-mode preference picks the dark variant. A locked server can set one theme for every dashboard and add an operator stylesheet (`::CLASS` hooks into it).

Locales: `en-US`, `en-GB`, `en-IE` (English with €), `de-DE`, `fr-FR`, and from CLDR `de-AT`, `de-CH`, `fr-BE`, `fr-CH`, `fr-CA`, `it-IT`, `it-CH`, `es-ES`, `es-MX`, `nl-NL`, `nl-BE`, `pt-PT`, `pt-BR`, `pl-PL`, `sv-SE`, `da-DK`, `nb-NO`, `fi-FI`, `cs-CZ`; another region takes its language's main locale (`de-LU` → de-DE), an unknown language shows as en-US. A directive may read a variable, so the page can follow the viewer: `coalesce(nullif(getvariable('locale'), ''), 'de-DE')::LOCALE` with `-- @vars locale` takes `?v.locale=en-US` from the page URL or an embed's `setVars`. Axis and value formats: `'€'::YFORMAT`, `'percent'::YFORMAT`, `'%b %Y'::XFORMAT`, and `::MONEY` / `::COMPACT` on tiles and table columns.

[Theme reference →](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/DOCS.md#themes-dark-mode-branding-and-locale) · [Accessibility →](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/DOCS.md#accessibility)
