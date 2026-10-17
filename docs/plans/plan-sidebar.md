# Plan: a sidebar users can manage — their own dashboards, the examples, served folders

Status: **approved by the owner (2026-10-17)**, based on master `cb4701a`.
Implementation follows §6. Supersedes the examples-only "sidebar by business area" proposal
of 2026-10-17 (see the handover note). Owner decisions taken so far:

- **Scope: builder and served.** One metadata model groups dashboards in the
  browser builder *and* in a locked-server folder's nav.
- **Examples: a collapsed library.** My dashboards come first. The examples
  become one "Examples" section grouped by business area, which a user can
  hide. First-time users, who have no dashboards yet, see it open.

## 1. What exists today

| | Where it lives | How it is grouped | What a user can do |
|---|---|---|---|
| **My dashboards** | `localStorage["dp_dashboards"]` = `{items:{name:{sql,group}}, groups, collapsed}`, one browser only (`web/app.js` `dashStore`, `saveDash`) | user groups, kept in the store, not in the SQL | save (overwrites by name without asking), delete, create/rename/delete a group, drag between groups |
| **Examples** | `SAMPLE_GROUPS` (inline) + `FILE_SAMPLES` (`web/examples/*.sql`) | hard-coded group constants; two always-present blocks, "Examples" and "anofox extensions" | open |
| **Served folders** | `*.sql` in one directory, flat (`duckext/src/serve.rs` `load_dashboards`, `serve --dashboards`) | none: a nav bar / `/` list sorted by id | none (authors edit files) |

Gaps: you can't rename, duplicate, search, sort, import or export. Grouping is
lost when a dashboard leaves the browser. The sidebar name and
`::DASHBOARD_TITLE` are two unrelated names. A saved dashboard named like an
example is hidden behind the example in `?dashboard=`. With about 40 examples
the user's own dashboards are easy to lose.

## 2. The metadata model: the SQL file carries its own place

A dashboard declares where it belongs in **header comments**, next to the
existing `-- @title`, `-- @refresh`, `-- @param`, `-- @load` and `-- @vars`
(`src/host/serving.rs` `parse_dashboard_meta`):

```sql
-- @title  Weekly sales by region
-- @group  Sales
-- @tags   weekly, kpi, emea
```

- **`-- @group <name>`**: one group per dashboard. A `/` makes one level of
  nesting (`Sales / EMEA`), shown as a sub-heading. Deeper paths are kept as
  written but drawn flat. No `@group` means ungrouped.
- **`-- @tags a, b`**: free labels used by search and the filter chips.
  Case-insensitive, trimmed, at most 10.
- **`-- @title`** becomes the display name everywhere (sidebar, served nav,
  `/` list). Without it the builder uses the saved name, and the server uses
  the file id, as today.
- Lint (`dashboard --check`) warns on an empty or over-long `@group` (over 80
  characters), more than 10 tags, and a duplicate `@title` in a served folder.

Because the place lives **in the SQL**, it travels with the dashboard: through
Save, Share links, `.sql` export and import, and a served folder. One parser is
shared through wasm, so the builder and the servers read the same thing.

## 3. The builder sidebar

```
＋ New dashboard      🔍 Explore data
[ Search dashboards…        ]   [weekly] [kpi] [×]      ← chips = tags in view
▾ My dashboards (7)                         ⋯ (sort, import, export all)
   ▾ Sales (3)
       Weekly sales by region
       Margin bridge
       …
   ▸ Ops (2)
     Untitled draft
▸ Recently opened (5)
▸ Examples (34) — grouped by business area                 ⋯ Hide examples
     Start here · Demand & stock · Logistics & production · Sales & customers
     Service & operations · Data quality · How it works
```

**My dashboards**
- Each entry has a ⋯ menu: Rename, Duplicate, Move to group, Download `.sql`,
  Delete.
- Delete can be undone: a toast offers *Undo* for about 8 seconds.
- Save no longer overwrites silently. It asks *Replace "X"?* when the name
  belongs to another dashboard.
