// End-to-end for the built-in login (#50): OIDC login, an embed link (also in
// a cross-site iframe), API keys, JWT bearer tokens and the audit log, in a
// real browser against the servers run.sh starts. Run it through run.sh.
import { createRequire } from "module";
const require = createRequire(import.meta.url);
const { chromium, request } = require("playwright");
import assert from "assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";

const E = process.env.WORK + "/";
const BASE = `http://localhost:${process.env.PORT || 8950}`;
const IDP = `https://127.0.0.1:${process.env.IDP_PORT || 8953}`;
const EMBED_SECRET = process.env.EMBED_SECRET;
const b64u = (b) => Buffer.from(b).toString("base64url");
const hs256 = (payload, secret = EMBED_SECRET, header = { alg: "HS256", typ: "JWT" }) => {
  const h = b64u(JSON.stringify(header)), p = b64u(JSON.stringify(payload));
  return `${h}.${p}.${crypto.createHmac("sha256", secret).update(`${h}.${p}`).digest("base64url")}`;
};
const now = () => Math.floor(Date.now() / 1000);
const ticks = (p) => p.evaluate(() => [...document.querySelectorAll(".dash .panel-svg svg text")].map((t) => t.textContent).filter((t) => /^(EU|US|APAC)$/.test(t)).sort());
const label = (p) => p.evaluate(() => document.querySelector(".dash")?.innerText.match(/Signed in as[^\n]*/)?.[0]);
const report = [];
const ok = (what) => { report.push("ok  " + what); console.log("ok ", what); };

const b = await chromium.launch();

// ---- 1. OIDC login -------------------------------------------------------
{
  const ctx = await b.newContext({ ignoreHTTPSErrors: true, viewport: { width: 1100, height: 700 } });
  const p = await ctx.newPage(); const errs = [];
  p.on("pageerror", (e) => errs.push(e.message));
  await p.goto(`${BASE}/d/rls?v.channel=web`);
  assert.match(p.url(), /^https:\/\/127\.0\.0\.1:\d+\/authorize\?response_type=code&client_id=dash/);
  const q = new URL(p.url()).searchParams;
  assert.equal(q.get("code_challenge_method"), "S256"); assert.ok(q.get("nonce") && q.get("state"));
  ok("unauthenticated page view → IdP with PKCE S256, state, nonce");
  await p.click("#u-alice");
  await p.waitForSelector(".dash .panel-svg svg", { timeout: 60000 }); await p.waitForTimeout(1500);
  assert.equal(p.url(), `${BASE}/d/rls?v.channel=web`);
  assert.deepEqual(await ticks(p), ["EU"]);
  assert.match(await label(p), /alice@example\.com .*channel web/);
  const c = (await ctx.cookies(BASE)).find((c) => c.name === "__Host-anofox_session");
  assert.ok(c && c.httpOnly && c.secure && c.sameSite === "Lax" && c.path === "/", JSON.stringify(c));
  assert.ok(!(await ctx.cookies(BASE)).some((c) => c.name === "__Host-anofox_oidc" && c.value), "state cookie cleared");
  await p.screenshot({ path: E + "oidc-alice.png" });
  ok("login as alice → back on /d/rls?v.channel=web, rows EU only, session cookie HttpOnly Secure SameSite=Lax __Host-");
  // the viewer can't set claims
  await p.goto(`${BASE}/d/rls?v.__claim_user=bob@example.com&v.__claim_groups=admins`);
  await p.waitForSelector(".dash .panel-svg svg"); await p.waitForTimeout(1200);
  assert.deepEqual(await ticks(p), ["EU"]);
  ok("?v.__claim_user / __claim_groups spoof ignored");
  const sessionCookie = `__Host-anofox_session=${c.value}`;
  // logout (POST via the button), then the old cookie is dead
  await p.goto(`${BASE}/auth/logout`); await p.click("button[type=submit]");
  await p.waitForURL(`${BASE}/auth/logged-out`);
  const api = await request.newContext();
  let r = await api.post(`${BASE}/api/panel`, { headers: { Cookie: sessionCookie, "Content-Type": "application/json" }, data: { dashboard: "rls", panel: 1 } });
  assert.equal(r.status(), 401);
  await p.goto(`${BASE}/d/rls`);
  assert.match(p.url(), /^https:\/\/127\.0\.0\.1:\d+\/authorize/);
  ok("logout revokes the session (replayed cookie → 401, page → login)");
  // a forged callback (no state cookie) and a cross-site logout POST
  r = await api.get(`${BASE}/auth/callback?code=x&state=y`, { maxRedirects: 0 });
  assert.equal(r.status(), 401);
  r = await api.post(`${BASE}/auth/logout`, { headers: { Origin: "https://evil.example" }, maxRedirects: 0 });
  assert.equal(r.status(), 403);
  r = await api.get(`${BASE}/auth/login?next=//evil.example/x`, { maxRedirects: 0 });
  assert.equal(r.status(), 302);
  ok("forged callback → 401; cross-origin logout POST → 403");
  assert.deepEqual(errs, []);
  await ctx.close();
}

