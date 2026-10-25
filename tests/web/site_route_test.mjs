// Node test for #417 (web/app.js siteDashId): the pure part of a published
// site's in-place /d/<id> router — does a path belong to this site, and
// which dashboard id does it name? siteGo()/sitePrepare() themselves need a
// real DOM + fetch + DuckDB-Wasm, covered by this PR's manual end-to-end
// check (a real site built with `dashboard --site`, served under a
// sub-path, compared side by side with the same folder on the locked
// server) rather than by a unit test here.
//
// No jsdom: siteDashId (and, since #500, the siteHref() it now calls
// first) reference the free identifiers `window`, `location` and
// `published` (not parameters — unlike pathVarValue/servedNavHref in
// nav_path_vars_test.mjs, which close over module-level state through
// explicit factory parameters instead). Extracted with `new Function`, its
// body runs against the *global* scope, so a minimal `window`/`location`/
// `published` stand-in set directly on `globalThis` is enough; `URL` is a
// Node builtin.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const fn = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
// siteDashId calls siteHref() (#500) — both must be defined in the same
// Function body.
const siteDashId = new Function(`${fn("siteHref")}\n${fn("siteDashId")}\nreturn siteDashId;`)();

function withSite(site, run, { published = true } = {}) {
  const prevWindow = globalThis.window;
  const prevLocation = globalThis.location;
  const prevPublished = globalThis.published;
  globalThis.window = { __anofoxSite: site };
  globalThis.location = { href: "http://example.test/repo/", origin: "http://example.test" };
  globalThis.published = published;
  try {
    run();
  } finally {
    globalThis.window = prevWindow;
    globalThis.location = prevLocation;
    globalThis.published = prevPublished;
  }
}

const site = { basePath: "/repo/", home: "overview" };

// No window.__anofoxSite at all (a locked server's own page, or the plain
// builder): siteDashId is never wired up there, but if it were called,
// it must not throw — null, so a caller can fall back to a real navigation.
withSite(null, () => {
  assert.equal(siteDashId("/repo/d/overview"), null);
});

withSite(site, () => {
  // The site's own root — the home dashboard (with or without a trailing
  // index.html, as a relative asset reference might spell it).
  assert.equal(siteDashId("/repo/"), "overview");
  assert.equal(siteDashId("/repo/index.html"), "overview");

  // A dashboard id, with or without a trailing slash (the launcher's own
  // directory) or /index.html.
  assert.equal(siteDashId("/repo/d/by-product"), "by-product");
  assert.equal(siteDashId("/repo/d/by-product/"), "by-product");
  assert.equal(siteDashId("/repo/d/by-product/index.html"), "by-product");

  // A query string / hash on the path don't change the id (URL parses them
  // off); siteGo() reads the query itself, separately.
  assert.equal(siteDashId("/repo/d/by-product?v.region=east"), "by-product");
  assert.equal(siteDashId("/repo/d/by-product#Tab"), "by-product");

  // A percent-encoded id round-trips (a dashboard id that happens to need
  // encoding — valid_dashboard_id allows '.', '-', '_' unencoded, but the
  // router must not assume that).
  assert.equal(siteDashId("/repo/d/a%2Eb"), "a.b");

  // Outside this site's own base path: not this site's router's business —
  // null, same as "no site at all" (followLink/the popstate handler then
  // leave it to a real navigation).
  assert.equal(siteDashId("/other/d/overview"), null);
  assert.equal(siteDashId("https://evil.example/repo/d/overview"), null);

  // A path under the site but not /d/<id> and not the root (an asset, a
  // mistyped link): no id to switch to.
  assert.equal(siteDashId("/repo/theme-base.css"), null);
  assert.equal(siteDashId("/repo/d/"), null);
});

// #500: a root-relative `/d/<id>` — exactly what a ::LINK or served-nav
// href is written as, same as on a locked server — is read through
// siteHref() first, so it resolves to this site's own dashboard even
// though it isn't (yet) under `/repo/`.
withSite(site, () => {
  assert.equal(siteDashId("/d/by-product"), "by-product");
  assert.equal(siteDashId("/d/by-product/"), "by-product");
  assert.equal(siteDashId("/d/by-product?v.region=east"), "by-product");

  // Still scoped to this site: a cross-origin `/d/<id>` isn't rewritten
  // into it (siteHref's own origin check), so it falls through to null
  // same as before #500.
  assert.equal(siteDashId("https://evil.example/d/overview"), null);
});

// Not `published` (a locked server's own page): siteHref() is a no-op, so
// a root-relative `/d/<id>` is left alone — outside this site's basePath,
// same as "no site at all". siteDashId isn't actually reachable here in
// the real app (every caller guards on `published` first), but it must
// still degrade safely, not rewrite, if it were.
withSite(
  site,
  () => {
    assert.equal(siteDashId("/d/by-product"), null);
  },
  { published: false }
);

console.log("site_route_test: ok");
