// ::IMAGE / ::IMAGE_DARK / markdown `![alt](src)` images (#514) in a real
// browser: a data: SVG panel with alt text and a caption, the dark variant
// switch (the dashboard's own toggle, not the OS setting), an inline
// markdown image, a rejected src drawing the alt-text placeholder with no
// network request, ::HEIGHT keeping the box fixed and contained, no
// horizontal overflow at 390 px, and the ⤓ HTML export still showing the
// image fully offline.
//
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/image-e2e.mjs \
//       [--url http://127.0.0.1:8947/] [--shots <dir>] [--axe axe.min.js]
import { createRequire } from "module";
import { readFileSync } from "fs";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const shots = arg("--shots", "");
const axePath = arg("--axe", "");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

// A 4x2 PNG (base64 of src/image.rs test fixture's bytes, by hand) and a
// small, self-contained legend SVG with a viewBox (no width/height — the
// browser's default replaced-element size kicks in, exercised deliberately).
const SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 220 60"><circle cx="20" cy="20" r="8" fill="#3aa876"/><circle cx="20" cy="40" r="8" fill="#e8a33d"/></svg>';
const SVG_DATA = "data:image/svg+xml;base64," + Buffer.from(SVG, "utf8").toString("base64");
const SVG_DARK = "data:image/svg+xml;base64," + Buffer.from(SVG.replace("3aa876", "6ad9a6"), "utf8").toString("base64");

const SQL = `SELECT '${SVG_DATA}'::IMAGE, '${SVG_DARK}'::IMAGE_DARK,
       'Status legend: green ok, amber delayed'::ALT, 'Legend'::TITLE, 'How to read the chart'::LABEL;
SELECT 'Below: ![a status legend](${SVG_DATA})'::MARKDOWN;
SELECT 'javascript:alert(1)'::IMAGE, 'rejected source'::ALT, 'Bad source'::TITLE;
SELECT 200::HEIGHT;
SELECT '${SVG_DATA}'::IMAGE, 'A fixed-height image'::ALT, 'Fixed height'::TITLE;
`;

const ready = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );

const browser = await chromium.launch();

async function open(viewport) {
  const page = await browser.newPage({ viewport, reducedMotion: "reduce", acceptDownloads: true });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  const requests = [];
  page.on("request", (r) => requests.push(r.url()));
  await page.goto(`${base}#sql=${b64(SQL)}`);
  await ready(page);
  return { page, errs, requests };
}

// (a) a data: SVG ::IMAGE: naturalWidth > 0, alt set, caption visible.
{
  const { page, errs, requests } = await open({ width: 1280, height: 900 });
  check(!errs.length, "no page errors: " + errs.join(" | "));
  const info = await page.evaluate(() => {
    const fig = document.querySelector(".dp-image");
    const img = fig?.querySelector("img.dp-img:not(.dp-img-dark)");
    const cap = fig?.querySelector(".dp-image-caption");
    return { naturalWidth: img?.naturalWidth || 0, alt: img?.alt, caption: cap?.textContent };
  });
  check(info.naturalWidth > 0, `(a) image has a natural width (${info.naturalWidth})`);
  check(info.alt === "Status legend: green ok, amber delayed", `(a) alt text set (${info.alt})`);
  check(info.caption === "How to read the chart", `(a) caption visible (${info.caption})`);
  if (shots) await page.screenshot({ path: `${shots}/image-light.png` });

  // (b) the dashboard's own dark toggle (#dark), not the OS setting, swaps
  // which <img> shows (CSS, not a reload).
  await page.click("#dark");
  await page.waitForTimeout(150);
  const vis = await page.evaluate(() => {
    const fig = document.querySelector(".dp-img-row.has-dark");
    const light = fig?.querySelector("img.dp-img:not(.dp-img-dark)");
    const dark = fig?.querySelector("img.dp-img-dark");
    const shown = (el) => !!el && el.getClientRects().length > 0 && getComputedStyle(el).display !== "none";
    return { light: shown(light), dark: shown(dark) };
  });
  check(vis.dark && !vis.light, `(b) dark toggle shows ::IMAGE_DARK, hides ::IMAGE (${JSON.stringify(vis)})`);
  if (shots) await page.screenshot({ path: `${shots}/image-dark.png` });

  // (c) markdown image visible inline.
  const mdImg = await page.evaluate(() => {
    const img = document.querySelector(".md-img");
    return img ? { visible: img.getClientRects().length > 0, alt: img.alt } : null;
  });
  check(!!mdImg && mdImg.visible, "(c) markdown image visible inline");
  check(mdImg?.alt === "a status legend", `(c) markdown image alt text (${mdImg?.alt})`);

  // (d) a bad src shows the alt-text placeholder and fires no request for it.
  const placeholder = await page.evaluate(() =>
    [...document.querySelectorAll(".dp-img-missing")].map((s) => s.textContent)
  );
  check(placeholder.includes("rejected source"), `(d) rejected src shows the alt text (${placeholder})`);
  const leaked = requests.some((u) => u.includes("alert(1)") || u.startsWith("javascript:"));
  check(!leaked, "(d) no network request for the rejected src");

  // (e) ::HEIGHT 200 keeps the box fixed, the image contained (not stretched
  // past its box, not left with empty space beyond the box).
  const fixed = await page.evaluate(() => {
    const fig = [...document.querySelectorAll(".dp-image")].find((f) => f.textContent.includes("Fixed height"));
    if (!fig) return null;
    const r = fig.getBoundingClientRect();
    const img = fig.querySelector("img.dp-img");
    const ir = img.getBoundingClientRect();
    return { figH: r.height, imgBottom: ir.bottom, figBottom: r.bottom, objectFit: getComputedStyle(img).objectFit };
  });
  check(!!fixed && fixed.objectFit === "contain", `(e) ::HEIGHT box uses object-fit: contain (${fixed?.objectFit})`);
  check(!!fixed && fixed.imgBottom <= fixed.figBottom + 1, "(e) image stays inside its fixed box");

  await page.close();
}

