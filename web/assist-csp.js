// The assistant's tab-wide CSP (#204): a meta tag (static hosting sends no
// headers), inserted once a key is stored (app.js boot()) or when the
// assistant panel opens (M1-3). It is accepted only after a positive
// enforcement probe (L-4/I-R2-... below), because a meta outside <head>, a
// malformed policy or a browser that ignores a dynamically inserted meta
// must all fail closed: no assistant opens, and app.js clears a stored key.
//
// A pure ES module (no DOM beyond what's passed in): tests/web/assist_csp_test.mjs
// drives it with a fake `document`.
//
// No network calls, no registerFileURL, no OPFS, no document.baseURI and no
// dynamic module loading here (a static grep test; see
// tests/web/assist_csp_test.mjs).

export const DUCKDB_WASM = "https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.33.1-dev65.0/";
export const KATEX = "https://cdn.jsdelivr.net/npm/katex@0.19.0/";
export const DUCKDB_EXT = "https://extensions.duckdb.org/v1.5.6/";
export const USGS = "https://earthquake.usgs.gov/earthquakes/feed/";
export const API = "https://api.anthropic.com";

// L-3: the app's own directory (e.g. https://datazoode.github.io/anofox-visualization/),
// never 'self' — every DataZooDE Pages site shares the datazoode.github.io
// origin, so 'self' there is the whole shared origin, not this app.
// I-R2-3(b): this is only correct while assist-csp.js sits in the app's own
// root directory (next to index.html); tests/web/assist_csp_test.mjs checks
// that appBase() equals the directory index.html is served from, so a later
// move (e.g. into web/assist/) fails that test instead of silently breaking
// every fetch the app makes.
export const appBase = (u = import.meta.url) => new URL("./", u).href;

// A probe the CSP must refuse: a host that can never resolve (.invalid,
// RFC 2606), so a browser that doesn't enforce the meta leaks no real data.
const PROBE_HOST = "assist-csp-probe.invalid";
const PROBE = `https://${PROBE_HOST}/p.png`;

// A single https:// origin — the app's own imgOrigins never widen past this.
const ORIGIN_RE = /^https:\/\/[a-z0-9.-]+(:\d+)?$/i;
// I-R2-3(a): a full URL (scheme + host + path, no query/fragment) is enough
// and narrower than granting the whole origin — accepted too, so a brand
// logo/favicon can be listed by its exact address.
const FULL_URL_RE = /^https:\/\/[a-z0-9.-]+(:\d+)?\/[^\s"'<>]*$/i;

export function assistCsp({ base = appBase(), imgOrigins = [] } = {}) {
  const img = [base, "data:", "blob:", ...imgOrigins.filter((o) => ORIGIN_RE.test(o) || FULL_URL_RE.test(o))];
  return [
    "default-src 'none'",
    `script-src ${base} 'wasm-unsafe-eval' ${DUCKDB_WASM} ${KATEX}`,
    "worker-src blob:",
    `connect-src ${base} ${API} ${DUCKDB_WASM} ${DUCKDB_EXT} ${USGS}`,
    `img-src ${img.join(" ")}`,
    `style-src ${base} 'unsafe-inline' https://fonts.googleapis.com ${KATEX}`,
    `font-src ${base} data: https://fonts.gstatic.com ${KATEX}`,
    `manifest-src ${base}`,
    "media-src 'none'",
    "object-src 'none'",
    "frame-src 'none'",
    "base-uri 'none'",
    "form-action 'none'",
  ].join("; ");
}

// Per-document memoised enforcement proofs (one `applyAssistCsp` per
// document; a second caller — boot() and the M1-3 open handler share one —
// gets the same promise: C-2).
const proofs = new WeakMap();

// Insert the meta once, then prove it: a probe <img> to a host that can
// never resolve must raise an img-src `securitypolicyviolation` within 1 s.
// Anything else (no <head>, the meta ignored, appended outside <head>, a
// detached element) rejects, and callers must treat that as "not available
// in this browser" rather than proceed with an unenforced policy.
export function applyAssistCsp(doc, opts = {}) {
  if (proofs.has(doc)) return proofs.get(doc);
  const p = (async () => {
    if (!doc.head) throw new Error("no <head>");
    const m = doc.createElement("meta");
    m.httpEquiv = "Content-Security-Policy";
    m.content = assistCsp(opts);
    m.dataset.anofoxAssist = "1";
    doc.head.appendChild(m);
    await new Promise((ok, fail) => {
      const t = setTimeout(() => {
        doc.removeEventListener("securitypolicyviolation", on);
        fail(new Error("the CSP is not enforced in this browser"));
      }, 1000);
      function on(e) {
        if (e.effectiveDirective === "img-src" && String(e.blockedURI).startsWith(`https://${PROBE_HOST}`)) {
          clearTimeout(t);
          doc.removeEventListener("securitypolicyviolation", on);
          ok();
        }
      }
      doc.addEventListener("securitypolicyviolation", on);
      const img = doc.createElement("img");
      img.src = PROBE + "?" + Date.now();
    });
  })();
  proofs.set(doc, p);
  p.catch(() => {}); // callers await it themselves; no unhandled rejection here
  return p;
}

// Resolves/rejects exactly as the memoised `applyAssistCsp` call did, for a
// caller (buildEngine) that must refuse to proceed without a proven CSP but
// never triggers the insertion itself.
export const assistCspProven = (doc) => proofs.get(doc) || Promise.reject(new Error("the CSP is not applied"));