// ---- 1b. An unverified e-mail is not an identity (#115) -------------------
{
  const ctx = await b.newContext({ ignoreHTTPSErrors: true });
  const p = await ctx.newPage();
  await p.goto(`${BASE}/d/rls`);
  const [resp] = await Promise.all([p.waitForResponse((r) => r.url().includes("/auth/callback")), p.click("#u-mallory")]);
  assert.equal(resp.status(), 401);
  assert.ok(!(await ctx.cookies(BASE)).some((c) => c.name === "__Host-anofox_session" && c.value));
  ok("an IdP account with alice's unverified e-mail can't sign in as alice (401, no session)");
  await ctx.close();
}

// ---- 2. Embed link -------------------------------------------------------
const link = hs256({ dashboard: "rls", user: "acme-viewer", claims: { tenant: "acme" }, vars: { channel: "web" }, exp: now() + 120, jti: "e2e-1" });
{
  const ctx = await b.newContext({ viewport: { width: 1100, height: 800 } });
  const p = await ctx.newPage(); const errs = [], bad = [], sent = [], assetReqs = [];
  p.on("pageerror", (e) => errs.push(e.message));
  p.on("response", (r) => { if (r.status() >= 400) bad.push(r.status() + " " + r.url()); });
  p.on("request", (r) => {
    if (r.url().endsWith("/api/panel")) sent.push(r.headers()["x-anofox-session"] ? "session" : "none");
    if (r.url().includes("/assets/rls-logo.svg")) assetReqs.push(r.headers()["x-anofox-session"] ? "session" : "none");
  });
  // a cross-site host page with the dashboard in an iframe; the URL tries to
  // widen the scope with ?v.channel=app
  await p.setContent(`<h1>Host app</h1><iframe id="f" src="${BASE}/d/rls?embed=${link}&v.channel=app" width="1000" height="650"></iframe>`);
  const f = await (await p.waitForSelector("#f")).contentFrame();
  await f.waitForSelector(".dash .panel-svg svg", { timeout: 60000 }); await f.waitForTimeout(1500);
  const t = await f.evaluate(() => [...document.querySelectorAll(".dash .panel-svg svg text")].map((t) => t.textContent).filter((t) => /^(EU|US|APAC)$/.test(t)));
  const lab = await f.evaluate(() => document.querySelector(".dash")?.innerText.match(/Signed in as[^\n]*/)?.[0]);
  assert.deepEqual(t, ["EU"]);
  assert.equal(lab, "Signed in as acme-viewer | tenant acme | channel web");
  assert.ok(sent.length > 0 && sent.every((s) => s === "session"), JSON.stringify(sent));
  // #554 review item 4: a relative assets/ ::IMAGE can't be fetched by a
  // bare <img src> in session-header (embed) mode — the browser's own
  // fetch-with-header-then-blob: path (web/app.js) must have run instead.
  await f.waitForFunction(() => { const img = document.querySelector(".dp-image img.dp-img"); return !!img && img.naturalWidth > 0; }, { timeout: 10000 });
  const imgSrc = await f.evaluate(() => document.querySelector(".dp-image img.dp-img").src);
  assert.ok(imgSrc.startsWith("blob:"), imgSrc);
  assert.ok(assetReqs.length > 0 && assetReqs.every((s) => s === "session"), JSON.stringify(assetReqs));
  ok("embed viewer's ::IMAGE (relative assets/rls-logo.svg) loads via a blob: URL fetched with the session header, not a bare <img src> (which would 401)");
  assert.equal((await ctx.cookies()).length, 0, "no cookies for an embed");
  const loc = await f.evaluate(() => location.search);
  assert.ok(!loc.includes("embed=") && loc.includes("v.channel=app"), loc);
  const nav = await f.evaluate(() => window.__served.nav.map((n) => n.id));
  assert.deepEqual(nav, ["rls"]);
  await p.screenshot({ path: E + "embed-iframe.png" });
  assert.deepEqual(bad.filter((x) => !/favicon/.test(x)), []); assert.deepEqual(errs, []);
  ok("embed link in a cross-site iframe: claims from the link, channel locked to web despite ?v.channel=app, page session header on every panel request, no cookies, nav = [rls]");
  // the page session: other dashboards are out of scope; locked vars hold
  const session = await f.evaluate(() => window.__served.session);
  const api = await request.newContext();
  const panel = (body, headers) => api.post(`${BASE}/api/panel`, { headers: { "Content-Type": "application/json", ...headers }, data: body });
  let r = await panel({ dashboard: "other", panel: 0 }, { "X-Anofox-Session": session });
  assert.equal(r.status(), 403);
  r = await panel({ dashboard: "rls", panel: 0, vars: { channel: "app" } }, { "X-Anofox-Session": session });
  assert.equal(r.status(), 200); assert.match(await r.text(), /channel web/);
  r = await api.get(`${BASE}/d/other`, { headers: { "X-Anofox-Session": session } });
  assert.equal(r.status(), 403);
  r = await api.get(`${BASE}/healthz`, { headers: { "X-Anofox-Session": session } });
  assert.deepEqual(await r.json(), { status: "ok" });
  ok("page session: /api/panel for another dashboard → 403, vars.channel=app → still web, /d/other → 403, /healthz → status only; the link is gone from the iframe URL");
  r = await api.get(`${BASE}/d/other?embed=${link}`); assert.equal(r.status(), 401);
  const tampered = link.split(".").map((x, i) => (i === 1 ? b64u(JSON.stringify({ dashboard: "rls", user: "acme-viewer", claims: { tenant: "globex" }, exp: now() + 120 })) : x)).join(".");
  r = await api.get(`${BASE}/d/rls?embed=${tampered}`); assert.equal(r.status(), 401);
  r = await api.get(`${BASE}/d/rls?embed=${hs256({ dashboard: "rls", exp: now() - 120 })}`); assert.equal(r.status(), 401);
  r = await api.get(`${BASE}/d/rls?embed=${hs256({ dashboard: "rls", exp: now() + 86400 })}`); assert.equal(r.status(), 401);
  r = await api.get(`${BASE}/d/rls?embed=${hs256({ dashboard: "rls", exp: now() + 60, vars: { __claim_user: "alice@example.com" } })}`); assert.equal(r.status(), 401);
  r = await api.get(`${BASE}/d/rls?embed=${hs256({ dashboard: "rls", exp: now() + 60 }, "wrong-secret-0123456789abcdef0123456789")}`); assert.equal(r.status(), 401);
  ok("embed: link on another dashboard, tampered, expired, too long-lived, reserved var, wrong secret → 401");
  r = await api.get(`${BASE}/app.js`); assert.equal(r.status(), 200);
  r = await api.get(`${BASE}/m5_monthly.parquet`); assert.equal(r.status(), 401);
  r = await api.get(`${BASE}/`, { maxRedirects: 0 }); assert.equal(r.status(), 302); assert.match(r.headers().location, /^\/auth\/login/);
  ok("UI code public with embed (app.js 200); data files need a credential, a page without one goes to the login");
  await ctx.close();
}

