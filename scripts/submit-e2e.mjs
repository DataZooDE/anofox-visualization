// `::SUBMIT` (#517 S4, plan §6) in a real browser, against a real locked
// server (options.state, proxy auth): the button renders with its own
// label; clicking it disables the button (aria-busy/aria-disabled), posts
// /api/submit, then shows "Submitted at …" (+ the ::SUBMIT_DONE text),
// re-enables the button, keeps focus on it, and the rest of the page
// re-runs (a viewer_state count panel goes up); a dashboard without
// options.state renders the button disabled with a visible, read-out
// reason; a crafted 400/429 response (page.route) shows the plan's own
// error text and re-enables (429: after Retry-After). Axe on both themes;
// no horizontal overflow at 390px. Review of #550: a network failure
// *after* the POST actually landed (route.fetch(), then abort) doesn't
// replay a stale answer once the viewer changes it before retrying (SF1);
// the plain wasm builder's preview submit works locally (bound params);
// a route()-mocked live-authoring backend never gets a viewer_state
// CREATE/INSERT, and the button shows disabled with that reason (SF2).
// See duckext/scripts/test-state.sh for the server-side isolation/
// trainer/cache coverage this doesn't repeat.
//
//   NODE_PATH=<dir with playwright>/node_modules node scripts/submit-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8931] \
//       [--duckdb duckdb] [--axe axe.min.js] [--shots DIR]
import { createRequire } from "module";
import { spawn } from "child_process";
import { mkdtempSync, writeFileSync, mkdirSync, existsSync } from "fs";
import { tmpdir } from "os";
import path from "path";
import crypto from "crypto";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8931"));
const duckdb = arg("--duckdb", "duckdb");
const axe = arg("--axe", "");
const shots = arg("--shots", "");
if (!ext || !existsSync(ext)) {
  console.error("usage: submit-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
if (!/^anofox_visualization(\..+)?\.duckdb_extension$/.test(path.basename(ext))) {
  console.error(`--ext ${ext}: must be named anofox_visualization.duckdb_extension (or anofox_visualization.<x>.duckdb_extension)`);
  process.exit(2);
}
if (shots) mkdirSync(shots, { recursive: true });
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
// Scratch lives next to the extension, not under the system temp dir
// (options.state.path refuses it, plan §1).
const W = mkdtempSync(path.join(path.dirname(ext), "submit-e2e-"));
const dashDir = path.join(W, "dash");
mkdirSync(dashDir, { recursive: true });
writeFileSync(
  path.join(dashDir, "quiz.sql"),
  "SELECT q1 ::BUTTONS FROM (VALUES ('A'), ('B'), ('C')) t(q1);\n" +
    "SELECT 'Check my answer' AS quiz_1::SUBMIT, 'q1' AS r::SUBMIT_VARS, 'Saved — thanks!' AS d::SUBMIT_DONE;\n" +
    "SELECT count(*) AS n ::METRIC FROM viewer_state;\n"
);
writeFileSync(
  path.join(dashDir, "noState.sql"),
  "SELECT q1 ::BUTTONS FROM (VALUES ('A'), ('B')) t(q1);\n" +
    "SELECT 'Check my answer' AS quiz_1::SUBMIT, 'q1' AS r::SUBMIT_VARS;\n"
);
const sec = crypto.randomBytes(24).toString("hex");
const statePath = path.join(W, "state.jsonl");
const opts = {
  auth: { mode: "proxy", shared_secret: sec, trusted_proxies: ["127.0.0.1"] },
  policy: "none",
  state: { path: statePath, dashboards: ["quiz"] },
};
async function serve() {
  const p = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"] });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  p.stdin.write(`LOAD ${sqlStr(ext)};\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(JSON.stringify(opts))});\n`);
  for (let i = 0; i < 100; i++) {
    try {
      if ((await fetch(`http://127.0.0.1:${port}/healthz`)).ok) return { proc: p, log: () => log };
    } catch (_) {}
    if (p.exitCode != null) break;
    await sleep(100);
  }
  p.kill();
  throw new Error(`the server did not start:\n${log}`);
}
const stop = (s) => {
  s.proc.stdin.end();
  s.proc.kill();
};

const browser = await chromium.launch();
async function openAs(user, theme, width = 1440, bypassCSP = false) {
  const ctx = await browser.newContext({
    viewport: { width, height: 900 },
    reducedMotion: "reduce",
    colorScheme: theme === "dark" ? "dark" : "light",
    // Only for the axe check below: addScriptTag injects axe.min.js as a
    // plain inline <script>, which the served page's own strict CSP
    // (script-src with a per-response nonce) refuses without this —
    // a Playwright-only bypass, not a change to what the real page sends.
    bypassCSP,
  });
  await ctx.setExtraHTTPHeaders({ "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": user });
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`http://127.0.0.1:${port}/d/quiz${theme ? "?theme=" + theme : ""}`);
  await page.waitForSelector(".submit-btn");
  return { ctx, page, errs };
}
const metricN = (page) => page.locator(".metric-value").first().innerText();
async function openAsPage(dash, user) {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, reducedMotion: "reduce" });
  await ctx.setExtraHTTPHeaders({ "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": user });
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`http://127.0.0.1:${port}/d/${dash}`);
  await page.waitForSelector(".submit-btn");
  return { ctx, page, errs };
}

