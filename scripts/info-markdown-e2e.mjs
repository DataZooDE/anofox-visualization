// #759 in a real browser: an ::INFO text keeps its line breaks and renders
// the escaped Markdown subset (a list, **bold**, a table); a long ::INFO
// gets the wide box and scrolls inside it, never past the viewport; a
// ::MARKDOWN panel draws a GFM table, a cell with <script> stays text; the
// popover keeps role=tooltip / aria-describedby and Esc closes it. Light and
// dark, at 1440 and 390 px.
//
//   (web/pkg built as for the other e2e scripts)
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/info-markdown-e2e.mjs [--url http://127.0.0.1:8947/]
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

const DEFS = Array.from({ length: 25 }, (_, k) => `- **Column ${k + 1}**: what column ${k + 1} means, in a sentence long enough to wrap twice.`).join("\n");
const SQL = `-- @title INFO markdown e2e (#759)
SELECT 'Rank' AS rank, 3 AS places ::TABLE, 'Rank: position by places' || chr(10) || 'Places: <b>count</b> of places' ::INFO;
SELECT 1 AS n ::TABLE, '${"Columns:\n" + DEFS}' ::INFO;
SELECT '| Column | What it means |' || chr(10) || '|---|--:|' || chr(10) || '| **Rank** | position |' || chr(10) || '| <script>window.__pwned=1</script> | 2 |' ::MARKDOWN;
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
    const page = await browser.newPage({ viewport: { width: w, height: h }, colorScheme: dark ? "dark" : "light", reducedMotion: "reduce" });
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e)));
    await page.goto(`${base}?mode=view#sql=${b64(SQL)}`);
    await ready(page);
    const btns = page.locator(".dp-info-btn");
    check((await btns.count()) === 2, `${tag}: two ⓘ buttons`);
    // 1. line breaks, escaped HTML
    await btns.nth(0).click();
    const p1 = await page.evaluate(() => {
      const b = document.querySelectorAll(".dp-info-btn")[0];
      const pop = document.getElementById(b.getAttribute("aria-describedby"));
      return { html: pop.innerHTML, role: pop.getAttribute("role"), hidden: pop.hidden, bold: !!pop.querySelector("b") };
    });
    check(p1.role === "tooltip" && !p1.hidden, `${tag}: the popover is a visible tooltip`);
    check(p1.html.includes("<br>") && p1.html.includes("&lt;b&gt;count&lt;/b&gt;") && !p1.bold, `${tag}: a newline is a line break, HTML stays text (${p1.html})`);
    await page.keyboard.press("Escape");
    // 2. a long list: wide, inside the viewport, scrolls inside
    await btns.nth(1).click();
    const p2 = await page.evaluate(() => {
      const b = document.querySelectorAll(".dp-info-btn")[1];
      const pop = document.getElementById(b.getAttribute("aria-describedby"));
      const r = pop.getBoundingClientRect();
      return {
        items: pop.querySelectorAll("li").length,
        strong: pop.querySelectorAll("li strong").length,
        wide: pop.classList.contains("wide"),
        inView: r.left >= 0 && r.right <= innerWidth && r.top >= 0 && r.bottom <= innerHeight,
        scrolls: pop.scrollHeight > pop.clientHeight && getComputedStyle(pop).overflowY === "auto",
        width: r.width,
      };
    });
    check(p2.items === 25 && p2.strong === 25, `${tag}: the glossary is a list with bold names (${p2.items})`);
    check(p2.wide && p2.inView, `${tag}: the long ::INFO is wide and inside the viewport (${JSON.stringify(p2)})`);
    check(p2.scrolls, `${tag}: the long ::INFO scrolls inside its box`);
    await page.keyboard.press("Escape");
    const closed = await page.evaluate(() => [...document.querySelectorAll(".dp-info-pop")].every((p) => p.hidden));
    check(closed, `${tag}: Esc closes it`);
    // 3. the ::MARKDOWN table
    const t = await page.evaluate(() => {
      const tb = document.querySelector(".md-body table.md-table, table.md-table");
      return tb
        ? {
            th: tb.querySelectorAll("th").length,
            rows: tb.querySelectorAll("tbody tr").length,
            right: getComputedStyle(tb.querySelectorAll("th")[1]).textAlign,
            text: tb.textContent,
            scripts: tb.querySelectorAll("script").length,
            pwned: window.__pwned === 1,
          }
        : null;
    });
    check(t && t.th === 2 && t.rows === 2 && t.right === "right", `${tag}: ::MARKDOWN draws a GFM table (${JSON.stringify(t)})`);
    check(t && t.scripts === 0 && !t.pwned && t.text.includes("<script>"), `${tag}: a <script> cell is text`);
    check(errs.length === 0, `${tag}: no page errors ${errs.join("; ")}`);
    await page.close();
  }
}
await browser.close();
