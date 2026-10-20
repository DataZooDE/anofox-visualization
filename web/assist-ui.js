// The builder's assistant panel (#244, M1-3): the UI over web/assist.js's
// core (M1-2) and web/assist-engine.js's locked engine (#204) — the panel
// itself, "What will be sent", the proposal card, the preview bar's
// actions, the diff, apply/undo, the key dialog, the phone sheet, a11y.
//
// A pure-at-import ES module: it never touches `document`/`window` at
// import time (tests/web/assist_ui_test.mjs imports it under Node), only
// inside `mountAssistant(host)` and the functions it returns. It imports
// only ./assist.js, ./assist-engine.js and ./lint.js — never duckdb, never
// a dynamic `import()` of its own (the SDK loads through assist.js's
// `loadSdk`, the one dynamic import in the whole assistant, G11).
//
// R2 (model text is text only): every piece of model text — the reply, the
// summary, panel titles/why, open_issues, issue messages, the refusal
// explanation — goes through `textBlock()`: paragraphs split on blank
// lines, each a plain `<p>` with `.textContent`. There is no HTML sink in
// this module at all (no innerHTML/outerHTML/insertAdjacentHTML/
// document.write, no `.src =`/`setAttribute("src"` — a static grep test,
// G5/G6).
import { browserKeyTransport, buildContext, buildPrefix, describeError, isAbort, loadSdk, pathsOf, payloadPreview, proposalReaders, runAssistant, wasmTools } from "./assist.js";
import { buildEngine, pageFingerprint } from "./assist-engine.js";
import { ownOpts, splitStatements } from "./lint.js";

// ---- pure helpers (unit-tested; no DOM) ----

// CRLF -> LF, split on blank lines, trimmed, empties dropped, capped.
export function modelParagraphs(text) {
  const norm = String(text ?? "").replace(/\r\n/g, "\n");
  const parts = norm
    .split(/\n\s*\n/)
    .map((p) => p.trim())
    .filter(Boolean)
    .slice(0, 40);
  return parts.map((p) => (p.length > 4000 ? p.slice(0, 4000) : p));
}
// R2: the only way model text reaches the DOM — one <p> per paragraph,
// `.textContent` only. A single `\n` stays inside the text (CSS
// `white-space: pre-line` renders it); never innerHTML, never a link,
// never an image.
export function textBlock(doc, el, text) {
  for (const p of modelParagraphs(text)) {
    const node = doc.createElement("p");
    node.textContent = p;
    el.appendChild(node);
  }
}
// R3/G6: the only shape accepted as an Anthropic API key (sk-ant-…, at
// least 20 chars after the prefix). Trimmed first (a pasted key often
// carries leading/trailing whitespace); a newline *inside* the key still
// fails — the allowed charset has no `\n`, so `$` can never reach it.
export function keyLooksValid(k) {
  return /^sk-ant-[A-Za-z0-9_-]{20,}$/.test(String(k ?? "").trim());
}
// The span of each of `splitStatements(sql)`'s pieces inside `sql` itself,
// in order (a later piece is searched for only after the previous one, so
// a repeated statement's second copy gets its own, later span).
export function statementSpans(sql) {
  const s = String(sql ?? "");
  const pieces = splitStatements(s);
  const spans = [];
  let cursor = 0;
  for (const piece of pieces) {
    const idx = s.indexOf(piece, cursor);
    if (idx < 0) {
      spans.push([cursor, cursor]);
      continue;
    }
    spans.push([idx, idx + piece.length]);
    cursor = idx + piece.length;
  }
  return spans;
}
// setup | input | layout | panel, from one of `plan(sql)`'s entries and a
// token -> category map (roleCategories(), built from roles_json()).
export function classifyStmt(planEntry, roleCats) {
  if (!planEntry) return "panel";
  if (planEntry.setup) return "setup";
  const roles = (planEntry.roles || []).map((r) => r[1]);
  if (!roles.length) return "panel";
  const rc = roleCats || {};
  const cats = roles.map((r) => rc[r]);
  if (cats.some((c) => c === "input")) return "input";
  if (cats.length && cats.every((c) => c === "layout" || c === "chrome")) return "layout";
  return "panel";
}
// token -> category, from roles_json()'s own token and every alias.
export function roleCategories(rolesJson) {
  const r = typeof rolesJson === "string" ? JSON.parse(rolesJson) : rolesJson;
  const out = {};
  for (const role of r.roles || []) {
    out[role.token] = role.category;
    for (const a of role.aliases || []) out[a] = role.category;
  }
  return out;
}

// A longest-common-subsequence alignment of two sequences (by `===` on
// their items): [[i|null, j|null], …] in order — both set: unchanged or
// paired; `i` only: removed from `a`; `j` only: added in `b`. O(n*m), fine
// at the size of a dashboard script (statements) or one statement's lines.
function lcsAlign(a, b) {
  const n = a.length,
    m = b.length;
  const dp = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
  const out = [];
  let i = 0,
    j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      out.push([i, j]);
      i++;
      j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      out.push([i, null]);
      i++;
    } else {
      out.push([null, j]);
      j++;
    }
  }
  while (i < n) out.push([i, null]), i++;
  while (j < m) out.push([null, j]), j++;
  return out;
}
// A line-level diff (an LCS alignment dressed as unified-diff rows):
// [{op:" "|"+"|"-", text}, …]. Identity -> every row " "; an insertion/
// deletion -> "+"/"-" rows in order; the output's length and order always
// match a straightforward walk of the alignment (no reordering).
export function lineDiff(aLines, bLines) {
  const pairs = lcsAlign(aLines, bLines);
  return pairs.map(([i, j]) => (i != null && j != null ? { op: " ", text: aLines[i] } : j != null ? { op: "+", text: bLines[j] } : { op: "-", text: aLines[i] }));
}

// Comments and string/quoted-identifier literals blanked out (so a table
// name mentioned only in a comment or a string doesn't count as "in
// scope", and the LCS pairing below never partially-matches through a
// literal).
function sqlCodeLite(sql) {
  return String(sql ?? "").replace(/'(?:[^']|'')*'|"(?:[^"]|"")*"|--[^\n]*|\/\*[\s\S]*?\*\//g, " ");
}
const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

