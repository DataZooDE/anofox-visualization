// #773 in a real browser: ::CLASS gives a panel the operator stylesheet's
// class as `ax-c-<name>` (a chart, a ::TABLE, a ::PAGED table, a ::MARKDOWN
// box, a KPI; several names; a value from the data). A name outside
// [a-z0-9-] is ignored, and the column is never a table cell. An operator
// rule for the class applies; the app's own classes stay out of reach.
//
//   (web/pkg built as for the other e2e scripts)
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/class-e2e.mjs [--url http://127.0.0.1:8947/]
import { createRequire } from "module";

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

const SQL = `-- @title Class e2e (#773)
SELECT k::XAXIS, v::BARCHART, 'Chart title'::TITLE, 'kpi-hero tall'::CLASS FROM (VALUES ('a', 1), ('b', 2)) t(k, v);
SELECT k ::TABLE, v, 'Table title'::TITLE, 'kpi-hero'::CLASS FROM (VALUES ('a', 1), ('b', 2)) t(k, v);
SELECT k ::PAGED, v, 'Paged title'::TITLE, 'kpi-hero'::CLASS FROM range(30) t(v), (SELECT 'z' AS k);
SELECT 'Some **text**.'::MARKDOWN, 'Md title'::TITLE, CASE WHEN 2 > 1 THEN 'warn' ELSE 'ok' END::CLASS;
SELECT 42::METRIC, 'Answer'::LABEL, 'Bad Name! hidden'::CLASS;
`;

const ready = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );

const want = {
  Chart: ["ax-c-kpi-hero", "ax-c-tall"],
  Table: ["ax-c-kpi-hero"],
  Paged: ["ax-c-kpi-hero"],
  Md: ["ax-c-warn"],
  Kpi: ["ax-c-hidden"],
};

const browser = await chromium.launch();
for (const [w, h] of [[1440, 900], [390, 800]]) {
  const tag = `${w}px`;
  const page = await browser.newPage({ viewport: { width: w, height: h }, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}?mode=view#sql=${b64(SQL)}`);
  await ready(page);
  await page.waitForFunction(() => document.querySelector(".ax-c-kpi-hero td"), null, { timeout: 30000 }).catch(() => {});
  // What an operator's site.css would say.
  await page.addStyleTag({ content: ".ax-c-kpi-hero { --operator-rule: applied; }" });
  const info = await page.evaluate((names) =>
    names.map((name) => {
      // A KPI tile shows no title: it is the one .metric panel.
      const t = [...document.querySelectorAll(".panel-title")].find((x) => x.textContent.startsWith(name + " title"));
      const fig = name === "Kpi" ? document.querySelector(".panel.metric") : t && t.closest(".panel");
      return {
        name,
        found: !!fig,
        classes: fig ? [...fig.classList].filter((c) => c.startsWith("ax-c-")) : [],
        allClasses: fig ? [...fig.classList] : [],
        inCells: fig ? [...fig.querySelectorAll("th, td")].some((c) => /kpi-hero|warn|hidden/.test(c.textContent)) : false,
        inSvg: fig ? [...fig.querySelectorAll("svg text")].some((c) => /kpi-hero|tall/.test(c.textContent)) : false,
        rule: fig ? getComputedStyle(fig).getPropertyValue("--operator-rule").trim() : "",
        visible: fig ? fig.getBoundingClientRect().height > 0 && getComputedStyle(fig).display !== "none" : false,
      };
    }),
    Object.keys(want)
  );
  for (const p of info) {
    const w = want[p.name];
    check(p.found && JSON.stringify(p.classes.sort()) === JSON.stringify([...w].sort()), `${tag} ${p.name}: classes ${JSON.stringify(p.classes)} (want ${JSON.stringify(w)})`);
    check(!p.inCells && !p.inSvg, `${tag} ${p.name}: the ::CLASS value is not a cell or chart text`);
    if (w.includes("ax-c-kpi-hero")) check(p.rule === "applied", `${tag} ${p.name}: the operator rule applies (${p.rule})`);
  }
  const kpi = info.find((p) => p.name === "Kpi");
  check(kpi.visible && !kpi.allClasses.includes("hidden") && !kpi.allClasses.some((c) => /bad|name!/i.test(c)), `${tag} Kpi: no unprefixed or invalid class (${JSON.stringify(kpi.allClasses)})`);
  check(errs.length === 0, `${tag}: no page errors ${errs.join("; ")}`);
  await page.close();
}
await browser.close();
