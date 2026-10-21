// #244 M1-3: web/assist-ui.js's pure functions (no DOM at import time — this
// test imports it directly under Node) and static greps over web/app.js,
// web/assist-ui.js and web/index.html proving the security requirements
// (§2 of plan-244) that can be checked without a browser: R1-R7 (config,
// model-text-as-text, the key never leaking, Remember, the apply gate,
// loadSdk timing, the CSP-before-everything order), R8/R9/R11/R12 (the
// preview never reaching the page's own DuckDB, no re-import of
// DuckDB-Wasm, no bypass hooks on `window`). The DOM/e2e half of the
// panel — the actual browser behaviour — is scripts/assist-e2e.mjs.
import { existsSync, readFileSync } from "fs";
import path from "path";
import assert from "assert/strict";
import * as U from "../../web/assist-ui.js";
import { assistConfig } from "../../web/assist.js";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const read = (p) => readFileSync(path.join(root, p), "utf8");
const appJs = read("web/app.js");
const uiJs = read("web/assist-ui.js");
const indexHtml = read("web/index.html");

// ---- units ----

// modelParagraphs: hostile text passes through unchanged (R2 is textBlock's
// job, not modelParagraphs' — this only splits/caps).
{
  const hostile = "<img src=x onerror=alert(1)>\n\n![a](https://e/x.png)\n\n[c](javascript:alert(1))";
  const parts = U.modelParagraphs(hostile);
  assert.deepEqual(parts, ["<img src=x onerror=alert(1)>", "![a](https://e/x.png)", "[c](javascript:alert(1))"]);
  assert.deepEqual(U.modelParagraphs("a\r\n\r\nb"), ["a", "b"], "CRLF -> LF before splitting");
  assert.deepEqual(U.modelParagraphs("  \n\n  "), [], "blank paragraphs dropped");
  const many = Array.from({ length: 50 }, (_, i) => "p" + i).join("\n\n");
  assert.equal(U.modelParagraphs(many).length, 40, "capped at 40 paragraphs");
  assert.equal(U.modelParagraphs("x".repeat(5000))[0].length, 4000, "capped at 4000 chars");
}

// textBlock: a fake doc that records createElement names and throws on
// innerHTML/src/href — only "p" with textContent must ever be touched.
{
  const created = [];
  const sinkNames = ["innerHTML", "outerHTML", "src", "href"];
  const fakeEl = () => {
    const txt = { value: "" };
    const kids = [];
    const node = {
      tagName: "P",
      appendChild(c) {
        kids.push(c);
        return c;
      },
      get textContent() {
        return txt.value;
      },
      set textContent(v) {
        txt.value = v;
      },
    };
    for (const s of sinkNames) {
      Object.defineProperty(node, s, {
        get() {
          throw new Error(`read of .${s}`);
        },
        set() {
          throw new Error(`write of .${s}`);
        },
      });
    }
    return node;
  };
  const fakeDoc = {
    createElement(name) {
      created.push(name);
      return fakeEl();
    },
  };
  const root = fakeEl();
  const appended = [];
  root.appendChild = (c) => appended.push(c);
  U.textBlock(fakeDoc, root, "<b>bold</b>\n\n![x](http://e/x.png)");
  assert.deepEqual(created, ["p", "p"]);
  assert.equal(appended.length, 2);
  assert.equal(appended[0].textContent, "<b>bold</b>");
  assert.equal(appended[1].textContent, "![x](http://e/x.png)");
}

// keyLooksValid
{
  assert.equal(U.keyLooksValid("sk-ant-api03-" + "a".repeat(40)), true);
  assert.equal(U.keyLooksValid(""), false);
  assert.equal(U.keyLooksValid("sk-ant-"), false);
  assert.equal(U.keyLooksValid("sk-ant-x y" + "a".repeat(20)), false);
  assert.equal(U.keyLooksValid("sk-proj-" + "a".repeat(30)), false);
  assert.equal(U.keyLooksValid("sk-ant-aa\n" + "a".repeat(25)), false, "a newline inside breaks the charset match");
  assert.equal(U.keyLooksValid("  sk-ant-" + "a".repeat(25) + "  "), true, "surrounding whitespace is trimmed");
}

// statementSpans: every span slices back to its own piece, including
// comments and an E'' string.
{
  const sql = "SELECT 1; -- a comment with ; inside\nSELECT E'it''s; here' AS x; SELECT 2;";
  const spans = U.statementSpans(sql);
  const pieces = spans.map(([a, b]) => sql.slice(a, b));
  assert.equal(pieces.length, 3);
  for (const p of pieces) assert.ok(sql.includes(p));
  assert.ok(pieces[1].includes("E'it''s; here'"));
}

