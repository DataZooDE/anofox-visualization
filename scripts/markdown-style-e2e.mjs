// ::MARKDOWN_STYLE (#536) in a real browser: a ::MARKDOWN panel with no
// style at all still gets today's tinted callout box; 'plain' draws no
// card/tint/bar at all (text sits on the page background); 'card' is a
// plain white card; an explicit style wins even over ::CALLOUT (the
// conflict ::CALLOUT 'warning'::CALLOUT keeps its icon only when the style
// stays 'callout'); the dashboard's own `-- @markdown_style` default is
// overridden by a panel's own ::MARKDOWN_STYLE; none of this overflows the
// panel width, in light/dark, at 1440 and 390 px.
//
//   cargo build --target wasm32-unknown-unknown --profile wasm --no-default-features --features wasm
//   wasm-bindgen --target web --out-dir web/pkg <the .wasm above>; wasm-opt -O -o web/pkg/anofox_visualization_bg.wasm web/pkg/anofox_visualization_bg.wasm
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/markdown-style-e2e.mjs \
//       [--url http://127.0.0.1:8947/] [--shots <dir>]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const shots = arg("--shots", "");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const LONG = "Monitoring answers three questions, chunk by chunk: is the input the same shape as training, does the model still predict well, and has the world it predicts moved on?";
const SQL = `-- @title Markdown style e2e (#536)
SELECT '${LONG}'::MARKDOWN;
SELECT '${LONG}'::MARKDOWN, 'plain'::MARKDOWN_STYLE;
SELECT '${LONG}'::MARKDOWN, 'card'::MARKDOWN_STYLE;
SELECT 'Margins of the last two weeks are provisional — late returns still come in.'::MARKDOWN, 'warning'::CALLOUT;
SELECT 'An explicit style wins even over ::CALLOUT.'::MARKDOWN, 'plain'::MARKDOWN_STYLE, 'warning'::CALLOUT;
`;
// The dashboard-wide default, overridden by the 2nd panel's own ::CALLOUT
// (which still draws 'callout' — not the header's 'plain').
const DEFAULT_SQL = `-- @markdown_style plain
SELECT 'No panel-level style: the dashboard default applies.'::MARKDOWN;
SELECT 'A panel-level ::CALLOUT still draws the callout box.'::MARKDOWN, 'note'::CALLOUT;
`;

const ready = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );

const browser = await chromium.launch();