// ---- 2b. S1: a relative ::IMAGE as the *first* statement -----------------
// Review of #554 re-review: the rls.sql case above didn't catch S1 because
// its ::IMAGE is the dashboard's *last* statement. imgfirst.sql puts it
// first, with a panel after it — mkImg runs while the panel is still part
// of the detached newGrid, and the grid only gets swapped into `.dash`
// after every panel's own /api/panel await, so the image's blob: fetch has
// every chance (not just a rare race) to resolve while `img.isConnected`
// is still false. Before the fix this left a blank <img> with no `src` at
// all (and so no `onerror`, not even the alt box); the fix always sets
// `src` once the fetch resolves, detached or not.
{
  const imgLink = hs256({ dashboard: "imgfirst", user: "img-viewer", exp: now() + 120, jti: "e2e-imgfirst" });
  const ctx = await b.newContext({ viewport: { width: 700, height: 500 } });
  const p = await ctx.newPage();
  const errs = [];
  p.on("pageerror", (e) => errs.push(e.message));
  await p.setContent(`<iframe id="f" src="${BASE}/d/imgfirst?embed=${imgLink}" width="650" height="400"></iframe>`);
  const f = await (await p.waitForSelector("#f")).contentFrame();
  await f.waitForSelector(".dash .metric", { timeout: 60000 });
  await f.waitForFunction(
    () => {
      const img = document.querySelector(".dp-image img.dp-img");
      return !!img && img.naturalWidth > 0;
    },
    { timeout: 10000 },
  );
  const imgSrc = await f.evaluate(() => document.querySelector(".dp-image img.dp-img").src);
  assert.ok(imgSrc.startsWith("blob:"), imgSrc);
  const missing = await f.evaluate(() => !!document.querySelector(".dp-img-missing"));
  assert.ok(!missing, "the image-first panel should not have fallen back to the alt box");
  assert.deepEqual(errs, []);
  ok("S1: a relative ::IMAGE as the dashboard's first statement (a later panel's await delays the swap) still loads via blob:, not a blank <img>");
  await ctx.close();
}

