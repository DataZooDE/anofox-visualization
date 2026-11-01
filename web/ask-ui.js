// Ask (#195, plan-ai-assistant.md §12.6 / §12.8): the viewer's panel on a
// served dashboard where the server says Ask is on (window.__served.ask).
// Loaded by app.js only then. It sends a question and the view state to
// POST /api/ask (never SQL, never a history), reads the streamed progress
// and answer, and shows:
//   - the notice before the first question of a session (the provider, and
//     whether the operator logs questions);
//   - the answer through renderAnswer: everything escaped, only **bold**,
//     *italics*, `code` and lists; no links, images, headings or HTML, and
//     URLs / e-mail addresses greyed out as inert text (M5, L5);
//   - citation chips built here from the verified cells and the page's own
//     panel titles — never from the model's text (M4); a click opens the
//     panel's tab, scrolls to it and highlights it (the only navigation, and
//     only on a click).
// No innerHTML anywhere in this file.

const MAX_PARAS = 30;

// ---- the answer: text → blocks (pure) ----

const URLISH = /(\bhttps?:\/\/[^\s)]+|\bwww\.[^\s)]+|[\w.+-]+@[\w-]+\.[\w.-]+|\b(?:javascript|data|vbscript):\S*)/gi;

// One line's inline segments: {t: "text" | "b" | "i" | "code" | "inert", s}.
export function inline(line) {
  const out = [];
  const push = (t, s) => {
    if (!s) return;
    // URLs, addresses and script schemes inside any segment stay inert text.
    if (t === "code") return out.push({ t, s });
    let last = 0;
    for (const m of s.matchAll(URLISH)) {
      if (m.index > last) out.push({ t, s: s.slice(last, m.index) });
      out.push({ t: "inert", s: m[0] });
      last = m.index + m[0].length;
    }
    if (last < s.length) out.push({ t, s: s.slice(last) });
  };
  const re = /(\*\*[^*\n]+\*\*|`[^`\n]+`|\*[^*\n]+\*)/g;
  let last = 0;
  for (const m of line.matchAll(re)) {
    push("text", line.slice(last, m.index));
    const tok = m[0];
    if (tok.startsWith("**")) push("b", tok.slice(2, -2));
    else if (tok.startsWith("`")) push("code", tok.slice(1, -1));
    else push("i", tok.slice(1, -1));
    last = m.index + tok.length;
  }
  push("text", line.slice(last));
  return out;
}

// The answer as blocks: {type: "p", parts} | {type: "ul" | "ol", items: [parts]}.
export function parseAnswer(text) {
  const blocks = [];
  const lines = String(text ?? "").replace(/\r\n?/g, "\n").split("\n");
  let para = [];
  const flush = () => {
    if (para.length) blocks.push({ type: "p", parts: inline(para.join(" ")) });
    para = [];
  };
  for (const raw of lines) {
    const line = raw.trim();
    const ul = /^[-*]\s+(.*)$/.exec(line);
    const ol = /^\d{1,3}[.)]\s+(.*)$/.exec(line);
    if (!line) {
      flush();
    } else if (ul || ol) {
      flush();
      const type = ul ? "ul" : "ol";
      const prev = blocks[blocks.length - 1];
      const item = inline((ul || ol)[1]);
      if (prev && prev.type === type) prev.items.push(item);
      else blocks.push({ type, items: [item] });
    } else {
      // headings, quotes, HTML: plain text (their markers stay as text)
      para.push(line);
    }
  }
  flush();
  return blocks.slice(0, MAX_PARAS);
}

// The blocks as DOM nodes (createElement + textContent only).
export function renderAnswer(doc, text) {
  const box = doc.createElement("div");
  box.className = "ax-ask-text";
  const fill = (el, parts) => {
    for (const p of parts) {
      const tag = { text: "span", b: "strong", i: "em", code: "code", inert: "span" }[p.t] || "span";
      const n = doc.createElement(tag);
      if (p.t === "inert") n.className = "ax-ask-inert";
      n.textContent = p.s;
      el.appendChild(n);
    }
  };
  for (const b of parseAnswer(text)) {
    if (b.type === "p") {
      const p = doc.createElement("p");
      fill(p, b.parts);
      box.appendChild(p);
    } else {
      const l = doc.createElement(b.type);
      for (const it of b.items) {
        const li = doc.createElement("li");
        fill(li, it);
        l.appendChild(li);
      }
      box.appendChild(l);
    }
  }
  return box;
}