// The script's statements, paired old->new by an LCS over their
// whitespace-normalised text, dressed as the diff dialog's groups.
// `opts`: {plan (the wasm plan(script) export), roleCats, panels (the
// proposal's own panels, for `why`)}.
export function diffScripts(oldSql, newSql, opts = {}) {
  const { plan, roleCats, panels = [] } = ownOpts(opts);
  const oldStmts = oldSql ? splitStatements(oldSql) : [];
  const newStmts = splitStatements(newSql);
  const oldPlan = oldSql ? JSON.parse(plan(oldSql)) : [];
  const newPlan = JSON.parse(plan(newSql));
  const oldKind = oldStmts.map((_, i) => classifyStmt(oldPlan[i], roleCats));
  const newKind = newStmts.map((_, i) => classifyStmt(newPlan[i], roleCats));
  const norm = (s) => s.replace(/\s+/g, " ").trim();
  const pairs = lcsAlign(oldStmts.map(norm), newStmts.map(norm));
  const raw = [];
  for (const [oi, ni] of pairs) {
    if (oi != null && ni != null) {
      if (oldStmts[oi] === newStmts[ni]) raw.push({ op: "same", n: ni + 1, kind: newKind[ni] });
      else
        raw.push({
          op: "chg",
          n: ni + 1,
          kind: newKind[ni],
          oldText: oldStmts[oi],
          newText: newStmts[ni],
          lines: lineDiff(oldStmts[oi].split("\n"), newStmts[ni].split("\n")),
          why: panels.find((p) => p.stmt === ni + 1)?.why,
        });
    } else if (ni != null) {
      raw.push({ op: "add", n: ni + 1, kind: newKind[ni], newText: newStmts[ni], lines: lineDiff([], newStmts[ni].split("\n")), why: panels.find((p) => p.stmt === ni + 1)?.why });
    } else {
      raw.push({ op: "del", n: oi + 1, kind: oldKind[oi], oldText: oldStmts[oi], lines: lineDiff(oldStmts[oi].split("\n"), []) });
    }
  }
  const refined = refineAdjacentChanges(raw);
  const groups = [];
  for (const g of refined) {
    if (g.op === "same") {
      const last = groups[groups.length - 1];
      if (last && last.op === "same") last.count++;
      else groups.push({ op: "same", count: 1 });
    } else groups.push(g);
  }
  return { groups, summary: diffSummary(groups) };
}
// A removed statement immediately followed by an added one, at the LCS's
// alignment, reads as one changed statement (the usual diff-tool reading
// of a line replaced in place) rather than a separate deletion and
// addition — paired positionally within each adjacent run, any leftover
// (the run's longer side) staying a plain add/del.
function refineAdjacentChanges(raw) {
  const out = [];
  let i = 0;
  while (i < raw.length) {
    if (raw[i].op !== "del") {
      out.push(raw[i]);
      i++;
      continue;
    }
    const dels = [];
    while (i < raw.length && raw[i].op === "del") dels.push(raw[i++]);
    const adds = [];
    while (i < raw.length && raw[i].op === "add") adds.push(raw[i++]);
    const k = Math.min(dels.length, adds.length);
    for (let x = 0; x < k; x++) {
      const d = dels[x],
        a = adds[x];
      out.push({ op: "chg", n: a.n, kind: a.kind, oldText: d.oldText, newText: a.newText, lines: lineDiff(d.oldText.split("\n"), a.newText.split("\n")), why: a.why });
    }
    for (let x = k; x < dels.length; x++) out.push(dels[x]);
    for (let x = k; x < adds.length; x++) out.push(adds[x]);
  }
  return out;
}
function diffSummary(groups) {
  const panelWord = (n, w) => `${n} ${w}${n === 1 ? "" : "s"}`;
  const added = groups.filter((g) => g.op === "add" && g.kind === "panel");
  const removed = groups.filter((g) => g.op === "del" && g.kind === "panel");
  const changed = groups.filter((g) => g.op === "chg" && g.kind === "panel");
  const filterOps = groups.filter((g) => g.kind === "input" && g.op !== "same");
  // Only an actual edit to an existing setup/layout statement is called out
  // — building them fresh (add) or dropping them (del) isn't "changed".
  const setupChanged = groups.some((g) => g.kind === "setup" && g.op === "chg");
  const layoutChanged = groups.some((g) => g.kind === "layout" && g.op === "chg");
  const parts = [];
  if (added.length) parts.push(`${panelWord(added.length, "panel")} added`);
  if (changed.length) parts.push(changed.length <= 2 ? changed.map((g) => `panel ${g.n} changed`).join(", ") : `${changed.length} panels changed`);
  if (removed.length) parts.push(`${panelWord(removed.length, "panel")} removed`);
  if (filterOps.length) {
    const kinds = new Set(filterOps.map((g) => (g.op === "add" ? "added" : g.op === "del" ? "removed" : "changed")));
    parts.push(`${panelWord(filterOps.length, "filter")} ${[...kinds].join("/")}`);
  }
  if (setupChanged) parts.push("data setup changed");
  if (layoutChanged) parts.push("layout changed");
  return parts.length ? parts.join(", ") + "." : "No changes.";
}

// The tables in scope for a request: those the editor's own text names
// (literal, outside comments/strings), else all of them; the first 15 (in
// catalog order) go with their profiles, the rest by name only.
export function scopeTables(tables, editorSql) {
  const avail = (tables || []).filter((t) => !t.excluded);
  const code = sqlCodeLite(editorSql).toLowerCase();
  const named = avail.filter((t) => new RegExp(`\\b${escapeRe(String(t.name).toLowerCase())}\\b`).test(code));
  const chosen = named.length ? named : avail;
  return { inScope: chosen.slice(0, 15), rest: chosen.slice(15) };
}
// The chips button's text / aria-label (§5.2): what is in scope, at a
// glance, and whether common values and sample rows are included.
export function chipsText(opts = {}) {
  const { scope = [], cols = 0, commonValues = true } = ownOpts(opts);
  const names = scope.length ? scope.join(", ") : "no tables yet";
  return `${names} · ${cols} column${cols === 1 ? "" : "s"} · profile ✓ · common values ${commonValues ? "✓" : "✗"} · sample rows ✗`;
}
export function chipsLabel(opts = {}) {
  const { scope = [], cols = 0, commonValues = true } = ownOpts(opts);
  const names = scope.length ? scope.join(", ") : "no tables yet";
  return `In scope for the assistant: ${names}. ${cols} column${cols === 1 ? "" : "s"} profiled. Common values ${commonValues ? "included" : "not included"}. Sample rows are never sent.`;
}
const TOOL_LABEL = { list_tables: "Listing tables…", profile_table: "Profiling a table…", run_sql: "Running a query…", check_dashboard: "Checking the dashboard…" };
// The status line's text for one `onStatus`/internal status object.
export function statusText(s) {
  if (!s) return "";
  switch (s.phase) {
    case "thinking":
      return "Thinking…";
    case "tool":
      return TOOL_LABEL[s.tool] || "Working…";
    case "checking":
      return `Checking panels · round ${s.round} of ${s.maxRounds}`;
    case "engine":
      return `Copying ${s.fq} for the assistant…`;
    case "profile":
      return `Profiling ${s.name} (${s.i} of ${s.n})…`;
    case "writing":
      return `Writing the dashboard… ${s.lines} line${s.lines === 1 ? "" : "s"}`;
    default:
      return "";
  }
}
// isEngineReset(text): the engine-state watcher's trigger (web/assist-engine.js
// L-1, web/assist.js's locked() re-check) — also used here to detect it in a
// preview query's own rejection.
export function isEngineReset(text) {
  return /assistant engine changed state|Cannot access directory/.test(String(text ?? ""));
}
// Apply's dialog model (D2): with a reader or a refused (class-3) statement,
// or a stale editor, ask first; otherwise one click. `readers`/`refused`
// are never trusted blindly — this only decides the *dialog*, never skips
// `tools.gate()` itself (that always runs first, in the engine).
export function applyModel(opts = {}) {
  const { readers = [], refused = [], stale = false } = ownOpts(opts);
  const needsDialog = readers.length > 0 || refused.length > 0 || !!stale;
  const buttons = refused.length
    ? [
        { value: "cancel", label: "Cancel" },
        { value: "norun", label: "Apply, don't run" },
        { value: "run", label: "Apply and run", primary: true, danger: true },
      ]
    : [
        { value: "cancel", label: "Cancel" },
        { value: "run", label: "Apply", primary: true },
      ];
  return { needsDialog, title: "Apply this dashboard?", buttons, sections: { readers, refused, stale } };
}
// In-memory undo stack (at most 20 entries) for Apply: {before, after, title}.
export function makeHistory() {
  const stack = [];
  return {
    push(entry) {
      stack.push(entry);
      if (stack.length > 20) stack.shift();
    },
    last() {
      return stack.length ? stack[stack.length - 1] : null;
    },
    canUndo(editorText) {
      const e = stack[stack.length - 1];
      return !!e && editorText === e.after;
    },
    pop() {
      return stack.length ? stack.pop() : null;
    },
  };
}