- Moving a dashboard to a group (drag, as now, or the menu) **rewrites its
  `-- @group` line**, the same way the header-name edit rewrites
  `::DASHBOARD_TITLE` (#69). Empty groups are kept in the store.
- Groups: create, rename (rewrites the member dashboards' `@group`), delete
  (its members become ungrouped), drag to reorder, and collapse.
- Sort: *Manual* (drag), *Name*, *Last edited*. The store records `updated`
  (ms) on each save.
- The header name and `::DASHBOARD_TITLE` become one name: renaming either
  updates both.

**Examples library**
- One section, with the groups by business area from the earlier proposal.
  The feature tours go under *How it works*, renamed as business questions.
  The separate "anofox extensions" block is dropped.
- The section is collapsed once the user has at least one dashboard of their
  own, and open otherwise. *Hide examples* removes it. *Show examples* is in
  the My dashboards ⋯ menu and in the empty state.
- The examples declare their own `@group` and `@tags` in their SQL files. The
  JS group constants go away, so `web/examples/*.sql` is the single source of
  truth.
- "Use as a template" duplicates an example into My dashboards.

**Search and chips**
- The search field filters all sections at once by name, group, tags and
  `@title`. It is also in the phone drawer.
- The tag chips filter by tag. *Recently opened* (the last 5) is per browser.

**Stable addresses**
- Each entry gets an **id**: a slug of the name for examples, and a stored id
  for user dashboards. `?dashboard=` accepts the id.
- A `LEGACY_NAMES` map keeps all current example names working, so old links
  still open.
- Lookup order stays *examples first*, because a shared link has to open the
  same example for everyone. A user dashboard can be addressed unambiguously
  as `?dashboard=my:<id>`.

**Storage.** Dashboards stay in this browser's `localStorage`. Version 2 of
the store is `{v:2, items:{id:{name, sql, updated}}, order, groups,
collapsed, hidden_examples, recent}`. The group is read from the SQL, not
stored twice. v1 is migrated on first load: each item's stored group is
written into its SQL as `-- @group`. A backup copy of v1 is kept as
`dp_dashboards_v1`.

**Import and export** (the way to leave one browser):
- *Download `.sql`* for one dashboard.
- *Export all* writes a `.zip` laid out as a served folder: `<id>.sql` with
  `@title`, `@group` and `@tags`. The zip is uncompressed ("store" method),
  about 60 lines of JS, with no new dependency. So a team can build in the
  browser and serve the folder unchanged with `anofox_serve_dashboards`.
- *Import* takes `.sql` files or a `.zip` (stored entries only; a deflated zip
  is rejected with a clear message) through the file picker or by dropping
  them on the sidebar. On a name clash it asks: *Keep both*, *Replace* or
  *Skip*.

## 4. Served folders (locked server and `serve --dashboards`)

- `parse_dashboard_meta` reads `@group` and `@tags`.
- `window.__served.nav` items become `{id, title, group, tags}`, still filtered
  by the viewer's scope.
- The nav bar groups its links. With more than 8 dashboards it becomes a
  grouped menu with a search field.
- The `/` list page (locked server and CLI) shows group headings and the tags.
- The folder stays **flat**: ids keep `valid_dashboard_id`, and there are no
  subdirectories. That keeps the id, policy and path rules unchanged; grouping
  comes from the metadata alone.
- Policy scopes keep working on ids. A later option could add `"groups": […]`
  scopes in `policy.json`, but it is not part of this plan.

## 5. Out of scope (later, if wanted)

- **Dashboards stored on a server, per user.** Syncing "My dashboards" across
  devices or sharing them in a team needs a writable store with auth on the
  live server. That is a separate feature, built on access control P1–P3. For
  now, export/import and served folders cover moving dashboards around.
- Nested folders deeper than one level, manual ordering inside served
  folders, and favourites/pins. Recents cover most of what pins would do.

## 6. Delivery: 4 PRs, about 3–4 days

| PR | Content | Tests |
|---|---|---|
| **1. Metadata and ids** | `@group` / `@tags` in `parse_dashboard_meta` (shared with wasm) plus the lint; `@group`/`@tags` headers on all `web/examples/*.sql` and the inline samples; a flat entry list `{id, name, group, tags, source}`; `LEGACY_NAMES`; examples rendered as one library by business area; *Hide examples* | Rust unit tests for the parser and lint; a node test that every old example name resolves; showcase-e2e and a11y-e2e on the sidebar |
| **2. Managing my dashboards** | store v2 with migration; ⋯ menu (rename, duplicate, move, delete with undo); Save asks before replacing; `@group` written into the SQL; group reorder; sort; the header name and `::DASHBOARD_TITLE` kept as one | node tests for the migration and the `@group` rewrite; an e2e flow for save → move → rename → reload |
| **3. Search, chips, recents, import/export** | search (also in the drawer), tag chips, recents, `.sql` download, *Export all* as a served-folder `.zip`, import with clash handling | node tests for the zip round-trip; e2e for import/export, including the phone drawer |
| **4. Served grouping** | `nav` with group and tags, a grouped nav bar or menu, grouped `/` pages (locked server and CLI), docs (`DOCS.md` builder and serving sections, README, the dashboards skill) | Rust serve tests (`nav` JSON, scope filtering keeps groups); a served-folder e2e |

PR 1 should land before more examples are added, because new examples would
then come with their own `@group` header.

## 7. Decisions (2026-10-17)

The owner approved the plan ("go with the sidebar"). The open questions were
settled with the proposed defaults; the owner can still redirect them:

1. **Business areas for the examples:** as listed in §3.
2. **One name:** the sidebar name and `::DASHBOARD_TITLE` are merged into
   one name.
3. **Server-side "My dashboards"** stays out of scope (§5). The v2 store sits
   behind a small storage interface (`list / get / put / remove`), so a
   server backend can replace `localStorage` later.