// ---- citations, suggestions, the stream (pure) ----

const fmt = (v) => {
  if (v === null || v === undefined) return "–";
  if (typeof v === "number") return Number.isInteger(v) ? v.toLocaleString() : v.toLocaleString(undefined, { maximumFractionDigits: 2 });
  return String(v);
};

// A verified chip's text: the panel's title (from the page), the row's key
// cells and the cited value, all from the server's result — never the
// model's label.
export function chipText(c, title) {
  const parts = [title || `Panel ${c.panel}`];
  for (const [, v] of c.row || []) parts.push(fmt(v));
  if (c.column !== undefined && c.value !== undefined) parts.push(`${c.column}: ${fmt(c.value)}`);
  return parts.join(" · ");
}

// Vars a citation was read with that differ from the page's: "with region = US".
export function varsNote(cvars, current) {
  const diff = Object.entries(cvars || {}).filter(([k, v]) => JSON.stringify(current?.[k]) !== JSON.stringify(v));
  return diff.length ? "with " + diff.map(([k, v]) => `${k} = ${Array.isArray(v) ? v.join(", ") : v}`).join(", ") : "";
}

// Up to three questions from the page's own panel titles (no model call).
export function suggestions(titles) {
  const t = titles.filter((s) => s && s.length <= 80).slice(0, 2);
  const out = t.map((s) => `What does “${s}” show?`);
  if (titles.length) out.push("What stands out on this dashboard?");
  return out.slice(0, 3);
}

// Server-sent events from a text buffer: complete events and the rest.
export function parseSse(buf) {
  const events = [];
  const chunks = buf.split("\n\n");
  const rest = chunks.pop();
  for (const ch of chunks) {
    let event = "message";
    const data = [];
    for (const l of ch.split("\n")) {
      if (l.startsWith("event:")) event = l.slice(6).trim();
      else if (l.startsWith("data:")) data.push(l.slice(5).trimStart());
    }
    try {
      events.push({ event, data: JSON.parse(data.join("\n")) });
    } catch {
      /* not ours */
    }
  }
  return { events, rest };
}

// The notice (L6): the provider, the operator's own text, and — set by the
// server, not the operator — whether questions are logged.
export function noticeText(cfg) {
  let t = `Your question and the data of the panels on this dashboard that the assistant reads are sent to ${cfg.provider || "the model provider"} to answer it. Answers are AI-generated and can be wrong; check the cited panels.`;
  if (cfg.logged) t += " Your questions are logged by the operator.";
  if (cfg.notice) t += " " + cfg.notice;
  return t;
}

// ---- the panel (DOM) ----

const NOTICE_KEY = "dp-ask-notice";
const el = (doc, tag, cls, text) => {
  const n = doc.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined) n.textContent = text;
  return n;
};