async function open(sql, viewport) {
  const page = await browser.newPage({ viewport, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}?mode=view#sql=${b64(sql)}`);
  await ready(page);
  return { page, errs };
}

const panelInfo = (page) =>
  page.evaluate(() => {
    const panels = [...document.querySelectorAll(".dash .panel")];
    return panels.map((fig) => {
      const body = fig.querySelector(".md-body");
      const cs = getComputedStyle(fig);
      return {
        classes: [...fig.classList].sort().join(" "),
        icon: fig.dataset.icon || null,
        role: fig.getAttribute("role"),
        bg: cs.backgroundColor,
        borderColor: cs.borderColor,
        boxShadow: cs.boxShadow,
        // the text must wrap inside the panel, never spill past it
        overflowsX: body ? body.scrollWidth > body.clientWidth + 1 : false,
      };
    });
  });

// (a) 1440 px, light: all five panels resolve the chrome the lint/SVG/CLI
// code paths do (src/roles.rs resolve_markdown_style).
{
  const { page, errs } = await open(SQL, { width: 1440, height: 1000 });
  check(!errs.length, "(a) no page errors: " + errs.join(" | "));
  const info = await panelInfo(page);
  check(info.length === 5, `(a) 5 panels rendered (${info.length})`);
  const [def, plain, card, callout, override] = info;

  check(def.classes === "md-box panel", `(a1) no style/no ::CALLOUT: today's look, 'md-box' (${def.classes})`);
  check(!def.icon, `(a1) the default 'note' tint carries no icon attribute (${def.icon})`);
  check(!def.overflowsX, "(a1) default callout text wraps inside the box");

  check(plain.classes === "md-plain panel", `(a2) 'plain'::MARKDOWN_STYLE (${plain.classes})`);
  check(plain.boxShadow === "none", `(a2) 'plain' has no shadow (${plain.boxShadow})`);
  check(!plain.overflowsX, "(a2) plain text wraps inside the box");

  check(card.classes === "panel", `(a3) 'card'::MARKDOWN_STYLE: the bare card look, no md-box/md-plain (${card.classes})`);
  check(!card.overflowsX, "(a3) card text wraps inside the box");

  check(callout.classes === "dp-callout dp-callout-warning md-box panel", `(a4) 'warning'::CALLOUT alone: the tinted box + icon class (${callout.classes})`);
  check(callout.icon === "⚠", `(a4) warning icon (${callout.icon})`);
  check(callout.role === "note", `(a4) role=note, never an assertive alert (${callout.role})`);

  check(override.classes === "md-plain panel", `(a5) an explicit 'plain'::MARKDOWN_STYLE wins over ::CALLOUT — no md-box/dp-callout (${override.classes})`);
  check(!override.icon, `(a5) …and so no icon either (${override.icon})`);

  if (shots) await page.screenshot({ path: `${shots}/markdown-style-1440-light.png`, fullPage: true });

  // (b) the dashboard's own dark toggle — the classes must not change with
  // the theme, only the CSS tokens behind them. The toggle rewrites the
  // page's single #ax-theme <style> block (applyTheme(), web/app.js) during
  // the async re-run the click kicks off — body.dark alone flips instantly,
  // but the tokens (and so a panel's computed background) only repaint once
  // that finishes, so wait for documentElement.dataset.axVariant to actually
  // read "dark" before comparing anything token-derived (#536 CI failure:
  // reading right after the click, before the rewrite landed, saw the same
  // light rgba twice).
  const calloutBgToken = () => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--ax-callout-bg").trim());
  const lightToken = await calloutBgToken();
  await page.click("#dark");
  await page.waitForFunction(() => document.documentElement.dataset.axVariant === "dark", null, { timeout: 15000 });
  const darkToken = await calloutBgToken();
  check(darkToken !== "" && darkToken !== lightToken, `(b) dark mode repaints the --ax-callout-bg token (${lightToken} -> ${darkToken})`);
  const infoDark = await panelInfo(page);
  check(
    infoDark.map((p) => p.classes).join("|") === info.map((p) => p.classes).join("|"),
    "(b) dark mode keeps the same chrome classes as light"
  );
  check(infoDark[0].bg !== info[0].bg, `(b) a panel's own computed background follows the token too (${infoDark[0].bg} vs ${info[0].bg})`);
  if (shots) await page.screenshot({ path: `${shots}/markdown-style-1440-dark.png`, fullPage: true });
  await page.close();
}

// (c) 390 px: no horizontal overflow of the page itself, still wraps.
{
  const { page, errs } = await open(SQL, { width: 390, height: 1400 });
  check(!errs.length, "(c) no page errors at 390px: " + errs.join(" | "));
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1);
  check(!overflow, "(c) no horizontal overflow at 390px");
  const info = await panelInfo(page);
  check(
    !info.some((p) => p.overflowsX),
    `(c) no panel's text overflows its own box at 390px (${JSON.stringify(info.map((p) => p.overflowsX))})`
  );
  if (shots) await page.screenshot({ path: `${shots}/markdown-style-390-light.png`, fullPage: true });
  await page.close();
}

// (d) `-- @markdown_style plain` (dashboard default), overridden by a
// panel's own ::CALLOUT.
{
  const { page, errs } = await open(DEFAULT_SQL, { width: 1280, height: 700 });
  check(!errs.length, "(d) no page errors: " + errs.join(" | "));
  const info = await panelInfo(page);
  check(info.length === 2, `(d) 2 panels rendered (${info.length})`);
  check(info[0].classes === "md-plain panel", `(d1) the dashboard's own -- @markdown_style plain applies with no panel override (${info[0].classes})`);
  check(info[1].classes === "dp-callout dp-callout-note md-box panel", `(d2) a panel's own ::CALLOUT still draws 'callout', over the 'plain' default (${info[1].classes})`);
  if (shots) await page.screenshot({ path: `${shots}/markdown-style-default-header.png`, fullPage: true });
  await page.close();
}