// ---- 3. API keys and JWT bearer -----------------------------------------
{
  const api = await request.newContext({ ignoreHTTPSErrors: true });
  const key = fs.readFileSync(E + "apikey.txt", "utf8").trim();
  const revoked = fs.readFileSync(E + "apikey2.txt", "utf8").trim();
  const panel = (auth, dashboard = "rls", n = 1) => api.post(`${BASE}/api/panel`, { headers: { "Content-Type": "application/json", Authorization: auth }, data: { dashboard, panel: n } });
  let r = await panel(`Bearer ${key}`);
  assert.equal(r.status(), 200);
  const rows = JSON.parse(await r.text());
  assert.deepEqual(rows.map((x) => Object.values(x)[0]).sort(), ["US"]);
  r = await panel(`Bearer ${key}`, "other", 0); assert.equal(r.status(), 403);
  r = await panel(`Bearer ${revoked}`); assert.equal(r.status(), 401);
  r = await panel(`Bearer ${key.slice(0, -1)}${key.endsWith("0") ? "1" : "0"}`); assert.equal(r.status(), 401);
  r = await api.get(`${BASE}/healthz`, { headers: { Authorization: `Bearer ${key}` } });
  assert.deepEqual(await r.json(), { status: "ok" });
  ok("API key: rows of svc-etl (US), other dashboard → 403, revoked → 401, wrong secret → 401, scoped key → /healthz status only");
  const { token, jwk } = await (await api.get(`${IDP}/mint?user=alice`)).json();
  r = await panel(`Bearer ${token}`); assert.equal(r.status(), 200);
  assert.deepEqual(JSON.parse(await r.text()).map((x) => Object.values(x)[0]), ["EU"]);
  const wrongAud = (await (await api.get(`${IDP}/mint?user=admin&aud=other`)).json()).token;
  r = await panel(`Bearer ${wrongAud}`); assert.equal(r.status(), 401);
  const expired = (await (await api.get(`${IDP}/mint?user=admin&exp_in=-600`)).json()).token;
  r = await panel(`Bearer ${expired}`); assert.equal(r.status(), 401);
  const payload = { iss: IDP, aud: "dash-api", sub: "root@example.com", groups: ["admins"], exp: now() + 300, iat: now() };
  // algorithm confusion: HS256 with the public JWK as the secret; alg none
  r = await panel(`Bearer ${hs256(payload, JSON.stringify(jwk), { alg: "HS256", kid: "k1" })}`); assert.equal(r.status(), 401);
  r = await panel(`Bearer ${b64u(JSON.stringify({ alg: "none", kid: "k1" }))}.${b64u(JSON.stringify(payload))}.`); assert.equal(r.status(), 401);
  r = await api.get(`${BASE}/healthz`, { headers: { Authorization: `Bearer ${token}` } });
  assert.ok("epoch" in (await r.json()));
  // an ID token of the OIDC login is no API credential
  const idt = (await (await api.get(`${IDP}/mint?user=alice&id_token=1`)).json()).token;
  r = await panel(`Bearer ${idt}`); assert.equal(r.status(), 401);
  ok("JWT bearer: alice → EU (and the full /healthz); wrong aud, expired, HS256-with-public-key, alg none, an ID token → 401");
}