// host: {cfg, dashboard, session, view(): {tab, vars}, title(i), titles(),
//        goTo(panel, citation) → element | null}
export function mountAsk(host, doc = document, win = window) {
  const cfg = host.cfg || {};
  const link = el(doc, "link");
  link.rel = "stylesheet";
  link.href = new URL("./ask-ui.css", import.meta.url).href;
  doc.head.appendChild(link);

  const btn = el(doc, "button", "ax-ask-open", "✦ Ask");
  btn.type = "button";
  btn.setAttribute("aria-controls", "ax-ask");
  btn.setAttribute("aria-expanded", "false");
  btn.title = "Ask about this dashboard (Ctrl+K)";
  const sheet = el(doc, "aside", "ax-ask");
  sheet.id = "ax-ask";
  sheet.setAttribute("role", "region");
  sheet.setAttribute("aria-label", "Ask about this dashboard");
  sheet.hidden = true;

  const head = el(doc, "div", "ax-ask-head");
  head.appendChild(el(doc, "strong", "", "Ask about this dashboard"));
  const fresh = el(doc, "button", "ax-ask-new", "New conversation");
  fresh.type = "button";
  const close = el(doc, "button", "ax-ask-close", "✕");
  close.type = "button";
  close.setAttribute("aria-label", "Close");
  head.append(fresh, close);
  const line = el(doc, "div", "ax-ask-line", `AI answers · sent to ${cfg.provider || "the model provider"}`);
  const log = el(doc, "div", "ax-ask-log");
  log.setAttribute("role", "log");
  log.setAttribute("aria-live", "polite");
  const sugg = el(doc, "div", "ax-ask-sugg");
  const status = el(doc, "div", "ax-ask-status");
  status.setAttribute("aria-live", "polite");
  const cancel = el(doc, "button", "ax-ask-cancel", "Cancel");
  cancel.type = "button";
  cancel.hidden = true;
  const form = el(doc, "form", "ax-ask-form");
  const input = el(doc, "textarea", "ax-ask-input");
  input.rows = 2;
  input.maxLength = 2000;
  input.setAttribute("aria-label", "Your question");
  input.placeholder = "Ask a question about this dashboard";
  const send = el(doc, "button", "ax-ask-send", "Ask");
  send.type = "submit";
  form.append(input, send);
  const statusRow = el(doc, "div", "ax-ask-statusrow");
  statusRow.append(status, cancel);
  sheet.append(head, line, log, sugg, statusRow, form);

  // the notice, before the first question of a session
  const notice = el(doc, "div", "ax-ask-notice");
  notice.setAttribute("role", "dialog");
  notice.setAttribute("aria-label", "Before you ask");
  notice.appendChild(el(doc, "p", "", noticeText(cfg)));
  const ok = el(doc, "button", "ax-ask-continue", "Continue");
  ok.type = "button";
  const no = el(doc, "button", "ax-ask-decline", "Not now");
  no.type = "button";
  notice.append(ok, no);
  sheet.appendChild(notice);
  doc.body.append(btn, sheet);

  let conversation = null;
  let opener = null;
  let busy = null; // AbortController of the running question
  let pointed = null; // "ask about this panel"
  const seen = () => {
    try {
      return win.sessionStorage.getItem(NOTICE_KEY) === "1";
    } catch {
      return false;
    }
  };
  const showNotice = (on) => {
    notice.hidden = !on;
    form.hidden = on;
    sugg.hidden = on;
    if (on) ok.focus();
  };
  const open = (panel) => {
    opener = doc.activeElement;
    pointed = Number.isInteger(panel) ? panel : null;
    sheet.hidden = false;
    btn.setAttribute("aria-expanded", "true");
    showNotice(!seen());
    if (seen()) input.focus();
  };
  const shut = () => {
    sheet.hidden = true;
    btn.setAttribute("aria-expanded", "false");
    (opener && opener.focus ? opener : btn).focus();
  };
  btn.addEventListener("click", () => (sheet.hidden ? open() : shut()));
  close.addEventListener("click", shut);
  ok.addEventListener("click", () => {
    try {
      win.sessionStorage.setItem(NOTICE_KEY, "1");
    } catch {
      /* private mode: shown again next time */
    }
    showNotice(false);
    input.focus();
  });
  no.addEventListener("click", shut);
  sheet.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      shut();
    }
  });
  win.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      sheet.hidden ? open() : input.focus();
    }
  });
  fresh.addEventListener("click", () => {
    conversation = null;
    log.replaceChildren();
    status.textContent = "";
  });
  cancel.addEventListener("click", () => {
    busy?.abort();
    const h = {};
    if (host.session) h["X-Anofox-Session"] = host.session;
    fetch("/api/ask", { method: "DELETE", headers: h }).catch(() => {});
  });
  for (const q of suggestions(host.titles())) {
    const b = el(doc, "button", "ax-ask-chip ax-ask-suggestion", q);
    b.type = "button";
    b.addEventListener("click", () => ask(q));
    sugg.appendChild(b);
  }

  const turn = (who, node) => {
    const t = el(doc, "div", `ax-ask-turn ax-ask-${who}`);
    t.appendChild(node);
    log.appendChild(t);
    t.scrollIntoView?.({ block: "end" });
    return t;
  };

  const showAnswer = (a) => {
    const box = el(doc, "div", "ax-ask-answer");
    box.appendChild(renderAnswer(doc, a.text));
    if (a.scope === "not_on_dashboard") box.appendChild(el(doc, "p", "ax-ask-scope", "This isn't on the dashboard."));
    if (a.scope === "partly") box.appendChild(el(doc, "p", "ax-ask-scope", "The dashboard shows only part of this."));
    const chips = el(doc, "div", "ax-ask-chips");
    for (const c of a.citations || []) {
      if (!c.verified) {
        const s = el(doc, "span", "ax-ask-unverified", `not verified: ${String(c.label || "").slice(0, 60)}`);
        chips.appendChild(s);
        continue;
      }
      const text = chipText(c, host.title(c.panel));
      const b = el(doc, "button", "ax-ask-chip", text);
      b.type = "button";
      b.setAttribute("aria-label", `Go to panel ${text}`);
      const note = varsNote(c.vars, host.view().vars);
      if (note) b.appendChild(el(doc, "span", "ax-ask-varsnote", ` (${note})`));
      b.addEventListener("click", () => {
        const target = host.goTo(c.panel, c);
        if (!target) return;
        const back = el(doc, "button", "ax-ask-back", "Back to the answer");
        back.type = "button";
        back.addEventListener("click", () => {
          back.remove();
          b.focus();
        });
        target.after?.(back);
      });
      chips.appendChild(b);
    }
    if (chips.childNodes.length) box.appendChild(chips);
    box.appendChild(el(doc, "p", "ax-ask-label", "AI-generated · check the cited panels"));
    if (a.refreshed) box.appendChild(el(doc, "p", "ax-ask-scope", "The data was refreshed: the conversation started over."));
    turn("bot", box);
  };

  async function ask(question) {
    question = String(question || "").trim();
    if (!question || busy) return;
    if (!seen()) return showNotice(true);
    turn("me", el(doc, "p", "", question));
    input.value = "";
    const ctl = new AbortController();
    busy = ctl;
    cancel.hidden = false;
    send.disabled = true;
    status.textContent = "Thinking…";
    const { tab, vars } = host.view();
    const body = { dashboard: host.dashboard, question, vars, conversation, tab: tab ?? null };
    if (pointed !== null) body.panel = pointed;
    const headers = { "Content-Type": "application/json" };
    if (host.session) headers["X-Anofox-Session"] = host.session;
    try {
      const r = await fetch("/api/ask", { method: "POST", headers, body: JSON.stringify(body), signal: ctl.signal });
      if (!r.ok || !(r.headers.get("content-type") || "").startsWith("text/event-stream")) {
        let msg = "The assistant couldn't answer right now.";
        try {
          msg = (await r.json()).error || msg;
        } catch {
          /* keep the generic message */
        }
        turn("bot", el(doc, "p", "ax-ask-error", String(msg).slice(0, 300)));
        return;
      }
      const reader = r.body.getReader();
      const dec = new TextDecoder();
      let buf = "";
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buf += dec.decode(value, { stream: true });
        const { events, rest } = parseSse(buf);
        buf = rest;
        for (const { event, data } of events) {
          if (event === "progress") {
            status.textContent = data.phase === "reading" ? `Reading “${host.title(data.panel) || "a panel"}”…` : "Thinking…";
          } else if (event === "answer") {
            conversation = data.conversation;
            showAnswer(data);
          } else if (event === "error") {
            turn("bot", el(doc, "p", "ax-ask-error", String(data.error || "The assistant couldn't answer.").slice(0, 300)));
          }
        }
      }
    } catch (e) {
      if (e?.name !== "AbortError") turn("bot", el(doc, "p", "ax-ask-error", "The assistant couldn't answer right now."));
      else turn("bot", el(doc, "p", "ax-ask-error", "Cancelled."));
    } finally {
      busy = null;
      cancel.hidden = true;
      send.disabled = false;
      status.textContent = "";
      pointed = null;
    }
  }
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    ask(input.value);
  });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      ask(input.value);
    }
  });
  return { open, ask, shut };
}
