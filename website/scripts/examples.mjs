// #1022: the builder's examples as the site links them — the same list and ids
// as the builder's sidebar (web/app.js: the inline samples, then FILE_SAMPLES;
// an id is the slug of the -- @title, by app.js's own slugify), read the way
// tests/web/examples_lib_test.mjs reads them.
import { readFileSync } from "fs";
import path from "path";

const web = path.resolve(import.meta.dirname, "..", "..", "web");
const src = readFileSync(path.join(web, "app.js"), "utf8");
const grab = (re, what) => {
  const m = src.match(re);
  if (!m) throw new Error(`${what} not found in web/app.js`);
  return m[0];
};
const lib = new Function(
  [
    grab(/^const SESSIONS = [\s\S]*?^const INLINE_SAMPLES = \[[\s\S]*?^\];/m, "INLINE_SAMPLES"),
    grab(/^const FILE_SAMPLES = \[[\s\S]*?^\];/m, "FILE_SAMPLES"),
    grab(/^function slugify\([\s\S]*?^}/m, "slugify"),
    "return { INLINE_SAMPLES, FILE_SAMPLES, slugify };",
  ].join("\n"),
)();

export const slugify = lib.slugify;
const title = (sql, fallback) => ((/^-- @title (.+)$/m.exec(sql) || [])[1] || fallback).trim();
/** [{ id, title, sql }] in sidebar order. */
export const examples = [
  ...lib.INLINE_SAMPLES.map((sql, i) => [sql, `Example ${i + 1}`]),
  ...lib.FILE_SAMPLES.map((u) => [readFileSync(path.join(web, u), "utf8"), u.replace(/^.*\/|\.sql$/g, "")]),
].map(([sql, fb]) => ({ id: slugify(title(sql, fb)), title: title(sql, fb), sql }));