// (f) 390 px: no horizontal scroll, the image fits the width.
{
  const { page, errs } = await open({ width: 390, height: 844 });
  check(!errs.length, "(f) no page errors at 390px");
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1);
  check(!overflow, "(f) no horizontal overflow at 390px");
  if (shots) await page.screenshot({ path: `${shots}/image-390.png`, fullPage: true });
  await page.close();
}

// (g) axe: no image-alt violation (optional — only with --axe).
if (axePath) {
  const { page } = await open({ width: 1280, height: 900 });
  await page.addScriptTag({ path: axePath });
  const violations = await page.evaluate(async () => {
    const r = await window.axe.run(document, { runOnly: ["wcag2a", "wcag2aa"] });
    return r.violations.map((v) => v.id);
  });
  check(!violations.includes("image-alt"), `(g) axe: no image-alt violation (${violations.join(",")})`);
  await page.close();
} else {
  console.log("skip (g) axe image-alt check — no --axe given");
}

// (h) the ⤓ HTML export still shows the image fully offline.
{
  const { page } = await open({ width: 1280, height: 900 });
  const [dl] = await Promise.all([page.waitForEvent("download"), page.click("#dlhtml")]);
  const html = readFileSync(await dl.path(), "utf8");
  await page.close();

  const off = await browser.newContext({ offline: true, viewport: { width: 1280, height: 900 } });
  const op = await off.newPage();
  const exportErrs = [];
  op.on("pageerror", (e) => exportErrs.push(String(e)));
  await op.route("**/*", (r) => (r.request().url().startsWith("http") ? (r.abort(), undefined) : r.continue()));
  await op.setContent(html, { waitUntil: "load" });
  await op.waitForTimeout(300);
  const shown = await op.evaluate(() => {
    const img = document.querySelector("img.dp-img:not(.dp-img-dark)");
    return img ? { visible: img.getClientRects().length > 0, src: img.getAttribute("src")?.slice(0, 10) } : null;
  });
  check(!!shown && shown.visible, "(h) exported HTML shows the image offline");
  check(!!shown && shown.src === "data:image", `(h) the export's image src is inlined as data: (${shown?.src})`);
  check(!exportErrs.length, "(h) no errors in the offline export: " + exportErrs.join(" | "));
  await off.close();
}

await browser.close();
if (process.exitCode) {
  console.error("\nimage-e2e: FAILED");
} else {
  console.log("\nimage-e2e: all checks passed");
}