// lineDiff: identity, insertion, deletion; length/order invariants.
{
  assert.deepEqual(U.lineDiff(["a", "b"], ["a", "b"]).map((x) => x.op), [" ", " "]);
  const ins = U.lineDiff(["a", "b"], ["a", "c", "b"]);
  assert.deepEqual(ins.map((x) => x.op + x.text), [" a", "+c", " b"]);
  const del = U.lineDiff(["a", "c", "b"], ["a", "b"]);
  assert.deepEqual(del.map((x) => x.op + x.text), [" a", "-c", " b"]);
  // every line of both inputs appears exactly once, in a stable relative order
  for (const [a, b] of [
    [["x", "y", "z"], ["x", "z"]],
    [[], ["only"]],
    [["only"], []],
  ]) {
    const d = U.lineDiff(a, b);
    assert.equal(d.filter((x) => x.op !== "+").length, a.length);
    assert.equal(d.filter((x) => x.op !== "-").length, b.length);
  }
}

// diffScripts / scopeTables / classifyStmt / roleCategories need the real
// wasm `plan` export (web/pkg): a missing pkg is a hard failure (never
// "skip" — the CI rule).
const wasmPkg = path.join(root, "web/pkg");
if (!existsSync(path.join(wasmPkg, "anofox_visualization_bg.wasm"))) {
  console.error("assist_ui_test: FAILED — needs web/pkg (wasm-pack build)");
  process.exit(1);
}
const wasm = await import(path.join(wasmPkg, "anofox_visualization.js"));
wasm.initSync({ module: readFileSync(path.join(wasmPkg, "anofox_visualization_bg.wasm")) });
const roleCats = U.roleCategories(wasm.roles_json());
const CLEAN = read("scripts/e2e-fixtures/assist/proposal-clean.sql");

{
  const d1 = U.diffScripts("", CLEAN, { plan: wasm.plan, roleCats, panels: [] });
  assert.match(d1.summary, /panels? added/);
  assert.match(d1.summary, /1 filter added/);

  const CHANGED = CLEAN.replace("::BARCHART", "::TABLE");
  const d2 = U.diffScripts(CLEAN, CHANGED, { plan: wasm.plan, roleCats, panels: [] });
  assert.match(d2.summary, /panel \d+ changed/);

  const money = "SELECT ROUND(SUM(units*price),0) ::MONEY, 'Revenue' ::LABEL\nFROM sales WHERE region = getvariable('region');\n";
  assert.ok(CLEAN.includes(money), "the fixture's KPI statement is exactly this text (sanity)");
  const REMOVED = CLEAN.replace(money, "");
  const d3 = U.diffScripts(CLEAN, REMOVED, { plan: wasm.plan, roleCats, panels: [] });
  assert.match(d3.summary, /panel(?:\(s\))? removed|1 panel removed/);

  const setupA = "CREATE TEMP TABLE t AS SELECT 1 AS x;\nSELECT x ::METRIC FROM t;";
  const setupB = "CREATE TEMP TABLE t AS SELECT 2 AS x;\nSELECT x ::METRIC FROM t;";
  const d4 = U.diffScripts(setupA, setupB, { plan: wasm.plan, roleCats, panels: [] });
  assert.match(d4.summary, /data setup changed/);

  const d5 = U.diffScripts(CLEAN, CHANGED, { plan: wasm.plan, roleCats, panels: [{ stmt: 7, title: "x", why: "because it helps" }] });
  const chg = d5.groups.find((g) => g.op === "chg" && g.n === 7);
  assert.equal(chg?.why, "because it helps", "why attaches to its statement number");

  assert.equal(U.diffScripts(CLEAN, CLEAN, { plan: wasm.plan, roleCats, panels: [] }).summary, "No changes.");
}

