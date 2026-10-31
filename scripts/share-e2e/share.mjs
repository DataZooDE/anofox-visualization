// End-to-end for share links (#763) in a real browser against the locked
// server run.sh starts: a public dashboard opens without any login, a
// password-protected one after its password, each as its own `share:<id>`
// identity with its claims, and nothing else of the server opens.
import { createRequire } from "module";
const require = createRequire(import.meta.url);
const { chromium, request } = require("playwright");
import assert from "assert/strict";
import fs from "node:fs";

const W = process.env.WORK + "/";
const BASE = `http://localhost:${process.env.PORT || 8956}`;
const PASSWORD = process.env.PASSWORD;
const ok = (what) => console.log("ok ", what);
const ticks = (p) => p.evaluate(() => [...document.querySelectorAll(".dash .panel-svg svg text")].map((t) => t.textContent).filter((t) => /^(EU|US|APAC)$/.test(t)).sort());
const text = (p) => p.evaluate(() => document.querySelector(".dash")?.innerText || "");

const b = await chromium.launch();
try {
  // 1. a public dashboard: no login, its own identity
  {
    const ctx = await b.newContext({ viewport: { width: 1100, height: 700 } });
    const p = await ctx.newPage();
    const errs = [];
    p.on("pageerror", (e) => errs.push(e.message));
    await p.goto(`${BASE}/d/other`);
    await p.waitForFunction(() => /other dashboard for/.test(document.querySelector(".dash")?.innerText || ""), null, { timeout: 60000 });
    assert.match(await text(p), /other dashboard for share:other/);
    assert.deepEqual(errs, []);
    ok("public dashboard opens without a login, as share:other");
    await p.screenshot({ path: W + "public.png" });
    await ctx.close();
  }
  // 2. nothing else of the server
  const api = await request.newContext();
  for (const path of ["/", "/d/nope"]) {
    const r = await api.get(BASE + path, { maxRedirects: 0 });
    assert.equal(r.status(), 401, path);
  }
  let r = await api.post(`${BASE}/api/panel`, { headers: { "Content-Type": "application/json" }, data: { dashboard: "other", panel: 0 } });
  assert.equal(r.status(), 401);
  r = await api.get(`${BASE}/healthz`);
  assert.deepEqual(await r.json(), { status: "ok" });
  ok("the list page, unknown dashboards and the API without a page session: 401; /healthz: status only");
  // 3. the password dashboard
  {
    const ctx = await b.newContext({ viewport: { width: 1100, height: 700 } });
    const p = await ctx.newPage();
    await p.goto(`${BASE}/d/rls?v.channel=web`);
    await p.waitForSelector('form[action="/share/rls"] input[type=password]');
    await p.screenshot({ path: W + "password.png" });
    await p.fill("input[type=password]", "not it");
    await p.click("button[type=submit]");
    await p.waitForSelector("[role=alert]");
    assert.match(await p.textContent("[role=alert]"), /not right/);
    ok("password page; a wrong password: told so, still the form");
    await p.fill("input[type=password]", PASSWORD);
    await p.click("button[type=submit]");
    await p.waitForSelector(".dash .panel-svg svg", { timeout: 60000 });
    await p.waitForTimeout(1200);
    assert.equal(p.url(), `${BASE}/d/rls?v.channel=web`);
    assert.deepEqual(await ticks(p), ["EU"]);
    assert.match(await text(p), /Signed in as share:rls \| tenant acme \| channel web/);
    const c = (await ctx.cookies(`${BASE}/d/rls`)).find((c) => c.name === "__Secure-anofox_share");
    assert.ok(!(await ctx.cookies(`${BASE}/`)).some((c) => c.name === "__Secure-anofox_share"), "not sent outside /d/rls");
    assert.ok(c && c.httpOnly && c.secure && c.sameSite === "Lax" && c.path === "/d/rls", JSON.stringify(c));
    ok("right password → back on /d/rls?v.channel=web as share:rls (tenant acme), rows EU only; cookie HttpOnly Secure Lax Path=/d/rls");
    // the image asset loads through the page session
    await p.waitForFunction(() => [...document.querySelectorAll(".dash img")].some((i) => i.complete && i.naturalWidth > 0), null, { timeout: 15000 });
    ok("the dashboard's assets/ image loads");
    await p.screenshot({ path: W + "rls.png" });
    // the cookie is no login for anything else
    await p.goto(`${BASE}/d/other`);
    await p.waitForFunction(() => /other dashboard for/.test(document.querySelector(".dash")?.innerText || ""), null, { timeout: 60000 });
    const g = await p.request.get(`${BASE}/`, { maxRedirects: 0 });
    assert.equal(g.status(), 401);
    // a reload goes straight to the dashboard
    await p.goto(`${BASE}/d/rls`);
    await p.waitForSelector(".dash .panel-svg svg", { timeout: 60000 });
    ok("cookie: no login for /, a reload of /d/rls skips the password page");
    await ctx.close();
  }
  // 4. a locked-out address
  {
    const ctx = await b.newContext();
    const p = await ctx.newPage();
    let last = 0;
    for (let i = 0; i < 11; i++) {
      const res = await p.request.post(`${BASE}/share/rls`, { form: { password: "wrong" }, headers: { Origin: BASE } });
      last = res.status();
    }
    assert.equal(last, 429);
    const res = await p.request.post(`${BASE}/share/rls`, { form: { password: PASSWORD }, headers: { Origin: BASE } });
    assert.equal(res.status(), 429);
    assert.ok(res.headers()["retry-after"]);
    const x = await p.request.post(`${BASE}/share/rls`, { form: { password: PASSWORD }, headers: { Origin: "https://evil.example" } });
    assert.equal(x.status(), 403);
    ok("after 10 wrong passwords from one address: 429 with Retry-After, even for the right one; a cross-origin post: 403");
    await ctx.close();
  }
  // 5. the audit log
  const log = fs.readFileSync(W + "audit.jsonl", "utf8");
  assert.match(log, /"audit":"share_login","outcome":"ok"/);
  assert.match(log, /"reason":"wrong_password"/);
  assert.match(log, /"reason":"locked_out"/);
  assert.ok(!log.includes(PASSWORD) && !log.includes("not it"));
  ok("audit log: logins, wrong passwords and lockouts, never a password");
} finally {
  await b.close();
}