// (e) ::MARKDOWN_TONE (#659): a toned card gets md-tone + md-tone-<tone>, a
// small label (data-tone-label, drawn by ::before) and a thin accent bar; a tone implies 'card' over the 'callout' header default; 'neutral'
// and a tone beside 'plain' draw no accent; dark mode repaints the bar.
const TONE_SQL = `-- @markdown_style callout
-- @title Markdown tones e2e (#659)
SELECT 'Why we watch the inputs, not only the error.'::MARKDOWN, 'info'::MARKDOWN_TONE;
SELECT 'A worked example: 120 orders, 9 late.'::MARKDOWN, 'example: Beispiel'::MARKDOWN_TONE;
SELECT 'Compare like with like: same weekday, same season.'::MARKDOWN, 'tip'::MARKDOWN_TONE;
SELECT 'What it cannot tell you: why the input moved.'::MARKDOWN, 'caution'::MARKDOWN_TONE;
SELECT '$$D = \\max_x |F_1(x) - F_2(x)|$$'::MARKDOWN, 'details:'::MARKDOWN_TONE;
SELECT 'Plain explanation.'::MARKDOWN, 'neutral'::MARKDOWN_TONE;
SELECT 'Dropped beside plain.'::MARKDOWN, 'plain'::MARKDOWN_STYLE, 'tip'::MARKDOWN_TONE;
`;
{
  const { page, errs } = await open(TONE_SQL, { width: 1440, height: 1000 });
  check(!errs.length, "(e) no page errors: " + errs.join(" | "));
  const tones = () =>
    page.evaluate(() =>
      [...document.querySelectorAll(".dash .panel")].map((fig) => {
        const before = getComputedStyle(fig, "::before");
        return {
          classes: [...fig.classList].sort().join(" "),
          label: fig.dataset.toneLabel || null,
          labelShown: before.content,
          labelColor: before.color,
          bar: getComputedStyle(fig).backgroundImage,
          bg: getComputedStyle(fig).backgroundColor,
        };
      })
    );
  const t = await tones();
  check(t.length === 7, `(e) 7 panels rendered (${t.length})`);
  const want = ["info", "example", "tip", "caution", "details"];
  want.forEach((tone, i) => {
    check(t[i].classes === `md-tone md-tone-${tone} panel`, `(e${i + 1}) '${tone}': md-tone card, not the 'callout' default (${t[i].classes})`);
    check(/linear-gradient/.test(t[i].bar), `(e${i + 1}) '${tone}': an accent bar (${t[i].bar})`);
  });
  check(t[0].label === "Info" && t[0].labelShown === '"Info"', `(e1) default label shown (${t[0].label} / ${t[0].labelShown})`);
  check(t[1].label === "Beispiel", `(e2) 'example: Beispiel' sets the label (${t[1].label})`);
  check(t[4].label === null && t[4].labelShown === "none", `(e5) 'details:' has no ::before label — it folds, see (f) (${t[4].label} / ${t[4].labelShown})`);
  check(t[2].labelColor !== t[3].labelColor, `(e) tip and caution labels differ in colour (${t[2].labelColor} vs ${t[3].labelColor})`);
  check(t[0].bg === t[5].bg, `(e) no tint: a toned card has the plain card background (${t[0].bg} vs ${t[5].bg})`);
  check(t[5].classes === "panel", `(e6) 'neutral': a bare card (${t[5].classes})`);
  check(t[6].classes === "md-plain panel", `(e7) a tone beside 'plain' is dropped (${t[6].classes})`);
  if (shots) await page.screenshot({ path: `${shots}/markdown-tone-1440-light.png`, fullPage: true });
  await page.click("#dark");
  await page.waitForFunction(() => document.documentElement.dataset.axVariant === "dark", null, { timeout: 15000 });
  const d = await tones();
  check(d[1].bar !== t[1].bar, `(e) dark mode repaints the example bar (${t[1].bar} -> ${d[1].bar})`);
  if (shots) await page.screenshot({ path: `${shots}/markdown-tone-1440-dark.png`, fullPage: true });
  await page.close();
}