{
  const tables = Array.from({ length: 20 }, (_, i) => ({ name: `t${i + 1}`, fq: `t${i + 1}`, rows: 1 }));
  // No name of `tables` appears literally in this SQL except inside a
  // comment ("t2") — so scopeTables falls back to "all" (none named), and
  // the comment never counts as "t2 is in scope".
  const { inScope, rest } = U.scopeTables(tables, "SELECT * FROM nothing_named_here -- t2 (a comment: does not count)");
  assert.equal(inScope.length, 15);
  assert.equal(rest.length, 5);

  const named = U.scopeTables(tables, "SELECT * FROM t3");
  assert.deepEqual(named.inScope.map((t) => t.name), ["t3"], "a name in a comment does not count; a name in real code does, and nothing else is pulled in");

  const withExcluded = U.scopeTables([...tables, { name: "tex", fq: "tex", rows: 0, excluded: "too large" }], "");
  assert.ok(!withExcluded.inScope.concat(withExcluded.rest).some((t) => t.name === "tex"), "excluded tables are dropped");
}

// applyModel: the 4 combinations.
{
  assert.equal(U.applyModel({}).needsDialog, false);
  assert.deepEqual(U.applyModel({}).buttons.map((b) => b.value), ["cancel", "run"]);
  assert.equal(U.applyModel({ readers: ["x"] }).needsDialog, true);
  assert.equal(U.applyModel({ stale: true }).needsDialog, true);
  const withRefused = U.applyModel({ refused: [{ sql: "x", reason: "y" }] });
  assert.equal(withRefused.needsDialog, true);
  assert.deepEqual(withRefused.buttons.map((b) => b.value), ["cancel", "norun", "run"]);
  assert.ok(withRefused.buttons.find((b) => b.value === "run").danger);
}

// makeHistory: push/last/canUndo/pop, cap 20.
{
  const h = U.makeHistory();
  assert.equal(h.last(), null);
  assert.equal(h.canUndo("x"), false);
  for (let i = 0; i < 25; i++) h.push({ before: "b" + i, after: "a" + i, title: "t" + i });
  assert.equal(h.last().after, "a24");
  assert.equal(h.canUndo("a24"), true);
  assert.equal(h.canUndo("a23"), false);
  let count = 0;
  while (h.pop()) count++;
  assert.equal(count, 20, "capped at 20 entries");
}

// statusText for every phase.
{
  assert.equal(U.statusText(null), "");
  assert.equal(U.statusText({ phase: "thinking" }), "Thinking…");
  assert.equal(U.statusText({ phase: "tool", tool: "list_tables" }), "Listing tables…");
  assert.equal(U.statusText({ phase: "tool", tool: "profile_table" }), "Profiling a table…");
  assert.equal(U.statusText({ phase: "tool", tool: "run_sql" }), "Running a query…");
  assert.equal(U.statusText({ phase: "tool", tool: "check_dashboard" }), "Checking the dashboard…");
  assert.equal(U.statusText({ phase: "checking", round: 2, maxRounds: 3 }), "Checking panels · round 2 of 3");
  assert.equal(U.statusText({ phase: "engine", fq: "a.b.c" }), "Copying a.b.c for the assistant…");
  assert.equal(U.statusText({ phase: "profile", name: "sales", i: 1, n: 3 }), "Profiling sales (1 of 3)…");
  assert.equal(U.statusText({ phase: "writing", lines: 5 }), "Writing the dashboard… 5 lines");
}

// isEngineReset
{
  assert.equal(U.isEngineReset("assistant engine changed state; rebuilding (#204)"), true);
  assert.equal(U.isEngineReset('Permission Error: Cannot access directory "x"'), true);
  assert.equal(U.isEngineReset("not run: reads https://x"), false);
}

console.log("assist_ui_test: units ok");

// ---- static greps (comments stripped first) ----

