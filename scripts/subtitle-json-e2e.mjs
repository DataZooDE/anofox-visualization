// #774 in a real browser: ::SUBTITLE is one muted line right under the
// title of a chart, a ::TABLE, a ::PAGED table and a ::MARKDOWN box (never a
// table column, never drawn twice); ::DOWNLOAD_JSON downloads the rows as a
// JSON array. Light and dark, at 1440 and 390 px.
//
//   (web/pkg built as for the other e2e scripts)
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/subtitle-json-e2e.mjs [--url http://127.0.0.1:8947/]
import { createRequire } from "module";
import { readFileSync } from "fs";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const SQL = `-- @title Subtitle e2e (#774)
SELECT k::XAXIS, v::BARCHART, 'Chart title'::TITLE, 'Chart sub <b>x</b>'::SUBTITLE FROM (VALUES ('a', 1), ('b', 2)) t(k, v);
SELECT k ::TABLE, v, 'Table title'::TITLE, 'Table sub'::SUBTITLE FROM (VALUES ('a', 1), ('b', 2)) t(k, v);
SELECT k ::PAGED, v, 'Paged title'::TITLE, 'Paged sub'::SUBTITLE FROM range(30) t(v), (SELECT 'z' AS k);
SELECT 'Some **text**.'::MARKDOWN, 'Md title'::TITLE, 'Md sub'::SUBTITLE;
SELECT k, v ::DOWNLOAD_JSON FROM (VALUES ('a', 1), ('b', 2)) t(k, v);
`;

const ready = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );

const browser = await chromium.launch();
for (const [w, h] of [[1440, 900], [390, 800]]) {
  for (const dark of [false, true]) {
    const tag = `${w}px${dark ? " dark" : ""}`;
    const page = await browser.newPage({ viewport: { width: w, height: h }, colorScheme: dark ? "dark" : "light", reducedMotion: "reduce", acceptDownloads: true });
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e)));
    await page.goto(`${base}?mode=view#sql=${b64(SQL)}`);
    await ready(page);
    await page.waitForFunction(() => [...document.querySelectorAll(".panel-title")].some((t) => t.textContent.includes("Paged title")), null, { timeout: 30000 });
    const info = await page.evaluate(() =>
      ["Chart", "Table", "Paged", "Md"].map((name) => {
        const t = [...document.querySelectorAll(".panel-title")].find((x) => x.textContent.startsWith(name + " title"));
        const fig = t && t.closest(".panel");
        const subs = fig ? [...fig.querySelectorAll(".panel-subtitle")] : [];
        const next = t && (t.nextElementSibling || t.parentElement.nextElementSibling);
        return {
          name,
          found: !!t,
          subs: subs.map((s) => s.textContent),
          rightAfter: !!next && next.classList.contains("panel-subtitle"),
          inTable: fig ? [...fig.querySelectorAll("th, td")].some((c) => /sub/.test(c.textContent)) : false,
          inSvg: fig ? [...fig.querySelectorAll("svg text")].some((c) => /sub/.test(c.textContent)) : false,
          color: subs[0] ? getComputedStyle(subs[0]).color : null,
          titleColor: t ? getComputedStyle(t).color : null,
        };
      })
    );
    for (const p of info) {
      check(p.found && p.subs.length === 1 && p.subs[0] === `${p.name} sub${p.name === "Chart" ? " <b>x</b>" : ""}`, `${tag} ${p.name}: one subtitle (${JSON.stringify(p.subs)})`);
      check(p.rightAfter, `${tag} ${p.name}: right under the title`);
      check(!p.inTable && !p.inSvg, `${tag} ${p.name}: not a table cell, not drawn in the chart`);
      check(p.color && p.color !== p.titleColor, `${tag} ${p.name}: muted (${p.color} vs ${p.titleColor})`);
    }
    const bold = await page.evaluate(() => document.querySelectorAll(".panel-subtitle b").length);
    check(bold === 0, `${tag}: subtitle HTML stays text`);
    // ::DOWNLOAD_JSON
    const btn = page.getByRole("button", { name: /Download JSON/ });
    check((await btn.count()) === 1, `${tag}: a Download JSON button`);
    const [dl] = await Promise.all([page.waitForEvent("download"), btn.click()]);
    const body = JSON.parse(readFileSync(await dl.path(), "utf8"));
    check(dl.suggestedFilename() === "data.json" && Array.isArray(body) && body.length === 2, `${tag}: data.json is the rows (${JSON.stringify(body)})`);
    check(errs.length === 0, `${tag}: no page errors ${errs.join("; ")}`);
    await page.close();
  }
}
await browser.close();
