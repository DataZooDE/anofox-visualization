# Dashboard visual review

`dashboard --check` validates a dashboard's SQL and structure but **never looks
at the rendered result** — a dashboard can lint clean and still read wrong
(clipped labels, locale-mangled numbers, colliding colours). This is the
appearance gate: render the dashboard to PNG with `scripts/shoot.sh`, then grade
the *image* against the checklist below. It's the visual analogue of
[`dashboard-design.md`](dashboard-design.md) (which covers structure).

## The loop

```
draft.sql → dashboard --check (structure, clean) →
scripts/shoot.sh (render each tab to PNG) →
visual review vs this rubric (a person, or a vision agent) →
fix findings → re-shoot → repeat until clean → commit golden PNGs
```

A vision agent is given the PNGs + this file and returns findings as
`{severity, panel, issue, fix}`. Treat `blocker`/`major` as must-fix.

## Checklist (grade every panel)

**Numbers & text**
1. Numbers are locale-stable and sensible: `$53.8T`, `1.92%`, `4,236` — never
   `53,8 Bio.` or a raw `53815828468224`. Currency/percent where meaningful.
2. Every chart has a title; titles state a takeaway, not just the column.
3. No clipped or truncated labels anywhere (axis ticks, legends, KPI captions,
   table headers). A cut word ("on Technology") is a blocker.

**Axes & labels**
4. Categorical axis labels are legible — rotated or horizontal-barred when many /
   long, never overlapping into a smear.
5. Axes are labelled with units; tick counts are readable (not a dense wall).

**Colour**
6. Every series is a distinguishable colour — no repeated hue within one legend
   (the #1 tell of a too-short palette).
7. ≤ ~7 colours carrying meaning; colour highlights, it doesn't decorate.
8. Colour survives grayscale / colour-blindness (not the only distinction).

**Layout & hierarchy**
9. A clear focal point / reading order; the headline is graspable in ~5s.
10. KPIs read as one compact strip, not scattered cards.
11. Panels aligned to the grid; consistent spacing; no dead whitespace band or
    a half-width panel stranded beside emptiness.
12. Charts sized to their content (2-up comparisons, full-width for time series /
    wide scatter); nothing squashed or needlessly huge.

**Data integrity**
13. No empty/blank panels, `NaN`/`–`/`null` leaking into a chart, or a bar/point
    obviously mis-scaled.
14. Tables paginate rather than dumping hundreds of rows.

## Severity

- **blocker** — clipped labels, colliding colours, locale-broken numbers, an
  empty/broken panel. Ship-stoppers.
- **major** — no hierarchy, unsorted ranking, overlapping ticks, missing titles.
- **minor** — spacing, tick density, wording polish.

Golden baselines for the reference dashboards live next to them (e.g.
`examples/sp500-dashboard/golden/`); regenerate with `scripts/shoot.sh` and
re-review when the renderer or a dashboard changes.