// Strips // and /* */ comments, tracking string/template literal state
// (so a "//" or a quote inside a string is never mistaken for the start of
// a comment or a new string) AND regex literals (so an apostrophe or quote
// *inside* a bare /regex/ — app.js has plenty, e.g. contractions in a
// comment are one thing, but a regex like /\bdon't\b/ would otherwise
// desync the whole scan: everything up to the next matching quote,
// anywhere later in the file, would wrongly be treated as "inside a
// string"). A `/` starts a regex only where one is syntactically possible
// (not right after an identifier/number/`)`/`]`, which is a division).
function stripComments(src) {
  let out = "";
  let i = 0;
  const n = src.length;
  let lastSig = "";
  const regexAllowed = () => !lastSig || !(/[A-Za-z0-9_$)\]]/.test(lastSig));
  while (i < n) {
    const ch = src[i];
    const two = src.slice(i, i + 2);
    if (ch === "'" || ch === '"' || ch === "`") {
      const quote = ch;
      let j = i + 1;
      while (j < n) {
        if (src[j] === "\\") {
          j += 2;
          continue;
        }
        if (src[j] === quote) {
          j++;
          break;
        }
        j++;
      }
      out += src.slice(i, j);
      i = j;
      lastSig = quote;
      continue;
    }
    if (two === "//") {
      let j = src.indexOf("\n", i);
      if (j < 0) j = n;
      out += " ";
      i = j;
      continue;
    }
    if (two === "/*") {
      const j = src.indexOf("*/", i + 2);
      out += " ";
      i = j < 0 ? n : j + 2;
      continue;
    }
    if (ch === "/" && regexAllowed()) {
      let j = i + 1,
        inClass = false,
        bad = false;
      while (j < n) {
        if (src[j] === "\\") {
          j += 2;
          continue;
        }
        if (src[j] === "\n") {
          bad = true;
          break;
        }
        if (src[j] === "[") inClass = true;
        else if (src[j] === "]") inClass = false;
        else if (src[j] === "/" && !inClass) {
          j++;
          break;
        }
        j++;
      }
      if (!bad) {
        while (j < n && /[a-zA-Z]/.test(src[j])) j++;
        out += src.slice(i, j);
        i = j;
        lastSig = "/";
        continue;
      }
      // not actually a regex (an unterminated line) — fall through as a
      // plain division character.
    }
    out += ch;
    if (!/\s/.test(ch)) lastSig = ch;
    i++;
  }
  return out;
}
const appClean = stripComments(appJs);
const uiClean = stripComments(uiJs);

