// Node test for #500 (web/app.js siteHref): on a published site served
// under a sub-path (GitHub Pages, /repo/), a dashboard's own `/d/<id>[/]
// [?q]` — written against the domain root, exactly like a locked server's
// real route — must resolve to *this* site's own `basePath + "d/" + <id>
// + "/"`, wherever it's served, so a ::LINK or nav href never leaves the
// site. Anything that isn't exactly that shape, isn't `published`, or has
// no site, passes through unchanged: no open redirect, no new behavior on
// a locked server.
//
// No jsdom: siteHref references the free identifiers `window`, `location`
// and `published` (module-level, not parameters) — a minimal stand-in on
// globalThis is enough; `URL` is a Node builtin.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const fn = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const siteHref = new Function(`${fn("siteHref")}\nreturn siteHref;`)();

function withEnv({ published = true, site = { basePath: "/repo/" }, href = "http://example.test/repo/" } = {}, run) {
  const prev = { window: globalThis.window, location: globalThis.location, published: globalThis.published };
  globalThis.window = { __anofoxSite: site };
  globalThis.location = { href, origin: new URL(href).origin };
  globalThis.published = published;
  try {
    run();
  } finally {
    globalThis.window = prev.window;
    globalThis.location = prev.location;
    globalThis.published = prev.published;
  }
}

withEnv({}, () => {
  // The exact shape: /d/<id>, with or without a trailing slash, with or
  // without a query string — rewritten under the site's own basePath, a
  // trailing slash always added (`out/d/<id>/index.html` is the real file).
  assert.equal(siteHref("/d/overview"), "/repo/d/overview/");
  assert.equal(siteHref("/d/overview/"), "/repo/d/overview/");
  assert.equal(siteHref("/d/overview?v.region=east"), "/repo/d/overview/?v.region=east");
  assert.equal(siteHref("/d/overview/?v.region=east"), "/repo/d/overview/?v.region=east");

  // The id is carried exactly as given (still percent-encoded if it was) —
  // never decoded or re-encoded, so it can't come out double-encoded.
  assert.equal(siteHref("/d/a%2Eb"), "/repo/d/a%2Eb/");

  // Not /d/<id> at all: an asset path, the root, a bare "/d/" with no id,
  // a tab fragment, a bare ?v.…, a different same-origin path, an
  // off-origin URL — every one passes through unchanged.
  for (const p of ["/theme-base.css", "/", "/d/", "#Tab", "?v.region=east", "https://example.org/x", "/other/d/overview"]) {
    assert.equal(siteHref(p), p);
  }

  // An extra path segment after the id is not this shape either — only
  // <id> itself, or its own trailing slash, never <id>/sub.
  assert.equal(siteHref("/d/overview/extra"), "/d/overview/extra");

  // No open redirect: a protocol-relative ("//host/…") or backslash-host
  // ("/\host/…") trick resolves, per the URL spec, to a *different origin*
  // — caught by the origin check, not the pathname shape, so it comes back
  // unchanged rather than rewritten under this site's basePath.
  for (const p of ["//evil.example/d/x", "/\\evil.example/d/x", "https://evil.example/d/x"]) {
    assert.equal(siteHref(p), p);
  }
});

// Not `published` (a locked server's own page, or the plain builder
// output opened directly): a no-op — a locked server's /d/<id> is already
// a real route and must never be rewritten.
withEnv({ published: false }, () => {
  assert.equal(siteHref("/d/overview"), "/d/overview");
});

// `published` but no window.__anofoxSite (sitePrepare() hasn't run yet, or
// this isn't a `--site` page): a no-op, never throws.
withEnv({ site: null }, () => {
  assert.equal(siteHref("/d/overview"), "/d/overview");
});

// A malformed path (`new URL()` throws): returned unchanged, never throws.
withEnv({}, () => {
  assert.equal(siteHref("http://"), "http://");
});

console.log("site_href_test: ok");
