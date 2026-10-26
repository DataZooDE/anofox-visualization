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
  // the theme, only the CSS tokens behind them.
  await page.click("#dark");
  const infoDark = await panelInfo(page);
  check(
    infoDark.map((p) => p.classes).join("|") === info.map((p) => p.classes).join("|"),
    "(b) dark mode keeps the same chrome classes as light"
  );
  check(infoDark[0].bg !== info[0].bg, `(b) dark mode does repaint the tokens (${infoDark[0].bg} vs ${info[0].bg})`);
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

await browser.close();
console.log(`markdown-style-e2e: ${process.exitCode ? "failed" : "0 failed"}`);