// (f) 'details' folds (#691): a <details> whose <summary> is the label
// (the tone's own 'Details' when empty), closed at first, opened by a click
// or by Enter on the focused summary; printing opens every folded box and
// folds them back afterwards; 390 px: no overflow.
const FOLD_SQL = `-- @title Markdown details e2e (#691)
SELECT '$$\\mathrm{MASE} = \\frac{\\mathrm{MAE}}{\\mathrm{MAE}_{naive}}$$ — the scaled error.'::MARKDOWN, 'details: How MASE is computed'::MARKDOWN_TONE;
SELECT 'Second folded box.'::MARKDOWN, 'details'::MARKDOWN_TONE;
SELECT 'Not folded.'::MARKDOWN, 'tip'::MARKDOWN_TONE;
`;
for (const width of [1440, 390]) {
  const { page, errs } = await open(FOLD_SQL, { width, height: 900 });
  check(!errs.length, `(f) ${width}px: no page errors: ` + errs.join(" | "));
  const st = () =>
    page.evaluate(() =>
      [...document.querySelectorAll(".dash .panel")].map((fig) => {
        const d = fig.querySelector(":scope > details.md-details");
        const body = fig.querySelector(".md-body");
        return {
          folded: !!d,
          open: d ? d.open : null,
          summary: d?.querySelector(":scope > summary")?.textContent ?? null,
          bodyVisible: !!body && body.checkVisibility(),
          labelAttr: fig.dataset.toneLabel || null,
          overflow: fig.scrollWidth > fig.clientWidth + 1,
        };
      })
    );
  let f = await st();
  check(f.length === 3, `(f) ${width}px: 3 panels (${f.length})`);
  check(f[0].folded && f[0].open === false && !f[0].bodyVisible, `(f1) ${width}px: folded and closed, body hidden (${JSON.stringify(f[0])})`);
  check(f[0].summary === "How MASE is computed" && f[0].labelAttr === null, `(f1) the label is the summary, not a ::before label (${f[0].summary} / ${f[0].labelAttr})`);
  check(f[1].summary === "Details", `(f2) 'details' alone: summary 'Details' (${f[1].summary})`);
  check(!f[2].folded && f[2].bodyVisible && f[2].labelAttr === "Tip", `(f3) other tones don't fold (${JSON.stringify(f[2])})`);
  check(f.every((x) => !x.overflow), `(f) ${width}px: no panel overflows`);
  await page.locator(".dash .panel").nth(0).locator("summary").click();
  f = await st();
  check(f[0].open && f[0].bodyVisible, `(f1) ${width}px: a click opens it`);
  await page.locator(".dash .panel").nth(1).locator("summary").focus();
  await page.keyboard.press("Enter");
  f = await st();
  check(f[1].open, `(f2) ${width}px: Enter on the focused summary opens it`);
  if (shots) await page.screenshot({ path: `${shots}/markdown-details-${width}-open.png`, fullPage: true });
  // fold the 2nd again, then print: both open while printing, the 2nd folds back
  await page.keyboard.press("Enter");
  await page.evaluate(() => dispatchEvent(new Event("beforeprint")));
  f = await st();
  check(f[0].open && f[1].open, `(f) ${width}px: printing opens every folded box`);
  await page.evaluate(() => dispatchEvent(new Event("afterprint")));
  f = await st();
  check(f[0].open && f[1].open === false, `(f) ${width}px: after printing, the box the reader had closed folds back`);
  await page.close();
}