await b.close();

// ---- 4. Audit + request log ----------------------------------------------
await new Promise((r) => setTimeout(r, 300));
const audit = fs.readFileSync(E + "audit.jsonl", "utf8");
const ev = audit.trim().split("\n").map((l) => JSON.parse(l));
const has = (f) => ev.some(f);
assert.ok(has((e) => e.audit === "api_key_created" && e.user === "svc-etl"));
assert.ok(has((e) => e.audit === "api_key_revoked" && e.user === "revoked-svc"));
assert.ok(has((e) => e.audit === "start" && e.detail === "jwt,oidc,embed,api_keys"));
assert.ok(has((e) => e.audit === "login" && e.outcome === "ok" && e.user === "alice@example.com" && e.method === "oidc"));
assert.ok(has((e) => e.audit === "login" && e.outcome === "deny" && e.reason === "no_state"));
assert.ok(has((e) => e.audit === "login" && e.outcome === "deny" && e.reason === "email_unverified"));
assert.ok(has((e) => e.audit === "logout" && e.user === "alice@example.com"));
assert.ok(has((e) => e.audit === "embed" && e.outcome === "ok" && e.user === "acme-viewer" && e.dashboard === "rls"));
for (const m of ["api_key", "jwt", "embed"]) assert.ok(has((e) => e.audit === "auth" && e.outcome === "ok" && e.method === m), "audited success " + m);
for (const reason of ["id_token", "revoked", "unknown_key", "out_of_scope", "bad_signature", "expired", "link_ttl", "bad_vars", "bad_alg", "bad_audience"])
  assert.ok(has((e) => e.audit === "auth" && e.outcome === "deny" && e.reason === reason), "audit reason " + reason);
const key = fs.readFileSync(E + "apikey.txt", "utf8").trim();
for (const secret of [key.slice(17), EMBED_SECRET, process.env.OIDC_SECRET, process.env.SESSION_SECRET, link.split(".")[2]])
  assert.ok(!audit.includes(secret), "audit log leaks a secret");
const reqs = fs.readFileSync(E + "requests.jsonl", "utf8");
for (const m of ["oidc", "embed", "api_key", "jwt"]) assert.match(reqs, new RegExp(`"auth":"${m}"`));
assert.ok(!reqs.includes(key.slice(17)) && !reqs.includes(link.split(".")[2]));
const mode = fs.statSync(E + "audit.jsonl").mode & 0o777;
assert.equal(mode, 0o600);
ok(`audit log: ${ev.length} events (start, key created/revoked, login ok/deny, logout, embed, ${new Set(ev.filter((e) => e.reason).map((e) => e.reason)).size} deny reasons), 0600, no keys/secrets/tokens; request log has auth=oidc|embed|api_key|jwt`);
fs.writeFileSync(E + "report.txt", report.join("\n") + "\n");
console.log("p3 e2e ok");
