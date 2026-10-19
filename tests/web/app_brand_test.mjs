// Node test for the builder's app bar per installation (web/app.js): the
// layer merge (defaults → app-config.json → the serving process) and the
// logo / link rules. The server side is src/host/app_brand.rs.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const defaults = src.match(/^const APP_BRAND_DEFAULT = .*;$/m);
assert.ok(defaults, "APP_BRAND_DEFAULT not found");
const warnings = [];
const console = { warn: (m) => warnings.push(m) };
const { appBrandMerge, appImageOk, appLinkOk, APP_BRAND_DEFAULT } = new Function(
  "console",
  `${defaults[0]}\n${grab("appImageOk")}\n${grab("appLinkOk")}\n${grab("appBrandMerge")}\n` +
    `return { appBrandMerge, appImageOk, appLinkOk, APP_BRAND_DEFAULT };`
)(console);

// defaults: the product name and the plain tagline
assert.deepEqual(appBrandMerge(), {
  name: "anofox-visualization",
  tagline: "Dashboards from SQL",
  logo: "",
  logo_dark: "",
  link: "",
  favicon: "",
  name_visible: true,
});
assert.deepEqual(appBrandMerge(null, undefined, [], "x", 3), APP_BRAND_DEFAULT);

// later layers win, key by key
const file = { name: "Acme Insights", tagline: "Numbers for the shop floor", logo: "brand/logo.svg" };
const server = { tagline: "", link: "https://intranet.acme.example/" };
assert.deepEqual(appBrandMerge(file, server), {
  name: "Acme Insights",
  tagline: "", // empty hides it
  logo: "brand/logo.svg",
  logo_dark: "",
  link: "https://intranet.acme.example/",
  favicon: "",
  name_visible: true,
});
// an empty name keeps the previous one; an empty logo clears it
assert.equal(appBrandMerge(file, { name: "  " }).name, "Acme Insights");
assert.equal(appBrandMerge(file, { logo: "" }).logo, "");
// non-strings and unknown keys are ignored (e.g. an arbitrary "credit" key)
const m = appBrandMerge({ name: 42, credit: "Someone else", footer: "x", tagline: null });
assert.deepEqual(m, APP_BRAND_DEFAULT);
// text: control characters dropped, whitespace collapsed, length capped
assert.equal(appBrandMerge({ name: "Acme\n\tBI" }).name, "Acme BI");
assert.equal(appBrandMerge({ name: "x".repeat(200) }).name.length, 80);
assert.equal(appBrandMerge({ tagline: "y".repeat(400) }).tagline.length, 160);
// markup stays text (the bar uses textContent; nothing is parsed)
assert.equal(appBrandMerge({ name: "<b>Acme</b>" }).name, "<b>Acme</b>");

// logo / favicon: images only
for (const ok of [
  "https://cdn.acme.example/logo.png",
  "http://intranet/logo.svg",
  "logo.svg",
  "brand/logo.webp",
  "data:image/png;base64,iVBORw0KGgo=",
  "data:image/svg+xml;base64,PHN2Zz4=",
  "data:image/svg+xml,%3Csvg%3E",
  "data:image/jpeg;base64,/9j/",
  "data:image/gif;base64,R0lG",
  "data:image/webp;base64,UklG",
])
  assert.ok(appImageOk(ok), ok);
for (const bad of [
  "",
  "javascript:alert(1)",
  "JaVaScRiPt:alert(1)",
  "data:text/html;base64,PGgxPg==",
  "data:image/svg+xml", // no payload separator
  "data:image/x-icon;base64,AAAB",
  "vbscript:x",
  "//evil.example/logo.png",
  'logo.png" onerror="alert(1)',
  "logo(1).png",
  "a b.png",
  "https://x/\u0000.png",
  "data:image/png;base64," + "A".repeat(700000),
])
  assert.ok(!appImageOk(bad), bad);
warnings.length = 0;
assert.equal(appBrandMerge({ logo: "javascript:alert(1)" }).logo, "");
assert.equal(appBrandMerge(file, { logo: "data:text/html,<script>" }).logo, "brand/logo.svg");
assert.equal(appBrandMerge({ favicon: "data:image/png;base64,AA==" }).favicon, "data:image/png;base64,AA==");
assert.equal(appBrandMerge({ favicon: "file:///etc/passwd" }).favicon, "");
assert.equal(warnings.length, 3, warnings.join("\n"));

