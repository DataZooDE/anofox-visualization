// The embed flow of `serve --dashboards` (pages without script): a one-time
// link is exchanged once; the page session then travels only in the body of
// the page's POSTed form — never in a URL, a history entry or a log line.
import { createRequire } from "module";
const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
import assert from "assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";

const BASE = `http://127.0.0.1:${process.env.BIN_PORT || 8954}`;
const b64u = (b) => Buffer.from(b).toString("base64url");
const h = b64u(JSON.stringify({ alg: "HS256", typ: "JWT" }));
const p = b64u(JSON.stringify({ dashboard: "p", jti: "bin-1", exp: Math.floor(Date.now() / 1000) + 120 }));
const link = `${h}.${p}.${crypto.createHmac("sha256", process.env.EMBED_SECRET).update(`${h}.${p}`).digest("base64url")}`;

const b = await chromium.launch();
const page = await b.newPage();
const urls = [];
page.on("request", (r) => urls.push(r.url()));
page.on("framenavigated", (f) => urls.push(f.url()));
await page.goto(`${BASE}/d/p?embed=${link}`);
assert.match(await page.textContent("body"), /user embed:p channel web/);
const session = await page.getAttribute('input[name="embed_session"]', "value");
assert.ok(session && session.length > 40);
assert.equal(await page.getAttribute("form.controls", "method"), "post");
// change the param: the form posts, the page session in its body
await Promise.all([page.waitForNavigation(), page.selectOption("select[name=channel]", "app")]);
assert.match(await page.textContent("body"), /user embed:p channel app/);
assert.equal(new URL(page.url()).search, "");
// again (a second POST with the same page session), then back in history
await Promise.all([page.waitForNavigation(), page.selectOption("select[name=channel]", "web")]);
assert.match(await page.textContent("body"), /channel web/);
// the one-time link is spent: replaying it is refused
const r = await page.request.get(`${BASE}/d/p?embed=${link}`);
assert.equal(r.status(), 401);
// a GET with the session in the query is no credential either
const q = await page.request.get(`${BASE}/d/p?embed_session=${session}`);
assert.equal(q.status(), 401);
await b.close();
for (const u of urls) assert.ok(!u.includes(session), "session in a URL: " + u);
const log = fs.readFileSync(process.env.WORK + "/bin-req.jsonl", "utf8");
assert.ok(!log.includes(session), "session in the request log");
assert.ok(log.includes('"method":"POST"'));
console.log(`ok  serve --dashboards: one-time link exchanged once, ${urls.length} URLs and ${log.trim().split("\n").length} log lines without the page session, POSTed form only`);
