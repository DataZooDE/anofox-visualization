// #520 (OWASP "CSV injection"): a string cell starting with = + - @ a tab or
// a CR must not reach an Excel/LibreOffice user as a live formula. Covers
// every export path in web/app.js: csvOfRows (CSV / .xls-as-HTML-table),
// csvOf's table-DOM fallback, and xlsxOfRows / xlsxOf (the .xlsx writer,
// where string cells are typed and don't need the same treatment — this
// file locks that decision in too, so a future change can't quietly
// reintroduce the risk in one path while "fixing" another).
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const line = (name) => {
  const m = src.match(new RegExp(`^const ${name} = .*;$`, "m"));
  assert.ok(m, `const ${name} not found in app.js`);
  return m[0];
};
const { csvSafe, csvOf, csvOfRows, xlsxOfRows, xlsxOf } = new Function(
  `${["NUM_RE", "FORMULA_LEAD", "csvSafe", "unq"].map(line).join("\n")}\n` +
    `${["csvOfRows", "excelDate", "xlsxOfRows", "crc32", "zipStored", "xlsxCol", "xlsxOf", "csvOf"].map(grab).join("\n")}\n` +
    `return { csvSafe, csvOf, csvOfRows, xlsxOfRows, xlsxOf };`
)();

// csvSafe(): the OWASP trigger set gets a leading quote; a plain (incl.
// negative) number and ordinary text are left alone.
for (const bad of ["=HYPERLINK(\"http://evil\")", "+1+1", "-1+A1", "@SUM(A1)", "\tcmd", "\rcmd"])
  assert.equal(csvSafe(bad), "'" + bad, bad);
for (const ok of ["-1", "3.14", "-2.5e3", "plain text", "", "A-1"]) assert.equal(csvSafe(ok), ok, ok);

// csvOfRows: cols is the header row too (same treatment); rows carry the
// app's "quoted string vs bare number" convention (unq strips the quotes).
const cols = ["id", "note", "qty"];
const rows = [
  { id: "1", note: '"=HYPERLINK(http://evil,click)"', qty: "3" },
  { id: "2", note: '"-1+A1"', qty: "-7" }, // text "-1+A1" vs a real negative number
  { id: "3", note: '"@SUM(A1:A9)"', qty: "0" },
  { id: "4", note: '"\t=evil"', qty: "-1" }, // leading tab, then more formula text
  { id: "5", note: '"-1"', qty: "-1" }, // a string column whose value is just "-1": no quote
];
const csv = csvOfRows(rows, cols);
const lines = csv.split("\n");
assert.equal(lines[0], '"id","note","qty"');
assert.equal(lines[1], '"1","\'=HYPERLINK(http://evil,click)","3"');
assert.equal(lines[2], '"2","\'-1+A1","-7"', "a real negative number (qty) keeps no quote");
assert.equal(lines[3], '"3","\'@SUM(A1:A9)","0"');
assert.equal(lines[4], '"4","\'\t=evil","-1"');
assert.equal(lines[5], '"5","-1","-1"', 'a string column holding plain "-1" is not neutralised');

// csvOf's table-DOM fallback: same rule, straight off `textContent`.
const fakeTable = {
  rows: [
    { cells: [{ textContent: "=cmd|'/c calc'!A1" }, { textContent: "-42" }] },
    { cells: [{ textContent: "ordinary" }, { textContent: "@SUM(1,2)" }] },
  ],
};
const domCsv = csvOf(fakeTable);
assert.equal(
  domCsv,
  '"\'=cmd|\'/c calc\'!A1","-42"\n' + '"ordinary","\'@SUM(1,2)"'
);

// _rows/_cols short-circuit still goes through csvOfRows (and so csvSafe).
const viaRows = csvOf({ _rows: [{ a: '"=evil"' }], _cols: ["a"] });
assert.equal(viaRows, '"a"\n"\'=evil"');

// xlsxOfRows / xlsxOf: a formula-looking string is written as a literal
// inline string, untouched (t="inlineStr" is never re-parsed as a formula —
// see the comment above csvSafe in app.js), while a real negative number
// still becomes a numeric <v> cell.
const wbRows = [{ note: '"=HYPERLINK(http://evil)"', qty: "-5" }];
const wbBytes = xlsxOfRows(wbRows, ["note", "qty"]);
// xlsxOfRows returns zip bytes; pull the worksheet part out (see xlsx_test.mjs
// for the same unzip-by-hand approach, with CRC/central-directory checks).
const findPart = (bytes, name) => {
  const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let p = 0;
  while (dv.getUint32(p, true) === 0x04034b50) {
    const nlen = dv.getUint16(p + 26, true);
    const size = dv.getUint32(p + 18, true);
    const fname = new TextDecoder().decode(bytes.subarray(p + 30, p + 30 + nlen));
    const data = bytes.subarray(p + 30 + nlen, p + 30 + nlen + size);
    if (fname === name) return new TextDecoder().decode(data);
    p += 30 + nlen + size;
  }
  return null;
};
const sheet1 = findPart(wbBytes, "xl/worksheets/sheet1.xml");
assert.match(sheet1, /t="inlineStr"><is><t xml:space="preserve">=HYPERLINK\(http:\/\/evil\)<\/t><\/is>/, "stored verbatim, no leading quote added");
assert.match(sheet1, /<c r="B2" s="\d+"><v>-5<\/v><\/c>/, "a real negative number is still a numeric cell");

console.log("csv_injection_test: ok");