// ---- the panel (DOM) ----

const CONSOLE_KEYS_URL = "https://console.anthropic.com/settings/keys";
const SIDE_DRAWER = "(max-width: 820px)";
const PHONE_INERT = "header, #sidebar, .toolbar, #ed-sql, .splitter, .dash, #explore";

// mountAssistant(host) -> {open(opts), close(), toggle(), isOpen()}. The
// only thing app.js's openAssistant() calls; `host` is built fresh
// (assistHost() in app.js) and never put on `window` (G12).
export function mountAssistant(hostArg) {
  const host = hostArg;
  const doc = document;
  const pane = doc.getElementById("assist-pane");

  const S = {
    open: false,
    busy: false,
    ctl: null,
    engine: null,
    engineFp: null,
    profiles: new Map(),
    prefix: null,
    Anthropic: null,
    conv: { id: 0, log: [], skipConsent: false, history: null },
    commonValues: true,
    model: "default",
    proposal: null,
    tools: null,
    requestEditor: "",
    session: null,
    lastPayload: null,
    toolLog: [],
    hist: makeHistory(),
    engineReset: false,
    rolesJson: null,
    roleCats: null,
  };

  // ---- the static panel DOM (built once) ----
  const el = {};
  const mk = (tag, cls, attrs) => {
    const n = doc.createElement(tag);
    if (cls) n.className = cls;
    if (attrs) for (const [k, v] of Object.entries(attrs)) n.setAttribute(k, v);
    return n;
  };
  function build() {
    pane.replaceChildren();
    const head = mk("div", "ai-head");
    const title = mk("h2");
    title.id = "ai-title";
    title.textContent = "Assistant";
    const newBtn = mk("button", "btn2", { type: "button" });
    newBtn.id = "ai-new";
    newBtn.textContent = "New conversation";
    const forgetBtn = mk("button", "btn2", { type: "button" });
    forgetBtn.id = "ai-forget";
    forgetBtn.textContent = "Forget key";
    const closeBtn = mk("button", "btn2 icon", { type: "button", "aria-label": "Close the assistant" });
    closeBtn.id = "ai-close";
    closeBtn.textContent = "✕";
    head.append(title, newBtn, forgetBtn, closeBtn);

    const ctx = mk("div", "ai-ctx");
    const chips = mk("button", "ai-chips", { type: "button" });
    chips.id = "ai-chips";
    chips.textContent = chipsText({});
    chips.setAttribute("aria-label", chipsLabel({}));
    const commonLbl = mk("label", "ai-opt");
    const commonInput = mk("input", "", { type: "checkbox" });
    commonInput.id = "ai-common";
    commonInput.checked = true;
    commonLbl.append(commonInput, " Include common values");
    ctx.append(chips, commonLbl);

    const notice = mk("div", "ai-notice");
    notice.id = "ai-notice";
    notice.hidden = true;
    const noticeText = mk("span");
    noticeText.textContent = "Add your API key to start";
    const noticeBtn = mk("button", "btn2", { type: "button" });
    noticeBtn.textContent = "Add key";
    notice.append(noticeText, noticeBtn);

    // A <div>, not <ol>/<li> (axe: role="log" overrides the ol's implicit
    // "list" role, and a child with role="listitem" then fails
    // aria-required-parent — there is no tag/role combination that is both
    // a list and a log; "log" is the one the live region actually needs).
    const log = mk("div", "ai-log", { role: "log", "aria-label": "Conversation", "aria-live": "polite", tabindex: "0" });
    log.id = "ai-log";
    const stream = mk("div", "ai-stream", { "aria-hidden": "true" });

    const card = mk("section", "ai-card", { "aria-labelledby": "ai-card-title" });
    card.id = "ai-card";
    card.hidden = true;
    const cardTitle = mk("h3");
    cardTitle.id = "ai-card-title";
    cardTitle.tabIndex = -1;
    const sum = mk("p", "ai-sum");
    const reads = mk("p", "ai-reads");
    const panelsList = mk("ol", "ai-panels");
    const issuesBox = mk("div", "ai-issues");
    const openBox = mk("div", "ai-open");
    const actions = mk("div", "ai-actions");
    const previewBtn = mk("button", "btn2", { type: "button" });
    previewBtn.id = "ai-preview";
    previewBtn.textContent = "Preview again";
    const diffBtn = mk("button", "btn2", { type: "button" });
    diffBtn.id = "ai-diff";
    diffBtn.textContent = "Show changes";
    const discardBtn = mk("button", "btn2", { type: "button" });
    discardBtn.id = "ai-discard";
    discardBtn.textContent = "Discard";
    const applyBtn = mk("button", "", { type: "button" });
    applyBtn.id = "ai-apply";
    applyBtn.textContent = "Apply";
    const undoBtn = mk("button", "btn2", { type: "button" });
    undoBtn.id = "ai-undo";
    undoBtn.textContent = "Undo";
    actions.append(previewBtn, diffBtn, discardBtn, applyBtn, undoBtn);
    card.append(cardTitle, sum, reads, panelsList, issuesBox, openBox, actions);

    const statusline = mk("div", "ai-statusline");
    const statusEl = mk("span", "", { role: "status", "aria-live": "polite" });
    statusEl.id = "ai-status";
    const cancelBtn = mk("button", "btn2", { type: "button" });
    cancelBtn.id = "ai-cancel";
    cancelBtn.hidden = true;
    cancelBtn.textContent = "Cancel";
    statusline.append(statusEl, cancelBtn);

    const debugLine = mk("p", "ai-debug");

    const form = mk("form", "ai-form");
    form.id = "ai-form";
    const qLabel = mk("label", "sr-only", { for: "ai-q" });
    qLabel.textContent = "Describe the dashboard you want";
    const q = mk("textarea", "", { rows: "3", placeholder: "Describe the dashboard you want…" });
    q.id = "ai-q";
    const formRow = mk("div", "ai-form-row");
    const modelLbl = mk("label");
    modelLbl.textContent = "Model ";
    const modelSel = mk("select");
    modelSel.id = "ai-model";
    const optDefault = mk("option", "", { value: "default" });
    optDefault.textContent = "Standard";
    const optHarder = mk("option", "", { value: "harder" });
    optHarder.textContent = "Try harder (about 2× the cost)";
    modelSel.append(optDefault, optHarder);
    modelLbl.append(modelSel);
    const sendBtn = mk("button", "", { type: "submit" });
    sendBtn.id = "ai-send";
    sendBtn.textContent = "Send";
    formRow.append(modelLbl, sendBtn);
    form.append(qLabel, q, formRow);

    pane.append(head, ctx, notice, log, stream, card, statusline, debugLine, form);

    Object.assign(el, {
      title,
      newBtn,
      forgetBtn,
      closeBtn,
      chips,
      commonInput,
      notice,
      noticeBtn,
      log,
      stream,
      card,
      cardTitle,
      sum,
      reads,
      panelsList,
      issuesBox,
      openBox,
      previewBtn,
      diffBtn,
      discardBtn,
      applyBtn,
      undoBtn,
      statusEl,
      cancelBtn,
      debugLine,
      q,
      modelSel,
      sendBtn,
      form,
    });
    wirePanel();
  }

  // ---- the lazy stylesheet (mount waits for its `load` before showing) ----
  let cssReady = null;
  function ensureCss() {
    if (cssReady) return cssReady;
    cssReady = new Promise((resolve) => {
      const href = new URL("./assist-ui.css", import.meta.url).href;
      const existing = doc.querySelector(`link[href="${href}"]`);
      if (existing) {
        resolve();
        return;
      }
      const link = doc.createElement("link");
      link.rel = "stylesheet";
      link.setAttribute("href", href);
      link.onload = () => resolve();
      link.onerror = () => resolve(); // still show the panel, unstyled, rather than never
      doc.head.appendChild(link);
    });
    return cssReady;
  }

  // ---- small dialog helper (app.js's dpDialog markup/classes) ----
  // Info-1 (sec-review-272): a destructured-in-signature parameter reads
  // Object.prototype too (a polluted prototype's `buttons`/`cancel`/…
  // would pass unseen) — ownOpts() first, like every other options bag.
  function dialog(opts) {
    const { title, body, buttons, cancel = null, size = null } = ownOpts(opts);
    return new Promise((resolve) => {
      const opener = doc.activeElement;
      const veil = mk("div", "md-dialog dp-dialog");
      const card = mk("div", "md-card dp-dialog-card" + (size === "wide" ? " ai-dlg-wide" : size === "full" ? " ai-dlg-full" : ""), { role: "dialog", "aria-modal": "true" });
      const hid = "ai-dlg-title-" + Math.random().toString(36).slice(2);
      card.setAttribute("aria-labelledby", hid);
      const h = mk("h3");
      h.id = hid;
      h.textContent = title;
      card.appendChild(h);
      if (body) card.appendChild(body);
      const acts = mk("div", "md-actions dp-dialog-actions");
      const spacer = mk("span");
      spacer.style.flex = "1";
      acts.appendChild(spacer);
      let checked = false;
      const done = (value) => {
        veil.remove();
        doc.removeEventListener("keydown", onKey, true);
        if (opener && opener.isConnected && typeof opener.focus === "function") opener.focus();
        resolve({ value, checked });
      };
      let primary = null;
      for (const b of buttons) {
        const btn = mk("button", b.primary ? "dp-dialog-primary" + (b.danger ? " danger" : "") : "btn2", { type: "button" });
        btn.textContent = b.label;
        btn.onclick = () => done(b.value);
        if (b.primary) primary = btn;
        acts.appendChild(btn);
      }
      card.appendChild(acts);
      veil.appendChild(card);
      veil.addEventListener("mousedown", (e) => e.target === veil && done(cancel));
      const onKey = (e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          done(cancel);
        } else if (e.key === "Tab") {
          const f = [...card.querySelectorAll('button, a[href], input, textarea, select, [tabindex="0"], summary')].filter((x) => x.offsetParent || x === doc.activeElement);
          if (!f.length) return;
          const i = f.indexOf(doc.activeElement);
          const j = i < 0 ? 0 : (i + (e.shiftKey ? -1 : 1) + f.length) % f.length;
          e.preventDefault();
          e.stopPropagation();
          f[j].focus();
        }
      };
      doc.addEventListener("keydown", onKey, true);
      doc.body.appendChild(veil);
      card.setAttribute("data-checked", "0");
      card._setChecked = (v) => (checked = v);
      (primary || card.querySelector("button"))?.focus();
    });
  }

  function textP(text) {
    const p = mk("p");
    p.textContent = text;
    return p;
  }
  // The one link this module ever creates, and the one place `.href` is
  // ever assigned — always the fixed console address, never anything
  // derived from model text or user input (G5).
  function consoleKeysLink(text) {
    const a = mk("a", "", { target: "_blank", rel: "noopener noreferrer" });
    a.href = CONSOLE_KEYS_URL;
    a.textContent = text;
    return a;
  }

  async function keyDialog() {
    const body = doc.createDocumentFragment();
    body.append(
      textP(
        "The assistant calls Claude from this browser with your own key. The key stays in this tab and is sent only to api.anthropic.com. It is never put in the SQL, a share link, an export or a saved dashboard."
      )
    );
    const p2 = mk("p");
    p2.append("Create a separate key with a low spend limit. ", consoleKeysLink("Open the Anthropic console"));
    body.append(p2);
    body.append(textP("While a key is stored, this tab runs under a strict security policy: images, fonts and data from other sites are blocked until you remove the key and reload."));
    const label = mk("label");
    label.append("API key ");
    const input = mk("input", "", { type: "password", autocomplete: "off", spellcheck: "false" });
    label.appendChild(input);
    body.appendChild(label);
    let checkbox = null;
    if (host.config.remember) {
      const l2 = mk("label", "ai-opt");
      checkbox = mk("input", "", { type: "checkbox" });
      l2.append(checkbox, " Remember on this device");
      body.appendChild(l2);
      body.append(textP("Any page on this site can read it."));
    }
    const err = mk("p", "ai-err", { role: "alert" });
    body.appendChild(err);
    return new Promise((resolve) => {
      const tryResolve = async () => {
        const r = await dialog({
          title: "Your Anthropic API key",
          body,
          buttons: [
            { value: "cancel", label: "Cancel" },
            { value: "save", label: "Save key", primary: true },
          ],
          cancel: "cancel",
        });
        if (r.value !== "save") {
          resolve(false);
          return;
        }
        if (!keyLooksValid(input.value)) {
          err.textContent = "That doesn't look like an Anthropic API key (it starts with sk-ant-).";
          body.appendChild(err);
          // re-show (the dialog already closed) with the error kept
          const again = await keyDialog2(body, input, err, checkbox);
          resolve(again);
          return;
        }
        host.keyStore().set(input.value, !!checkbox?.checked);
        input.value = "";
        resolve(true);
      };
      tryResolve();
    });
  }
  // A re-shown key dialog after a validation error (keeps the same fields).
  async function keyDialog2(body, input, err, checkbox) {
    for (;;) {
      const r = await dialog({
        title: "Your Anthropic API key",
        body,
        buttons: [
          { value: "cancel", label: "Cancel" },
          { value: "save", label: "Save key", primary: true },
        ],
        cancel: "cancel",
      });
      if (r.value !== "save") return false;
      if (!keyLooksValid(input.value)) {
        err.textContent = "That doesn't look like an Anthropic API key (it starts with sk-ant-).";
        continue;
      }
      host.keyStore().set(input.value, !!checkbox?.checked);
      input.value = "";
      return true;
    }
  }

  function kb(n) {
    return `${Math.max(1, Math.round(n / 1024))} KB`;
  }
  async function sentDialog(payload, engine, opts = {}) {
    const { send } = ownOpts(opts);
    const body = doc.createDocumentFragment();
    body.append(textP(`To: ${payload.destination}`));
    body.append(textP(payload.statics.map((s) => `${s.name} (${kb(s.bytes)})`).join(", ")));
    body.append(textP(`About ${payload.approxTokens} tokens`));
    const pre = mk("pre", "ai-pre", { tabindex: "0" });
    pre.textContent = payload.dynamic;
    body.appendChild(pre);
    if (payload.redacted) body.append(textP(`${payload.redacted} secret${payload.redacted === 1 ? "" : "s"} in your SQL were replaced by ***.`));
    for (const t of engine?.tables || []) {
      const notes = [t.sampled && "sampled", t.truncated && "truncated", t.types_changed && "types changed", t.excluded && `excluded: ${t.excluded}`].filter(Boolean);
      if (notes.length) body.append(textP(`${t.fq}: ${notes.join(", ")}`));
    }
    body.append(textP("The assistant works on a copy of your tables in a sandbox without network access."));
    if (payload.note) body.append(textP(payload.note));
    for (const t of payload.tools || []) {
      const det = mk("details");
      const summary = mk("summary");
      summary.textContent = t.name;
      const pre2 = mk("pre");
      pre2.textContent = t.content;
      det.append(summary, pre2);
      body.appendChild(det);
    }
    let checkbox = null;
    if (send) {
      const l = mk("label", "ai-opt");
      checkbox = mk("input", "", { type: "checkbox" });
      l.append(checkbox, " Don't ask again in this conversation");
      body.appendChild(l);
    }
    const buttons = send
      ? [
          { value: "cancel", label: "Cancel" },
          { value: "send", label: "Send", primary: true },
        ]
      : [{ value: "close", label: "Close" }];
    const r = await dialog({ title: "What will be sent", body, buttons, cancel: send ? "cancel" : "close", size: "wide" });
    return { value: r.value, checked: !!checkbox?.checked };
  }

  async function diffDialog() {
    const { groups, summary } = diffScripts(S.requestEditor, S.proposal.sql, { plan: host.wasm.plan, roleCats: ensureRoleCats(), panels: S.proposal.panels || [] });
    const body = doc.createDocumentFragment();
    body.append(textP(summary));
    for (const g of groups) {
      if (g.op === "same") {
        const det = mk("details");
        const summary2 = mk("summary");
        summary2.textContent = `${g.count} unchanged statement${g.count === 1 ? "" : "s"}`;
        det.appendChild(summary2);
        body.appendChild(det);
        continue;
      }
      const sec = mk("section");
      const h3 = mk("h3");
      const kindWord = { add: "added", del: "removed", chg: "changed" }[g.op] || g.op;
      h3.textContent = `Statement ${g.n} · ${kindWord}`;
      sec.appendChild(h3);
      if (g.why) {
        const p = mk("p", "ai-why");
        p.textContent = g.why;
        sec.appendChild(p);
      }
      const pre = mk("pre", "ai-diff");
      for (const line of g.lines || []) {
        if (line.op === "-") {
          const del = mk("del");
          const sr = mk("span", "sr-only");
          sr.textContent = "removed: ";
          del.append(sr, line.text);
          pre.appendChild(del);
          pre.appendChild(doc.createTextNode("\n"));
        } else if (line.op === "+") {
          const ins = mk("ins");
          const sr = mk("span", "sr-only");
          sr.textContent = "added: ";
          ins.append(sr, line.text);
          pre.appendChild(ins);
          pre.appendChild(doc.createTextNode("\n"));
        } else {
          pre.appendChild(doc.createTextNode(line.text + "\n"));
        }
      }
      sec.appendChild(pre);
      body.appendChild(sec);
    }
    await dialog({ title: "Changes", body, buttons: [{ value: "close", label: "Close" }], cancel: "close", size: host.ui.isPhone() ? "full" : "wide" });
  }

  async function applyDialog(m) {
    const body = doc.createDocumentFragment();
    if (m.sections.stale) body.append(textP("Your SQL changed since you asked. Applying replaces it."));
    const readersP = mk("p");
    readersP.textContent = "It reads these files and URLs:";
    body.appendChild(readersP);
    if (m.sections.readers.length) {
      const ul = mk("ul");
      for (const r of m.sections.readers) {
        const li = mk("li");
        li.textContent = r;
        ul.appendChild(li);
      }
      body.appendChild(ul);
    }
    if (m.sections.refused.length) {
      body.append(textP("It also runs statements the assistant may not run by itself:"));
      const ul = mk("ul", "ai-refused");
      for (const r of m.sections.refused) {
        const li = mk("li");
        const reasonP = mk("p", "ai-why");
        reasonP.textContent = r.reason || "";
        // M-1: the statement in full (whitespace collapsed, a scrollable
        // <pre> of text nodes — never innerHTML), its keyword in <strong>,
        // capped so one huge statement can't push the dialog off-screen.
        const pre = mk("pre", "ai-pre");
        pre.tabIndex = 0;
        const collapsed = String(r.sql).replace(/\s+/g, " ").trim();
        const CAP = 2000;
        const shown = collapsed.length > CAP ? collapsed.slice(0, CAP) : collapsed;
        const kw = r.keyword || "";
        if (kw && shown.toUpperCase().startsWith(kw)) {
          const strong = mk("strong");
          strong.textContent = shown.slice(0, kw.length);
          pre.appendChild(strong);
          pre.appendChild(doc.createTextNode(shown.slice(kw.length)));
        } else {
          pre.appendChild(doc.createTextNode(shown));
        }
        if (collapsed.length > CAP) pre.appendChild(doc.createTextNode(`\n… ${collapsed.length - CAP} more characters`));
        li.append(reasonP, pre);
        ul.appendChild(li);
      }
      body.appendChild(ul);
      body.append(textP('"Apply, don\'t run" puts the SQL in the editor without running it; nothing runs until you press Run.'));
    }
    const r = await dialog({ title: m.title, body, buttons: m.buttons, cancel: "cancel", size: host.ui.isPhone() ? "full" : "wide" });
    return r.value === "cancel" ? null : r.value;
  }

  // ---- role categories / prefix (memoised) ----
  function ensureRoleCats() {
    if (!S.roleCats) {
      S.rolesJson = host.wasm.rolesJson();
      S.roleCats = roleCategories(S.rolesJson);
    }
    return S.roleCats;
  }
  let prefixP = null;
  async function ensurePrefix() {
    if (prefixP) return prefixP;
    prefixP = (async () => {
      const [instructions, guide] = await Promise.all([
        fetch(new URL("./assist/instructions.md", import.meta.url)).then((r) => r.text()),
        fetch(new URL("./assist/guide.md", import.meta.url)).then((r) => r.text()),
      ]);
      return buildPrefix({ instructions, guide, rolesJson: host.wasm.rolesJson(), version: host.wasm.version() });
    })();
    return prefixP;
  }
  async function ensureTransport() {
    S.Anthropic ||= await loadSdk();
    return browserKeyTransport({ Anthropic: S.Anthropic, apiKey: host.keyStore().get() });
  }

  // ---- the engine (per-request, fingerprinted) ----
  // A build in flight for the engine's current generation (keyed by `fp`):
  // a second call that lands while one is still running (e.g. the lock
  // re-check/profile step of one send overlapping the next, or two rapid
  // Sends) awaits the same promise instead of racing a second
  // close()+rebuild against the first — closing an engine that the other
  // call is still mid-build with would leak its Worker and could leave
  // `S.engine` pointing at a terminated instance.
  let engineBuildFp = null,
    engineBuildP = null;
  async function ensureEngine(signal) {
    if (host.preview.active()) await host.preview.exit({ rerun: true });
    await host.deps.prepareFiles();
    const ext = host.deps.extensions();
    const files = host.deps.files();
    const fp = await pageFingerprint(host.deps.page, { extensions: ext, files });
    if (fp === S.engineFp && S.engine) return S.engine;
    if (engineBuildP && engineBuildFp === fp) return engineBuildP;
    engineBuildFp = fp;
    engineBuildP = (async () => {
      S.engine?.close();
      S.profiles = new Map();
      try {
        S.engine = await buildEngine({ makeDb: host.deps.makeDb, page: host.deps.page, extensions: ext, files, signal, onProgress: ({ fq }) => setStatus({ phase: "engine", fq }) });
      } catch (e) {
        S.engine = null;
        S.engineFp = null;
        throw new Error(`The assistant could not prepare its copy of your data: ${e?.message || e}`);
      }
      S.engineFp = fp;
      return S.engine;
    })();
    try {
      return await engineBuildP;
    } finally {
      engineBuildP = null;
      engineBuildFp = null;
    }
  }
  function makeTools(engine, editorSql) {
    return wasmTools({
      db: engine.db,
      lintStep: host.wasm.lintStep,
      plan: host.wasm.plan,
      pageSql: host.wasm.pageSql,
      known: new Set(pathsOf(editorSql)),
      commonValues: () => S.commonValues,
      sampleRows: () => false,
      profiles: S.profiles,
      snapshot: new Map(engine.tables.map((t) => [t.fq, t])),
      extensions: engine.extensions,
    });
  }
  async function buildCtx(tools, engine, editorSql) {
    const { inScope, rest } = scopeTables(engine.tables, editorSql);
    const cardsOf = new Map();
    let i = 0;
    for (const t of inScope) {
      i++;
      setStatus({ phase: "profile", name: t.fq, i, n: inScope.length });
      await tools.profile_table({ table: t.fq });
      cardsOf.set(t.fq, S.profiles.get(t.fq)?.cards);
    }
    const tables = [...inScope.map((t) => ({ name: t.fq.replace(/"/g, ""), rows: t.rows, cards: cardsOf.get(t.fq) })), ...rest.map((t) => ({ name: t.fq.replace(/"/g, ""), rows: t.rows }))];
    if (el.chips) {
      const scopeNames = inScope.map((t) => t.fq.replace(/"/g, ""));
      const cols = inScope.reduce((n, t) => n + (cardsOf.get(t.fq)?.length || 0), 0);
      el.chips.textContent = chipsText({ scope: scopeNames, cols, commonValues: S.commonValues });
      el.chips.setAttribute("aria-label", chipsLabel({ scope: scopeNames, cols, commonValues: S.commonValues }));
    }
    return buildContext({ mode: editorSql.trim() ? "edit" : "new", tables, sql: editorSql, spans: statementSpans(editorSql), commonValues: S.commonValues });
  }

  // ---- status / log / stream ----
  function setStatus(s) {
    el.statusEl.textContent = statusText(s) || "";
  }
  function logEntry(cls, text, extraBtn) {
    const li = mk("div", "ai-msg " + cls);
    textBlock(doc, li, text);
    if (extraBtn) li.appendChild(extraBtn);
    el.log.appendChild(li);
    el.log.scrollTop = el.log.scrollHeight;
    return li;
  }
  function streamDelta(d) {
    el.stream.textContent += d;
  }
  function clearStream() {
    el.stream.textContent = "";
  }

  // ---- the proposal card ----
  function renderCard() {
    const p = S.proposal;
    el.card.hidden = !p;
    if (!p) return;
    el.cardTitle.textContent = p.applied ? "Applied" : p.issues?.length ? "Proposal with open issues" : "Proposal";
    el.sum.replaceChildren();
    textBlock(doc, el.sum, p.summary || "");
    el.reads.replaceChildren();
    const readers = proposalReaders(p.sql);
    el.reads.textContent = readers.length ? `Reads: ${readers.join(", ")}` : "Reads no files or URLs: only tables in your database.";
    el.panelsList.replaceChildren();
    for (const panel of p.panels || []) {
      const li = mk("li");
      const strong = mk("strong");
      strong.textContent = panel.title || "";
      const span = mk("span");
      span.textContent = panel.why ? " — " + panel.why : "";
      li.append(strong, span);
      el.panelsList.appendChild(li);
    }
    el.issuesBox.replaceChildren();
    if (p.issues?.length) {
      const h4 = mk("h4");
      h4.textContent = "Unresolved";
      const ul = mk("ul");
      for (const i of p.issues) {
        const li = mk("li");
        li.textContent = `Statement ${i.stmt}, ${i.code}: ${i.message}`;
        ul.appendChild(li);
      }
      el.issuesBox.append(h4, ul);
    }
    el.openBox.replaceChildren();
    if (p.open_issues?.length) {
      const h4 = mk("h4");
      h4.textContent = "The assistant notes";
      const ul = mk("ul");
      for (const t of p.open_issues) {
        const li = mk("li");
        li.textContent = t;
        ul.appendChild(li);
      }
      el.openBox.append(h4, ul);
    }
    el.previewBtn.hidden = host.preview.active();
    el.undoBtn.hidden = !p.applied;
    el.undoBtn.disabled = !S.hist.last();
  }

  // ---- the flows ----
  async function open(opts = {}) {
    // Info-1: read through ownOpts, not a bare opts.request/opts.send (a
    // polluted Object.prototype.send would auto-send without the consent
    // dialog's own skip-consent checkbox ever being set — no pollution
    // source was found, but the read itself should be safe regardless).
    // `send` is renamed on the way out: the outer send() function below
    // must not be shadowed by the option of the same name.
    const { request, send: wantSend } = ownOpts(opts);
    await ensureCss();
    if (!el.title) build();
    S.open = true;
    pane.hidden = false;
    document.getElementById("assist-btn")?.setAttribute("aria-expanded", "true");
    document.getElementById("tab-assist")?.setAttribute("aria-selected", "true");
    syncPhone(true);
    const ks = host.keyStore();
    if (!ks.get()) {
      const saved = await keyDialog();
      if (!saved) {
        el.notice.hidden = false;
        el.noticeBtn.onclick = () => keyDialog().then((ok) => ok && (el.notice.hidden = true));
        return;
      }
    }
    if (request) el.q.value = request;
    if (wantSend) await send(request);
    else el.q.focus();
  }
  function close(opts = {}) {
    const { toPreview } = ownOpts(opts);
    pane.hidden = true;
    syncPhone(false);
    S.open = false;
    document.getElementById("assist-btn")?.setAttribute("aria-expanded", "false");
    document.getElementById("tab-assist")?.setAttribute("aria-selected", "false");
    if (!toPreview) host.ui.opener()?.focus?.();
  }
  function toggle() {
    if (S.open) close();
    else open({});
  }
  function isOpen() {
    return S.open;
  }

  function syncPhone(opening) {
    const phone = host.ui.isPhone();
    if (!phone) return;
    if (opening) {
      pane.setAttribute("role", "dialog");
      pane.setAttribute("aria-modal", "true");
      for (const sel of PHONE_INERT.split(",").map((s) => s.trim())) doc.querySelectorAll(sel).forEach((n) => (n.inert = true));
      el.q?.focus();
    } else {
      pane.removeAttribute("role");
      pane.removeAttribute("aria-modal");
      for (const sel of PHONE_INERT.split(",").map((s) => s.trim())) doc.querySelectorAll(sel).forEach((n) => (n.inert = false));
    }
  }

  async function send(forcedRequest) {
    if (S.busy) return;
    const request = (forcedRequest ?? el.q.value).trim();
    if (!request) return;
    S.busy = true;
    el.sendBtn.hidden = true;
    el.cancelBtn.hidden = false;
    S.ctl = new AbortController();
    logEntry("ai-user", request);
    el.q.value = "";
    try {
      const engine = await ensureEngine(S.ctl.signal);
      const editor = host.editor.get();
      S.requestEditor = editor;
      const tools = makeTools(engine, editor);
      const ctx = await buildCtx(tools, engine, editor);
      const prefix = await ensurePrefix();
      S.model = el.modelSel.value === "harder" ? "harder" : "default";
      S.lastPayload = payloadPreview({ prefix, ctx, request, destination: "https://api.anthropic.com", toolLog: S.toolLog });
      if (!S.conv.skipConsent) {
        const r = await sentDialog(S.lastPayload, engine, { send: true });
        if (r.checked) S.conv.skipConsent = true;
        if (r.value !== "send") {
          el.log.lastChild?.remove();
          el.q.value = request;
          setStatus(null);
          el.statusEl.textContent = "Not sent.";
          return;
        }
      }
      const transport = await ensureTransport();
      const r = await runAssistant({
        transport,
        prefix,
        model: host.config.models[S.model],
        ctx,
        request,
        history: S.conv.history || [],
        tools,
        limits: { rounds: host.config.maxRounds },
        signal: S.ctl.signal,
        onStatus: setStatus,
        onText: streamDelta,
        onProposalInput: (snap) => setStatus({ phase: "writing", lines: String(snap?.sql || "").split("\n").length }),
        onToolResult: (e) => {
          S.toolLog.push(e);
          if (isEngineReset(e.content)) {
            S.engineReset = true;
            S.ctl.abort();
          }
        },
      });
      clearStream();
      if (S.engineReset) {
        S.engine?.close();
        S.engine = null;
        S.engineFp = null;
        S.engineReset = false;
        logEntry("ai-note", "The assistant's sandbox changed state and was reset. Send again.");
      }
      if (r.kind === "proposal" || (r.kind === "stopped" && r.proposal)) {
        if (r.text) logEntry("ai-bot", r.text);
        await showProposal(r.proposal, tools);
        S.conv.history = null;
      } else if (r.kind === "reply") {
        logEntry("ai-bot", r.text);
        S.conv.history = r.messages;
      } else if (r.kind === "refusal") {
        logEntry("ai-note", "The model declined this request. " + (r.reason || ""));
        S.conv.history = null;
      } else if (r.kind === "stopped") {
        logEntry("ai-note", r.reason || "Stopped.");
      } else if (r.kind === "cancelled") {
        logEntry("ai-note", "Cancelled.");
      } else if (r.kind === "error") {
        const btn = r.status === 401 ? mk("button", "btn2", { type: "button" }) : null;
        if (btn) {
          btn.textContent = "Change key";
          btn.onclick = () => keyDialog();
        }
        logEntry("ai-note", r.reason, btn);
      }
      if (host.debug && r.usage) {
        el.debugLine.textContent = `in ${r.usage.input_tokens} · cache read ${r.usage.cache_read_input_tokens} · write ${r.usage.cache_creation_input_tokens} · out ${r.usage.output_tokens} · ${r.turns} calls · ${r.rounds} rounds`;
      }
    } catch (e) {
      if (!isAbort(e, S.ctl?.signal)) logEntry("ai-note", describeError(e));
      else logEntry("ai-note", "Cancelled.");
    } finally {
      S.busy = false;
      el.sendBtn.hidden = false;
      el.cancelBtn.hidden = true;
      if (!el.statusEl.textContent) setStatus(null);
    }
  }

  // L-1 (sec-review-272): the one place a session is ever closed — called
  // from every exit path (enterPreview's own start, discardPreview,
  // host.onPreviewExit) so "Preview again", typing, loadDash, Explore and a
  // new Send each close at most once (S.session is nulled first, so a
  // second call anywhere else is a no-op) instead of leaking a CONNECT per
  // "Preview again" click.
  function closeSession() {
    const s = S.session;
    S.session = null;
    s?.close();
  }
  async function showProposal(p, tools) {
    S.proposal = p;
    S.tools = tools;
    renderCard();
    el.cardTitle.focus();
    await enterPreview();
  }
  async function enterPreview() {
    // Info-4 (sec-review-272-rereview): exit any already-active preview
    // FIRST, before the new session even exists. host.preview.enter()
    // (awaited below) does its own exit when one is active, but by then
    // S.session already held the NEW session — its exit fires
    // host.onPreviewExit -> closeSession() here, which closed the BRAND
    // NEW session instead of the old one (a second Send while previewing
    // left every panel's `run` calling S.session.run on null). Closing
    // the old one here means closeSession() just below is a no-op.
    if (host.preview.active()) await host.preview.exit({ rerun: false });
    closeSession();
    S.session = S.tools.previewSession(S.proposal.sql);
    const run = async (sql) => {
      try {
        return await S.session.run(sql);
      } catch (e) {
        if (isEngineReset(e?.message)) {
          discardPreview();
          logEntry("ai-note", "The assistant's sandbox changed state while previewing. The preview was discarded.");
        }
        throw e;
      }
    };
    // L-1: hide "Preview again" right away — host.preview.enter() awaits
    // several things (any run in flight, the lazy tail, then the preview's
    // own render) before `preview` actually becomes truthy, and renderCard()
    // (called by the proposal-card flow right after this) must not show it
    // live again meanwhile.
    if (el.previewBtn) el.previewBtn.hidden = true;
    const entered = host.preview.enter({ sql: S.proposal.sql, run });
    // The phone sheet's own close + the preview title's focus are the
    // immediate UI feedback for "Send" (sec-review-272 regression check:
    // this must not wait for `entered`, which only resolves once the
    // preview has actually rendered — gating the sheet close on that left
    // the full-screen panel up, unresponsive, for the whole render).
    if (host.ui.isPhone()) {
      close({ toPreview: true });
      document.getElementById("preview-title")?.focus();
    }
    await entered;
    renderCard();
  }
  function discardPreview() {
    closeSession();
  }
  async function discard() {
    await host.preview.exit({ rerun: true });
    discardPreview();
    S.proposal = null;
    renderCard();
    logEntry("ai-note", "Discarded.");
    el.q.focus();
  }
  async function apply() {
    const p = S.proposal;
    if (!p) return;
    const now = host.editor.get();
    const stale = now !== S.requestEditor;
    const readers = proposalReaders(p.sql);
    const g = await S.tools.gate(p.sql);
    const m = applyModel({ readers, refused: g.statements, stale });
    let choice = "run";
    if (m.needsDialog) {
      choice = await applyDialog(m);
      if (!choice) return;
    }
    // sec-review-272 H-1: "Apply, don't run" holds the page DB until the
    // viewer explicitly runs it (Run / Ctrl+Enter) — nothing implicit may
    // run the SQL about to go into the editor below.
    await host.preview.exit({ rerun: false, hold: choice === "norun" });
    discardPreview();
    const title = p.summary || "Applied dashboard";
    S.hist.push({ before: now, after: p.sql, title });
    host.editor.replace(p.sql);
    if (!host.dashName.get().trim() || host.dashName.get().trim() === "Untitled dashboard") {
      try {
        const meta = JSON.parse(host.wasm.dashboardMeta(p.sql));
        if (meta.title) host.dashName.set(meta.title);
      } catch (_) {}
    }
    if (choice === "run") host.run();
    // M-3 (sec-review-272-rereview fifth pass): remember the exact text
    // that is held — see host.hold.arm() (app.js) — so the editor's own
    // undo/redo (Ctrl+Z / Ctrl+Shift+Z) can never bring this unconfirmed
    // proposal back with no hold, however the hold was last cleared
    // (Undo, Run of other text, …).
    else host.hold.arm(p.sql);
    S.proposal = { ...p, applied: true };
    renderCard();
    host.ui.toast("Applied the assistant's dashboard.", "Undo", undo, () => document.getElementById("sql"));
    logEntry("ai-note", choice === "run" ? "Applied." : "Applied without running. Press Run ▸ when ready.");
  }
  function undo() {
    const e = S.hist.last();
    if (!e) return;
    if (host.editor.get() !== e.after) {
      host.ui.toast("The editor changed since. Use Ctrl+Z in the editor.", "OK", () => {});
      return;
    }
    host.editor.replace(e.before);
    S.hist.pop();
    host.run();
    logEntry("ai-note", "Undone.");
    renderCard();
  }
  function newConversation() {
    S.ctl?.abort();
    if (host.preview.active()) host.preview.exit({ rerun: true });
    discardPreview();
    S.conv = { id: S.conv.id + 1, log: [], skipConsent: false, history: null };
    S.toolLog = [];
    S.proposal = null;
    el.log.replaceChildren();
    renderCard();
  }
  function forgetKey() {
    host.keyStore().clear();
    setStatus(null);
    el.statusEl.textContent = "Key removed. Reload the page to lift the assistant's security policy.";
    logEntry("ai-note", "Key removed. Reload the page to lift the assistant's security policy.");
    el.notice.hidden = false;
  }

  function wirePanel() {
    el.closeBtn.onclick = () => close();
    el.newBtn.onclick = () => newConversation();
    el.forgetBtn.onclick = () => forgetKey();
    el.previewBtn.onclick = () => enterPreview();
    el.diffBtn.onclick = () => diffDialog();
    el.discardBtn.onclick = () => discard();
    el.applyBtn.onclick = () => apply();
    el.undoBtn.onclick = () => undo();
    el.cancelBtn.onclick = () => S.ctl?.abort();
    el.commonInput.onchange = () => (S.commonValues = el.commonInput.checked);
    el.chips.onclick = () => {}; // informational; no popover in M1-3
    el.form.addEventListener("submit", (e) => {
      e.preventDefault();
      send();
    });
    el.q.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        send();
      }
    });
    pane.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && !e.target.closest(".dp-dialog")) {
        e.preventDefault();
        close();
      }
    });
  }

  host.onPreviewExit(() => {
    // L-1: every exit of the preview — however it happens (Discard, Apply,
    // typing in the editor, loadDash, Explore, a new Send) — closes this
    // module's own engine session too, not only the ones that go through
    // discard()/apply() here.
    closeSession();
    if (el.previewBtn) el.previewBtn.hidden = !S.proposal;
  });

  // The preview bar's own three buttons (§5.1, static markup in
  // index.html — present and wired at mount, never shown before this).
  const previewBarDiff = doc.getElementById("preview-diff");
  const previewBarDiscard = doc.getElementById("preview-discard");
  const previewBarApply = doc.getElementById("preview-apply");
  if (previewBarDiff) previewBarDiff.onclick = () => diffDialog();
  if (previewBarDiscard) previewBarDiscard.onclick = () => discard();
  if (previewBarApply) previewBarApply.onclick = () => apply();

  if (typeof matchMedia === "function") {
    matchMedia(SIDE_DRAWER).addEventListener?.("change", () => {
      if (S.open) syncPhone(true);
    });
  }

  return { open, close, toggle, isOpen };
}