const srv = await serve();
try {
  // (a) the button renders with the author's own label.
  {
    const { page, errs, ctx } = await openAs("alice", "light");
    check((await page.locator(".submit-btn").innerText()) === "Check my answer", "(a) the button carries the author's own label");
    check(!(await page.locator(".submit-error").first().isVisible()), "(a) no error shown before any click");
    check(!errs.length, `(a) no page errors: ${errs.join(" | ")}`);
    await ctx.close();
  }

  // (b) click -> disabled while in flight is too fast to catch reliably
  // here (same-host fetch), but the end state is: success text, button
  // re-enabled, focus on it, and the viewer_state count panel moved.
  {
    const { page, errs, ctx } = await openAs("bob", "light");
    const n0 = await metricN(page);
    await page.locator(".submit-btn").click();
    await page.waitForFunction(() => document.querySelector(".submit-status")?.textContent?.includes("Submitted at"), null, { timeout: 15000 });
    const status = await page.locator(".submit-status").innerText();
    check(/^Submitted at \d{1,2}:\d{2}/.test(status), `(b) the status reads "Submitted at HH:MM", got: ${status}`);
    check(status.includes("Saved — thanks!"), "(b) the ::SUBMIT_DONE text is appended");
    check(!(await page.locator(".submit-btn").isDisabled()), "(b) the button is enabled again");
    check((await page.locator(".submit-btn").getAttribute("aria-busy")) === null, "(b) aria-busy is cleared");
    const focused = await page.evaluate(() => document.activeElement?.classList.contains("submit-btn"));
    check(focused, "(b) focus stays on the button");
    await page.waitForFunction((n0) => document.querySelector(".metric-value")?.textContent !== n0, n0, { timeout: 15000 });
    const n1 = await metricN(page);
    check(n0 !== n1, `(b) the viewer_state count panel re-ran (was ${n0}, now ${n1})`);
    check(!errs.length, `(b) no page errors: ${errs.join(" | ")}`);
    await ctx.close();
  }

  // (c) re-submitting appends a new row (no server-side "once" gate here).
  {
    const { page, errs, ctx } = await openAs("carol", "light");
    for (let k = 0; k < 2; k++) {
      await page.locator(".submit-btn").click();
      await page.waitForFunction(() => document.querySelector(".submit-status")?.textContent?.includes("Submitted at"), null, { timeout: 15000 });
      await page.waitForFunction(() => !document.querySelector(".submit-btn")?.disabled, null, { timeout: 15000 });
    }
    check((await metricN(page)) === "2", `(c) two submissions from one viewer both recorded, got ${await metricN(page)}`);
    check(!errs.length, `(c) no page errors: ${errs.join(" | ")}`);
    await ctx.close();
  }

  // (d) a 400 the server would actually send (an answer outside the
  // options — route() crafts it so this doesn't depend on timing a real
  // validation failure): the plain server text shows in the alert region,
  // the button re-enables.
  {
    const { page, errs, ctx } = await openAs("dana", "light");
    await page.route("**/api/submit", (route) => route.fulfill({ status: 400, body: "'q1' is not one of the options" }));
    await page.locator(".submit-btn").click();
    await page.waitForFunction(() => document.querySelector(".submit-error")?.textContent?.includes("not one of the options"), null, { timeout: 15000 });
    check(true, "(d) the server's own 400 text shows in the alert region");
    check(!(await page.locator(".submit-btn").isDisabled()), "(d) the button re-enables after a 400");
    check(!errs.length, `(d) no page errors: ${errs.join(" | ")}`);
    await ctx.close();
  }

  // (d2) #551: a 400 whose JSON body names one of this dashboard's own
  // variables (`{"error","var"}`, duckext serve.rs `reply_invalid_var`) —
  // that input's own control gets `aria-invalid`, and the error line
  // carries a focusable link to it; picking a different answer (a real
  // value change, not another submit) clears the marking on its own,
  // before any further click.
  {
    const { page, errs, ctx } = await openAs("dana2", "light");
    await page.route("**/api/submit", (route) =>
      route.fulfill({ status: 400, contentType: "application/json", body: JSON.stringify({ error: "'q1' is not one of the options", var: "q1" }) })
    );
    await page.locator(".submit-btn").click();
    await page.waitForFunction(() => document.querySelector(".submit-error")?.textContent?.includes("not one of the options"), null, { timeout: 15000 });
    const seg = page.locator(".dp-seg button[aria-checked='true']").first();
    check((await seg.getAttribute("aria-invalid")) === "true", "(d2) the named input's control is aria-invalid");
    const link = page.locator(".submit-error-link");
    check((await link.count()) === 1, "(d2) the error line carries a focusable link");
    if (shots) await page.screenshot({ path: `${shots}/submit-invalid-var-light-1440.png` });
    await link.click();
    check(await seg.evaluate((el) => el === document.activeElement), "(d2) the link focuses the named input");
    // Picking a different option is a real change — the panel re-renders
    // (a fresh DOM), and `mkSubmit`'s own restore no longer reapplies
    // aria-invalid once the snapshotted bad value no longer matches.
    await page.locator('.dp-seg button:text-is("B")').click();
    await page.waitForFunction(() => !document.querySelector("[aria-invalid='true']"), null, { timeout: 5000 });
    check(true, "(d2) aria-invalid is cleared once the answer actually changes");
    await page.unroute("**/api/submit");
    check(!errs.length, `(d2) no page errors: ${errs.join(" | ")}`);
    await ctx.close();
  }

  // (e) a 429 with Retry-After: "too many submissions" text, the button
  // stays disabled until the wait elapses.
  {
    const { page, errs, ctx } = await openAs("erin", "light");
    await page.route("**/api/submit", (route) => route.fulfill({ status: 429, headers: { "Retry-After": "1" }, body: "too many submissions" }));
    await page.locator(".submit-btn").click();
    await page.waitForFunction(() => document.querySelector(".submit-error")?.textContent?.includes("Too many submissions"), null, { timeout: 15000 });
    check(await page.locator(".submit-btn").isDisabled(), "(e) the button stays disabled right after a 429");
    await page.unroute("**/api/submit");
    await page.waitForFunction(() => !document.querySelector(".submit-btn")?.disabled, null, { timeout: 5000 });
    check(true, "(e) the button re-enables once Retry-After elapses");
    check(!errs.length, `(e) no page errors: ${errs.join(" | ")}`);
    await ctx.close();
  }

  // (e2) review of #550 SF1: a network failure *after* the POST actually
  // landed (route.fetch() really sends it, then the client is told the
  // request failed) must not silently replay a stale answer on retry once
  // the viewer has changed it — the retry id is only reused while the
  // vars are unchanged. route.fetch() really submits q1=A (recorded
  // server-side); the client sees only a network error; changing the
  // answer to B and retrying must append a *second*, distinct row (B), not
  // a replay of the first (A)'s cached reply.
  {
    const { page, errs, ctx } = await openAs("ivy", "light");
    let first = true;
    await page.route("**/api/submit", async (route) => {
      if (first) {
        first = false;
        await route.fetch(); // really reaches the server; the row for A is recorded
        await route.abort("failed"); // but the client never sees that response
      } else {
        await route.continue();
      }
    });
    await page.locator(".submit-btn").click();
    await page.waitForFunction(() => document.querySelector(".submit-error")?.textContent?.includes("Couldn't save"), null, { timeout: 15000 });
    await page.locator('.dp-seg button:text-is("B")').click(); // change the answer before retrying
    await page.locator(".submit-btn").click();
    await page.waitForFunction(() => document.querySelector(".submit-status")?.textContent?.includes("Submitted at"), null, { timeout: 15000 });
    await page.waitForFunction(() => document.querySelector(".metric-value")?.textContent === "2", null, { timeout: 15000 });
    check((await metricN(page)) === "2", `(e2) both the landed-but-lost A and the retried B are recorded, got ${await metricN(page)}`);
    check(!errs.length, `(e2) no page errors: ${errs.join(" | ")}`);
    await ctx.close();
  }

  // (f) a dashboard without options.state: the button renders disabled,
  // with a visible, read-out (aria-describedby) reason — never silent.
  {
    const { page, errs, ctx } = await openAsPage("noState", "frank");
    check(await page.locator(".submit-btn").isDisabled(), "(f) the button is disabled when state isn't enabled for this dashboard");
    const reasonId = await page.locator(".submit-btn").getAttribute("aria-describedby");
    check(!!reasonId, "(f) the button names its reason via aria-describedby");
    check((await page.locator("#" + reasonId).innerText()).includes("not enabled"), "(f) the reason text is visible");
    check(!errs.length, `(f) no page errors: ${errs.join(" | ")}`);
    await ctx.close();
  }

  // (g) 390px: no horizontal overflow.
  {
    const { page, errs, ctx } = await openAs("gina", "light", 390);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1);
    check(!overflow, "(g) no horizontal scroll at 390px");
    if (shots) await page.screenshot({ path: `${shots}/submit-390.png` });
    check(!errs.length, `(g) no page errors: ${errs.join(" | ")}`);
    await ctx.close();
  }

  // (i)/(j): preview/authoring mode — a plain static file server for
  // web/ (no locked server at all), the same way the plain wasm builder
  // is actually reached. Independent of the `srv` locked server above.
  {
    const staticPort = port + 1;
    const nocache = spawn("python3", ["serve_nocache.py", String(staticPort)], { cwd: path.dirname(path.dirname(new URL(import.meta.url).pathname)) });
    try {
      for (let i = 0; i < 100; i++) {
        try {
          if ((await fetch(`http://127.0.0.1:${staticPort}/`)).ok) break;
        } catch (_) {}
        await sleep(100);
      }
      const previewSql =
        "SELECT q1 ::BUTTONS FROM (VALUES ('A'), ('B'), ('C')) t(q1);\n" +
        "SELECT 'Check my answer' AS quiz_1::SUBMIT, 'q1' AS r::SUBMIT_VARS, 'Saved!' AS d::SUBMIT_DONE;\n";
      const b64 = Buffer.from(previewSql, "utf8").toString("base64");
      const settled = (page) =>
        page.waitForFunction(() => /\d+ panels?/.test(document.getElementById("status")?.textContent || ""), null, { timeout: 30000 });

      // (i) the plain wasm builder: a functional preview submit — the
      // banner shows, clicking inserts locally (bound params; #550 nit)
      // and shows "Submitted at …". Nothing here is the locked server at
      // all (a different origin/port), so this is never reachable from
      // a real network response — a genuine local-only path.
      {
        const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
        const page = await ctx.newPage();
        const errs = [];
        page.on("pageerror", (e) => errs.push(String(e)));
        await page.goto(`http://127.0.0.1:${staticPort}/#sql=${b64}`);
        await settled(page);
        check((await page.locator(".submit-preview-note").count()) === 1, "(i) the preview banner shows on the plain wasm builder");
        await page.locator(".submit-btn").click();
        await page.waitForFunction(
          () => document.querySelector(".submit-status")?.textContent?.includes("Submitted at") || document.querySelector(".submit-error")?.textContent,
          null,
          { timeout: 15000 }
        );
        const status = await page.locator(".submit-status").innerText();
        check(status.includes("Submitted at") && status.includes("Saved!"), `(i) preview submit succeeds locally, got status="${status}" error="${await page.locator(".submit-error").innerText()}"`);
        check(!errs.length, `(i) no page errors: ${errs.join(" | ")}`);
        await ctx.close();
      }

      // (j) review of #550 SF2: the live-authoring backend must never see
      // a viewer_state CREATE/INSERT. No real live-authoring server is
      // even started — route() makes app.js's own startup /query probe
      // succeed, so it sets backend="live" purely client-side; every
      // request this page ever sends to /query is recorded and checked
      // for "viewer_state" at the end (the strongest form of "never
      // sent" — not just "the button looks disabled").
      {
        const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
        const page = await ctx.newPage();
        const errs = [];
        const queryBodies = [];
        page.on("pageerror", (e) => errs.push(String(e)));
        await page.route("**/query", async (route) => {
          queryBodies.push(route.request().postData() || "");
          // One generic row (enough columns for any statement, including
          // the ::SUBMIT one — mkSubmit needs exactly 1 row to render at
          // all, not an empty panel) — the point of this test is never
          // what renders, only that viewer_state SQL never reaches here.
          await route.fulfill({
            status: 200,
            contentType: "application/json",
            body: '[{"c0":"A","c1":"ok","c2":"ok","c3":"ok","c4":"ok"}]',
          });
        });
        await page.goto(`http://127.0.0.1:${staticPort}/#sql=${b64}`);
        await settled(page);
        check((await page.locator(".submit-btn").isDisabled()) === true, '(j) the button is disabled once backend="live" (no real live server needed to prove this: route() alone flips it)');
        const reasonId = await page.locator(".submit-btn").getAttribute("aria-describedby");
        check(!!reasonId && (await page.locator("#" + reasonId).innerText()).includes("live-authoring"), "(j) the reason names live-authoring preview");
        // Clicking (if it somehow weren't disabled) must still never reach
        // /query with viewer_state SQL — belt and braces on top of the
        // disabled check above.
        await page.locator(".submit-btn").click({ force: true }).catch(() => {});
        check(
          !queryBodies.some((b) => /viewer_state/i.test(b)),
          `(j) no /query request ever mentions viewer_state (${queryBodies.length} request(s) sent): ${JSON.stringify(queryBodies)}`
        );
        check(!errs.length, `(j) no page errors: ${errs.join(" | ")}`);
        await ctx.close();
      }
    } finally {
      nocache.kill();
    }
  }

  // (h) axe: no serious/critical violation on the submit panel, light + dark.
  if (axe && existsSync(axe)) {
    for (const theme of ["light", "dark"]) {
      const { page, ctx } = await openAs("hank", theme, 1440, true);
      await page.addScriptTag({ path: axe });
      const r = await page.evaluate(async () => window.axe.run(".submit-panel", { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }));
      for (const v of r.violations) console.log(`axe .submit-panel (${theme}) ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help}`);
      const serious = r.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
      check(!serious.length, `(h) axe (${theme}): no serious/critical violation on .submit-panel`);
      await ctx.close();
    }
  } else {
    console.log("(h) skipped: no --axe axe.min.js given");
  }

  // Screenshots: light/dark, 1440px, ready-to-submit and submitted states.
  if (shots) {
    for (const theme of ["light", "dark"]) {
      const { page: p1, ctx: c1 } = await openAs("shot-ready-" + theme, theme);
      await p1.screenshot({ path: `${shots}/submit-ready-${theme}-1440.png` });
      await p1.locator(".submit-btn").click();
      await p1.waitForFunction(() => document.querySelector(".submit-status")?.textContent?.includes("Submitted at"), null, { timeout: 15000 });
      await p1.screenshot({ path: `${shots}/submit-submitted-${theme}-1440.png` });
      await c1.close();
      const { page: p2, ctx: c2 } = await openAs("shot-err-" + theme, theme);
      await p2.route("**/api/submit", (route) => route.fulfill({ status: 400, body: "'q1' is not one of the options" }));
      await p2.locator(".submit-btn").click();
      await p2.waitForFunction(() => document.querySelector(".submit-error")?.textContent?.includes("not one of the options"), null, { timeout: 15000 });
      await p2.screenshot({ path: `${shots}/submit-error-${theme}-1440.png` });
      await c2.close();
    }
    console.log(`shots written to ${shots}`);
  }
} finally {
  stop(srv);
}
await browser.close();
console.log(`submit-e2e: ${bad} failed`);
if (bad) process.exitCode = 1;
