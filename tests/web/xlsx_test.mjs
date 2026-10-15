// The browser's .xlsx writer (app.js: crc32, zipStored, xlsxOf, xlsxCol,
// excelDate) builds a valid stored zip with the SpreadsheetML parts a
// spreadsheet app needs: typed cells, number formats, formulas, a frozen
// header, a filter and merged cells. No browser, no dependencies.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { crc32, xlsxOf, xlsxCol, excelDate } = new Function(
  `${["crc32", "zipStored", "xlsxCol", "xlsxOf", "excelDate"].map(grab).join("\n")}\nreturn { crc32, xlsxOf, xlsxCol, excelDate };`
)();

// CRC-32 check value, column letters, date serials
assert.equal(crc32(new TextEncoder().encode("123456789")), 0xcbf43926);
assert.deepEqual([0, 25, 26, 51, 701, 702].map(xlsxCol), ["A", "Z", "AA", "AZ", "ZZ", "AAA"]);
assert.equal(excelDate("2026-01-01"), 46023);
assert.equal(excelDate("1900-03-01"), 61);
assert.equal(excelDate("2026-01-01 12:00"), 46023.5);
assert.equal(excelDate("01.02.2026"), null);

const bytes = xlsxOf({
  name: "Stores: by region?",
  cols: [{ width: 12 }, { width: 14 }],
  rows: [
    [{ v: "Money", style: "group" }, null],
    [{ v: "Region", style: "head" }, { v: "Revenue ($M)", style: "head" }],
    ["East <&>", { v: 3100000, fmt: '#,##0.00,,' }],
    ["North", { v: -1210000, fmt: '#,##0.00,,;(#,##0.00,,)' }],
    [{ v: "Total", style: "tot" }, { v: 1890000, f: "SUBTOTAL(109,B3:B4)", fmt: '#,##0.00,,', style: "tot" }],
    [true, { v: 46023, fmt: "dd.mm.yyyy" }],
  ],
  freeze: 2,
  filter: "A2:B4",
  merges: ["A1:B1"],
});

// unzip (stored entries): local headers, names, CRCs, and the central directory
const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
const files = {};
let p = 0;
while (dv.getUint32(p, true) === 0x04034b50) {
  assert.equal(dv.getUint16(p + 8, true), 0, "stored");
  const crc = dv.getUint32(p + 14, true);
  const size = dv.getUint32(p + 18, true);
  const nlen = dv.getUint16(p + 26, true);
  const name = new TextDecoder().decode(bytes.subarray(p + 30, p + 30 + nlen));
  const data = bytes.subarray(p + 30 + nlen, p + 30 + nlen + size);
  assert.equal(crc32(data), crc, `crc of ${name}`);
  files[name] = new TextDecoder().decode(data);
  p += 30 + nlen + size;
}
const end = bytes.length - 22;
assert.equal(dv.getUint32(end, true), 0x06054b50);
assert.equal(dv.getUint16(end + 10, true), Object.keys(files).length);
assert.equal(dv.getUint32(end + 16, true), p, "central directory offset");
assert.deepEqual(Object.keys(files).sort(), [
  "[Content_Types].xml",
  "_rels/.rels",
  "xl/_rels/workbook.xml.rels",
  "xl/styles.xml",
  "xl/workbook.xml",
  "xl/worksheets/sheet1.xml",
]);

const ws = files["xl/worksheets/sheet1.xml"];
assert.match(ws, /<pane ySplit="2" topLeftCell="A3" activePane="bottomLeft" state="frozen"\/>/);
assert.match(ws, /<c r="A3" s="\d+" t="inlineStr"><is><t xml:space="preserve">East &lt;&amp;&gt;<\/t><\/is><\/c>/);
assert.match(ws, /<c r="B3" s="\d+"><v>3100000<\/v><\/c>/);
assert.match(ws, /<c r="B5" s="\d+"><f>SUBTOTAL\(109,B3:B4\)<\/f><v>1890000<\/v><\/c>/);
assert.match(ws, /<c r="A6" s="0" t="b"><v>1<\/v><\/c>/);
assert.match(ws, /<\/sheetData><autoFilter ref="A2:B4"\/><mergeCells count="1"><mergeCell ref="A1:B1"\/><\/mergeCells>/);
const st = files["xl/styles.xml"];
for (const f of ["#,##0.00,,", "#,##0.00,,;(#,##0.00,,)", "dd.mm.yyyy"]) assert.ok(st.includes(`formatCode="${f}"`), f);
assert.match(st, /<top style="double">/);
// the sheet name loses the characters Excel forbids; the filter has its defined name
const wb = files["xl/workbook.xml"];
assert.match(wb, /<sheet name="Stores  by region" sheetId="1" r:id="rId1"\/>/);
assert.match(wb, /<definedName name="_xlnm._FilterDatabase" localSheetId="0" hidden="1">'Stores  by region'!\$A\$2:\$B\$4<\/definedName>/);
console.log("xlsx_test: ok");