const grabFn = (src, sig) => {
  const m = src.match(new RegExp(`^(?:async )?function ${sig}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${sig} not found`);
  return m[0];
};

// G1: assistConfig( matches exactly once; its first argument is appAssistBlock.
{
  const matches = [...appClean.matchAll(/assistConfig\(/g)];
  assert.equal(matches.length, 1, `assistConfig( occurs ${matches.length} times, expected 1`);
  assert.match(appClean, /assistConfig\(appAssistBlock,/);
}
// G2: appAssistBlock = occurs only inside appBrandLoad().
{
  const body = grabFn(appJs, "appBrandLoad");
  const bodyClean = stripComments(body);
  const assigns = [...appClean.matchAll(/\bappAssistBlock\s*=/g)];
  // one inside appBrandLoad, one in the module-level `let … appAssistBlock = null …` declaration.
  const inBody = [...bodyClean.matchAll(/\bappAssistBlock\s*=/g)].length;
  assert.equal(inBody, 1, "appAssistBlock is assigned exactly once inside appBrandLoad");
  assert.equal(assigns.length, 2, `appAssistBlock = occurs ${assigns.length} times total, expected 2 (the declaration + appBrandLoad)`);
  assert.match(appClean, /let assistCfg[\s\S]{0,80}appAssistBlock\s*=\s*null/, "the other occurrence is the module-level declaration (= null)");
}
// G3: no URL-param reading of "assistant", and assistConfig never gets params/location.
{
  assert.doesNotMatch(appClean, /searchParams\.get\("assistant/);
  assert.doesNotMatch(appClean, /params\.get\("assistant/);
  assert.doesNotMatch(appClean, /assistConfig\([^)]*\b(params|location)\b/);
}
// G4: app-config.json is fetched via import.meta.url.
assert.match(appClean, /fetch\(new URL\("app-config\.json",\s*import\.meta\.url\)/);

// G5: no HTML sink in assist-ui.js; .href = occurs exactly once, assigned CONSOLE_KEYS_URL.
{
  for (const sink of ["innerHTML", "outerHTML", "insertAdjacentHTML", "document.write", "renderMarkdown", "srcdoc", "eval(", "new Function", ".src =", 'setAttribute("src"']) {
    assert.doesNotMatch(uiClean, new RegExp(sink.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")), `assist-ui.js must not contain ${sink}`);
  }
  const hrefAssigns = [...uiClean.matchAll(/\.href\s*=/g)];
  assert.equal(hrefAssigns.length, 1, `.href = occurs ${hrefAssigns.length} times, expected 1`);
  assert.match(uiClean, /\.href\s*=\s*CONSOLE_KEYS_URL/);
}
// G6: no console.* in assist-ui.js; no raw .error storage; no localStorage/sessionStorage.
{
  assert.doesNotMatch(uiClean, /\bconsole\.(log|warn|error|info|debug|trace|assert|table)\b/);
  assert.doesNotMatch(uiClean, /\.error\s*=(?!=)/);
  assert.doesNotMatch(uiClean, /\br\.error\b/);
  assert.doesNotMatch(uiClean, /\bresult\.error\b/);
  assert.doesNotMatch(uiClean, /\blocalStorage\b/);
  assert.doesNotMatch(uiClean, /\bsessionStorage\b/);
}
// G7: KEY_SLOT only in the import and the #256 boot block; share/export/save
// never mention keyStore/KEY_SLOT/assist.
{
  const keySlotLines = appClean.split("\n").filter((l) => l.includes("KEY_SLOT"));
  assert.ok(keySlotLines.length <= 2, `KEY_SLOT appears on ${keySlotLines.length} lines, expected at most 2 (import + boot's hasKey check)`);
  for (const fn of ["shareLink", "downloadHtml", "exportAll", "saveDash"]) {
    const body = grabFn(appJs, fn);
    const bc = stripComments(body);
    assert.doesNotMatch(bc, /keyStore|KEY_SLOT|\bassist/i, `${fn} must never mention keyStore/KEY_SLOT/assist`);
  }
}
// G8/G9: conn./db. direct use outside boot() — boot() itself is exempt.
{
  const bootBody = grabFn(appClean, "boot");
  const withoutBoot = appClean.replace(bootBody, " ");
  assert.doesNotMatch(withoutBoot, /\bconn\.(query|send|prepare|close)\b/, "conn.* outside boot()");
  assert.doesNotMatch(withoutBoot, /\bdb\.(registerFile\w*|copyFileToBuffer|dropFile|globFiles|connect)\(/, "db.* outside boot()/assistDeps");
  assert.equal([...appClean.matchAll(/conn = await db\.connect\(\)/g)].length, 1, "conn = await db.connect() occurs exactly once (boot)");
  assert.equal([...appClean.matchAll(/scratchRunner\(db\b/g)].length, 0, "scratchRunner(db, …) must always be scratchRunner(pageDb(), …)");
}
// G10: runSqlInner dispatches on its own captured preview token (not the
// live, mutable `preview` — sec-review-272-rereview H-1R: a run queued
// while a preview was active could still be in flight once Discard/"Apply,
// don't run" had already nulled the global, and the live read let its
// later statements fall through to pageConn()) before prepareSql(;
// guardPage(pageAdapter( exists.
{
  const body = grabFn(appJs, "runSqlInner");
  const bc = stripComments(body);
  const tokenIdx = bc.indexOf("const token = pv !== undefined ? pv : runPreviewToken;");
  const dispatchIdx = bc.indexOf("if (token) return token.run(sql);");
  const prepareIdx = bc.indexOf("prepareSql(");
  assert.ok(
    tokenIdx >= 0 && dispatchIdx >= 0 && prepareIdx >= 0 && tokenIdx < dispatchIdx && dispatchIdx < prepareIdx,
    "the pv-or-runPreviewToken dispatch comes before prepareSql("
  );
  // The dispatch must never read the live, mutable global directly, and a
  // caller's own explicitly-passed `pv` (sec-review-272-rereview H-1R2: a
  // lazy/prefetch job bound to the run that created it) must win outright
  // over the global, which a lazy job's query may run long after.
  assert.doesNotMatch(bc, /if \(preview\) return preview\.run\(sql\);/, "runSqlInner must not dispatch on the live `preview` (H-1R)");
  assert.doesNotMatch(bc, /if \(runPreviewToken\) return runPreviewToken\.run\(sql\);/, "runSqlInner must not dispatch on the bare global alone (H-1R2)");
  assert.match(appClean, /guardPage\(pageAdapter\(/);
}
// G10c (sec-review-272-rereview final check, H-1R2): every lazy/prefetch
// job is tied to the preview (or page, if null) the run that created it
// captured — never to the live, mutable global — both for whether it
// runs at all (lazyRunJob/lazyPump) and for where its own queries go
// (panelSql/runSetup/runSql take `pv` explicitly).
{
  const lazyRunJobBody = stripComments(grabFn(appJs, "lazyRunJob"));
  assert.match(lazyRunJobBody, /L\.pv !== preview/, "lazyRunJob must skip a job whose captured preview no longer matches the live one");
  const lazyPumpBody = stripComments(grabFn(appJs, "lazyPump"));
  assert.match(lazyPumpBody, /L\.pv !== preview/, "lazyPump's own loop must break once its L.pv no longer matches the live preview");
  assert.match(stripComments(grabFn(appJs, "panelSql")), /async function panelSql\(sql, at, pv\)/, "panelSql must take pv explicitly");
  assert.match(stripComments(grabFn(appJs, "runSetup")), /async function runSetup\(sql, pv\)/, "runSetup must take pv explicitly");
  assert.match(appClean, /const lazy = dpLazyTabs \? \{ gen: dpLazy\.gen, pv, fresh,/, "the lazy object must carry its own run's pv");
  // run()'s finally also bumps dpLazy.gen whenever it drops a stale token
  // (belt-and-suspenders alongside the L.pv checks above).
  const runFinally = stripComments(grabFn(appJs, "run"));
  assert.match(runFinally, /if \(runPreviewToken && runPreviewToken !== preview\) \{\s*runPreviewToken = null;\s*dpLazy\.gen\+\+;\s*\}/, "run()'s finally must bump dpLazy.gen when it drops a stale runPreviewToken");
}
// G10b (sec-review-272-rereview H-1R): runPreviewToken is captured once,
// synchronously, in run() — not re-read live inside runOnce() after an
// await — and threaded into runOnce(fresh, pv), which assigns it before
// its own first await.
{
  const runBody = stripComments(grabFn(appJs, "run"));
  assert.match(runBody, /const pv = preview;/, "run() must capture `pv` synchronously, before anything async");
  assert.match(runBody, /runOnce\(fresh, pv\)/, "run() must thread its captured `pv` into runOnce()");
  const runOnceBody = stripComments(grabFn(appJs, "runOnce"));
  const tokenIdx = runOnceBody.indexOf("runPreviewToken = pv;");
  const firstAwaitIdx = runOnceBody.indexOf("await ");
  assert.ok(tokenIdx >= 0 && firstAwaitIdx >= 0 && tokenIdx < firstAwaitIdx, "runOnce() must set runPreviewToken before its first await");
}
// G11: assist-ui.js has no literal `import(` and never mentions duckdb.
{
  assert.doesNotMatch(uiClean, /\bimport\(/, "assist-ui.js must contain no dynamic import() — the SDK loads through assist.js's loadSdk");
  assert.doesNotMatch(uiClean, /duckdb/i);
}
// G12: no baseURL/requireLocked/__testing/lock:/prepare: in a wasmTools( call
// (there is none in assist-ui.js or app.js); no window.assistDeps/__assist;
// import("./assist-ui.js") occurs exactly once in app.js, inside openAssistant.
{
  for (const bad of ["baseURL", "requireLocked", "__testing", "lock:", "prepare:"]) {
    assert.doesNotMatch(uiClean, new RegExp(`wasmTools\\([^)]*${bad}`, "s"));
    assert.doesNotMatch(appClean, new RegExp(`wasmTools\\([^)]*${bad}`, "s"));
  }
  assert.doesNotMatch(appClean, /window\.assistDeps/);
  assert.doesNotMatch(appClean, /window\.__assist\b/);
  const uiImports = [...appClean.matchAll(/import\("\.\/assist-ui\.js"\)/g)];
  assert.equal(uiImports.length, 1, `import("./assist-ui.js") occurs ${uiImports.length} times, expected 1`);
  const openAssistantBody = grabFn(appJs, "openAssistant");
  assert.match(stripComments(openAssistantBody), /import\("\.\/assist-ui\.js"\)/);
}
// G13: #assist-btn and .ed-tabs have `hidden`; #assist-pane is not inside
// .editor; no ancestor (.ed-col/.workspace/.main/.shell) has backdrop-filter/filter.
{
  assert.match(indexHtml, /<button id="assist-btn"[^>]*\bhidden\b/);
  assert.match(indexHtml, /<div class="ed-tabs"[^>]*\bhidden\b/);
  const editorOpen = indexHtml.indexOf('<div class="editor"');
  const editorClose = indexHtml.indexOf("</div>", indexHtml.indexOf('<section id="assist-pane"'));
  const paneIdx = indexHtml.indexOf('<section id="assist-pane"');
  assert.ok(paneIdx > 0, "#assist-pane exists");
  // #assist-pane must come after .editor's own closing tag that precedes it
  // (a cheap but sufficient check: the nearest preceding "</div>\n        <section id=\"assist-pane\"" pairing in the markup we wrote).
  assert.match(indexHtml, /<\/div>\s*<section id="assist-pane"/);
  for (const sel of [/\.ed-col\{[^}]*\}/, /\.workspace\{[^}]*\}/, /^\.main\{[^}]*\}/m, /\.shell\{[^}]*\}/]) {
    const m = indexHtml.match(sel);
    if (m) assert.doesNotMatch(m[0], /backdrop-filter|(?<!-)\bfilter\s*:/);
  }
}
// G14: app-config.example.json parses; its assistant block defaults off;
// enabling it (host not shared) turns remember off by default.
{
  const cfgText = read("web/app-config.example.json");
  const cfg = JSON.parse(cfgText);
  assert.ok(cfg.assistant, "app-config.example.json has an assistant block");
  assert.equal(cfg.assistant.enabled, false);
  const enabled = assistConfig({ ...cfg.assistant, enabled: true }, { host: "x.example" });
  assert.equal(enabled.enabled, true);
  assert.equal(enabled.remember, false);
}

// G15 (Info-11, sec-review-272-rereview seventh pass, #336): the proposal's
// own `heldTexts` entry — the one `host.hold.arm()` adds — is pinned, so
// the ~50-entry eviction below can never drop it, and an explicit
// confirmation of that exact text clears the pin again.
{
  assert.match(appClean, /function heldTextsAdd\(sql, pin\)/, "heldTextsAdd takes a pin flag");
  assert.match(appClean, /if \(pin\) \{\s*pinnedHeldText = sql;/, "pin sets pinnedHeldText");
  assert.match(appClean, /if \(oldest === pinnedHeldText\) continue;/, "eviction must skip the pinned entry");
  assert.match(appClean, /hold: \{ arm: \(sql\) => heldTextsAdd\(sql, true\) \}/, "host.hold.arm() pins the proposal's own entry");
  const confirmBody = stripComments(grabFn(appJs, "confirmHeldIfMatch"));
  assert.match(confirmBody, /pinnedHeldText === v\) pinnedHeldText = null/, "a confirmed match releases the pin");
}

// G18 (M-4, sec-review-343, Medium): the hold must be statement-aware, not
// just text-aware — a native redo (or any other path) that reassembles
// only a FRAGMENT of a held proposal is never a member of `heldTexts`
// (a whole-text set), so a separate, uncapped `heldStatements` set (every
// normalised statement of a proposal actually armed) must also re-arm the
// hold whenever the editor contains ANY one of them. Also fixes Info-12
// (only the newest proposal's entry survived `heldTexts`' LRU cap): this
// set has no cap, so an earlier, still-unconfirmed proposal's statements
// are never evicted just because a later one was also armed.
{
  assert.match(appClean, /let heldStatements = new Set\(\);/, "a statement-level hold set exists");
  assert.match(appClean, /function normStmt\(sql\)/, "a comment-stripping, whitespace-collapsing normaliser exists");
  // normStmt must keep string/dollar-quoted literal CONTENT verbatim — a
  // sqlCode()-style blank-the-literals normaliser would make two
  // DIFFERENT statements that only share structure compare equal, a false
  // re-arm risk the other direction (every literal is a hazard, not a
  // convenience): a dedicated `normStmt` function must exist, never reuse
  // of `sqlCode(` for this purpose.
  const normStmtBody = stripComments(grabFn(appJs, "normStmt"));
  assert.doesNotMatch(normStmtBody, /sqlCode\(/, "normStmt must not blank string literals (sqlCode does; the hold needs exact literal content)");
  // Info-13 (sec-review-343 second pass): every character outside a
  // literal/quoted identifier/dollar-quote is upper-cased.
  assert.match(normStmtBody, /out \+= c\.toUpperCase\(\);/, "normStmt upper-cases non-literal text (Info-13)");
  assert.match(appClean, /function normStatementsOf\(sql\)/);
  assert.match(appClean, /function editorHeldByStatement\(sql\)/);
  // M-5 (sec-review-343 second pass, Medium): a statement's own
  // line-suffix tails must be held too, not just the whole statement — a
  // model-crafted multi-line statement (`EXPLAIN\nATTACH …;`, one
  // harmless statement) can have Chromium's redo fragment (same
  // drop-the-first-line shape as M-4) land on just its tail
  // (`ATTACH …;`), a DIFFERENT, harmful statement never otherwise
  // recorded. `heldEntriesOf` must split on newlines and normalise every
  // suffix, and both the pinned arm and an explicit confirmation must go
  // through it (not the whole-statement-only `normStatementsOf`).
  assert.match(appClean, /function heldEntriesOf\(sql\)/, "a tail-expanding entry builder exists");
  const heldEntriesBody = stripComments(grabFn(appJs, "heldEntriesOf"));
  assert.match(heldEntriesBody, /lines\.slice\(i\)\.join\("\\n"\)/, "heldEntriesOf holds every line-suffix tail of a statement");
  const heldTextsAddBody = stripComments(grabFn(appJs, "heldTextsAdd"));
  assert.match(heldTextsAddBody, /for \(const s of heldEntriesOf\(sql\)\) heldStatements\.add\(s\)/, "a pinned (fresh) arm records every statement AND tail");
  const confirmBody = stripComments(grabFn(appJs, "confirmHeldIfMatch"));
  assert.match(confirmBody, /for \(const s of heldEntriesOf\(v\)\) heldStatements\.delete\(s\)/, "an explicit run releases every statement AND tail it ran");
  // Both re-arm sites (the #sql input handler, run()'s belt-and-braces
  // check) must OR the statement-level check onto the whole-text one.
  assert.match(appClean, /heldTexts\.has\(v\) \|\| editorHeldByStatement\(v\)/, "the #sql input handler's re-arm check");
  assert.match(appClean, /heldTexts\.has\(\$\("sql"\)\.value\) \|\| editorHeldByStatement\(\$\("sql"\)\.value\)/, "run()'s belt-and-braces re-arm check");
  assert.match(appClean, /heldStatements\.clear\(\);/, "loadDash releases the statement-level hold too");
}

// L-8 (sec-review-343): `heldBasisSql` must not capture an EARLIER,
// still-unconfirmed proposal's text as the engine's trusted basis — only
// set it for a FRESH hold when the editor isn't already showing a held
// proposal (by either the whole-text or the statement-level check).
{
  const body = stripComments(grabFn(appJs, "previewExit"));
  assert.match(
    body,
    /if \(!applyHold && !heldTexts\.has\(v\) && !editorHeldByStatement\(v\)\) heldBasisSql = v;/,
    "heldBasisSql is captured only when the editor isn't already held"
  );
}

// G16 (Info-9, sec-review-272-rereview seventh pass, #336): the dark-toggle
// MutationObserver that redraws Explore's own charts must skip the redraw
// outright while a preview is active, and route whatever it does run as
// page work (`pv: null`) rather than falling through to the live,
// possibly-preview `runPreviewToken`.
{
  const mo = appClean.match(/new MutationObserver\(\(\) => \{[\s\S]*?\}\)\.observe\(document\.body/);
  assert.ok(mo, "the dark-toggle chart-redraw MutationObserver not found");
  assert.match(mo[0], /if \(preview\) return;/, "Info-9: skip the redraw while a preview is active");
  assert.match(mo[0], /xpDrawChart\(c\.holder, c\.sql, null\)/, "Info-9: route the redraw as page work (pv: null)");
}

// G17 (Info-10, sec-review-272-rereview fifth pass, #336 follow-up):
// assistDeps()'s extensions/files/prepareFiles must read `assistEditorBasis()`
// — the last text the viewer actually confirmed while a proposal is held
// — never `$("sql").value` (the model's own unconfirmed text) directly.
{
  assert.match(appClean, /function assistEditorBasis\(\) \{\s*return applyHold \? heldBasisSql : \$\("sql"\)\.value;\s*\}/, "assistEditorBasis falls back to heldBasisSql while held");
  const depsBody = stripComments(grabFn(appJs, "assistDeps"));
  assert.match(depsBody, /editorNeeds: neededExtensions\(assistEditorBasis\(\)\)/);
  assert.match(depsBody, /editorFileNames\(assistEditorBasis\(\)\)/);
  assert.match(depsBody, /const s = assistEditorBasis\(\);/);
  assert.doesNotMatch(depsBody, /neededExtensions\(\$\("sql"\)\.value\)/, "assistDeps must not read the live editor text directly");
  assert.doesNotMatch(depsBody, /editorFileNames\(\$\("sql"\)\.value\)/, "assistDeps must not read the live editor text directly");
  // the one place `heldBasisSql` is (conditionally — see L-8 below)
  // captured for a FRESH hold, right before apply() (assist-ui.js)
  // overwrites the editor with the proposal.
  assert.match(appClean, /if \(!applyHold && !heldTexts\.has\(v\) && !editorHeldByStatement\(v\)\) heldBasisSql = v;\s*\n\s*\}\s*\n\s*armHold\(\);/);
}

console.log("assist_ui_test: ok");