// (g) 'caption'::MARKDOWN_STYLE (#692): the text right after a chart is
// that chart card's footer (no box of its own), with its {{values}}; two
// captions stack; a caption with no chart before it is plain text; a chart
// on a hidden tab (rendered lazily) gets its caption when shown.
const CAP_SQL = `-- @title Captions e2e (#692)
SELECT 'Overview'::TAB;
SELECT 'Lead-in text.'::MARKDOWN, 'caption'::MARKDOWN_STYLE;
SELECT d::XAXIS, v::LINECHART, 'Orders per day'::TITLE FROM (SELECT range AS d, (range * 7) % 11 AS v FROM range(10));
SELECT 'Source: ERP; values below **{{t}}** are good.'::MARKDOWN, 'caption'::MARKDOWN_STYLE, 1 AS t;
SELECT 'MASE = $\\frac{a}{b}$.'::MARKDOWN, 'caption'::MARKDOWN_STYLE;
SELECT 'A separate box.'::MARKDOWN;
SELECT 'Detail'::TAB;
SELECT d::XAXIS, v::BARCHART, 'Later chart'::TITLE FROM (SELECT range AS d, (range * 3) % 7 AS v FROM range(6));
SELECT 'Caption of a lazily drawn chart.'::MARKDOWN, 'caption'::MARKDOWN_STYLE;
`;
for (const width of [1440, 390]) {
  const { page, errs } = await open(CAP_SQL, { width, height: 900 });
  check(!errs.length, `(g) ${width}px: no page errors: ` + errs.join(" | "));
  const st = () =>
    page.evaluate(() =>
      [...document.querySelectorAll(".tabpane:not([hidden]) .panel")].filter((p) => p.checkVisibility()).map((p) => ({
        chart: !!p.querySelector(".panel-svg svg"),
        caps: [...p.querySelectorAll(":scope > .dp-caption")].map((c) => c.textContent.trim()),
        math: !!p.querySelector(".dp-caption .katex"),
        rules: [...p.querySelectorAll(":scope > .dp-caption")].map((c) => getComputedStyle(c).borderTopWidth),
        cls: [...p.classList].sort().join(" "),
        overflow: p.scrollWidth > p.clientWidth + 1,
      }))
    );
  let f = await st();
  check(f.length === 3, `(g) ${width}px: 3 panels — the captions have no box of their own (${f.length})`);
  check(f[0].cls === "md-plain panel" && !f[0].chart, `(g1) no chart before: plain text (${f[0].cls})`);
  check(f[1].chart && f[1].caps.length === 2 && f[1].caps[0] === "Source: ERP; values below 1 are good.", `(g2) the chart's footer, {{values}} filled (${JSON.stringify(f[1].caps)})`);
  if (!f[1].math) {
    await page.waitForSelector(".dp-caption .katex", { timeout: 15000 }).catch(() => {});
    f = await st();
  }
  check(f[1].math, "(g2) a formula in a caption is typeset");
  check(f[1].rules.join() === "1px,0px", `(g2) one rule above the captions, not one each (${f[1].rules})`);
  check(!f[2].chart && f[2].caps.length === 0, "(g3) the next ::MARKDOWN is its own box");
  check(f.every((x) => !x.overflow), `(g) ${width}px: no panel overflows`);
  if (shots) await page.screenshot({ path: `${shots}/markdown-caption-${width}.png`, fullPage: true });
  await page.click('.tabbar > .tab-btn:text-is("Detail")');
  await page.waitForFunction(() => [...document.querySelectorAll(".tabpane:not([hidden]) .panel .dp-caption")].length === 1, null, { timeout: 30000 }).catch(() => {});
  f = await st();
  check(f.length === 1 && f[0].chart && f[0].caps[0] === "Caption of a lazily drawn chart.", `(g4) ${width}px: a lazily drawn chart gets its caption (${JSON.stringify(f)})`);
  await page.close();
}

await browser.close();
console.log(`markdown-style-e2e: ${process.exitCode ? "failed" : "0 failed"}`);