// logo_dark (#198): the dark-theme variant, the logo's image rules, only with a logo
{
  const w0 = warnings.length;
  assert.equal(appBrandMerge(file, { logo_dark: "brand/logo-dark.svg" }).logo_dark, "brand/logo-dark.svg");
  assert.equal(appBrandMerge(file, { logo_dark: "javascript:alert(1)" }).logo_dark, "");
  assert.equal(appBrandMerge(file, { logo_dark: 'x.png" onerror="alert(1)' }).logo_dark, "");
  assert.equal(appBrandMerge({ logo_dark: "brand/logo-dark.svg" }).logo_dark, ""); // no logo
  assert.equal(appBrandMerge(file, { logo_dark: "d.svg" }, { logo: "" }).logo_dark, ""); // logo cleared later
  assert.equal(appBrandMerge(file, { logo_dark: "d.svg" }, { logo_dark: "" }).logo_dark, "");
  assert.equal(warnings.length - w0, 4, warnings.slice(w0).join("\n"));
  warnings.length = w0;
  const ap = grab("appBrandApply");
  assert.ok(/app-logo-/.test(ap) && /cfg\.logo_dark/.test(ap), "a dark-theme logo next to the light one");
  const page = readFileSync(new URL("../../web/index.html", import.meta.url), "utf8");
  assert.ok(page.includes("body.dark header h1 .app-logo-light{display:none}") || page.includes("body.dark header h1 .app-logo-light"), "the light logo hides in dark mode");
}

// link: absolute http(s) only
assert.ok(appLinkOk("https://acme.example/"));
assert.ok(appLinkOk("http://intranet:8080/home"));
for (const bad of ["javascript:alert(1)", "mailto:a@b.example", "/home", "https://", "https:///x", "https://a b", "ftp://x"])
  assert.ok(!appLinkOk(bad), bad);
assert.equal(appBrandMerge({ link: "javascript:alert(1)" }).link, "");

// name_visible: false (a wordmark) only with a logo — the bar is never empty
assert.equal(appBrandMerge(file, { name_visible: false }).name_visible, false);
assert.equal(appBrandMerge({ name_visible: false }).name_visible, true);
assert.equal(appBrandMerge(file, { name_visible: false }, { logo: "" }).name_visible, true);
assert.equal(appBrandMerge(file, { name_visible: "false" }).name_visible, true); // booleans only
assert.equal(warnings.length, 6, warnings.join("\n"));

// the bar renders text with textContent only and draws the logo as an <img>
const apply = grab("appBrandApply");
assert.ok(!/innerHTML|insertAdjacentHTML|outerHTML/.test(apply), "appBrandApply must not parse markup");
assert.ok(/img\.alt = cfg\.name_visible \? "" : cfg\.name/.test(apply), "a wordmark logo carries the name as alt");
assert.ok(/createElement\("img"\)/.test(apply) && !/createElementNS|<svg/.test(apply), "the logo is an <img>");
// the sidebar company line is not driven by the app-brand settings
const html = readFileSync(new URL("../../web/index.html", import.meta.url), "utf8");
assert.ok(!html.includes("© DataZoo GmbH"), "no company line in the sidebar footer");
assert.ok(!/app(Brand|-brand)[\s\S]{0,40}side-foot/.test(apply), "footer untouched");

// the assistant's CSP allows the app bar's images: logo, logo_dark (#269), favicon
{
  const brandImgOrigins = new Function(`${grab("brandImgOrigins")}\nreturn brandImgOrigins;`)();
  assert.deepEqual(
    brandImgOrigins({ logo: "https://a.example/l.svg", logo_dark: "https://b.example/d.svg", favicon: "data:image/png;base64,x" }),
    ["https://a.example/l.svg", "https://b.example/d.svg"],
  );
  assert.deepEqual(brandImgOrigins({ logo_dark: "javascript:alert(1)" }), []);
}

process.stdout.write("app_brand_test: ok\n");
