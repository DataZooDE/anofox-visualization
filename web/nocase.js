// #1240: small-caps labels (KPI captions, form section titles, table group
// headers, …) are uppercased with CSS `text-transform: uppercase`, and the
// browser maps Greek letters to their capitals, which read as Latin ones
// (μ → M, τ → T, ρ → P). `keepMathCase` wraps the Greek letters inside such
// labels in `<span class="nocase">` (`text-transform: none`, theme-base.css),
// so "Average cycle μ" reads AVERAGE CYCLE μ.
//
// Self-contained (no imports, no app.js globals): the app imports it, and the
// static and served pages (src/page.rs) embed its source and run it once.
// Not covered: `.md-tone`'s label, which CSS draws from an attribute.
export function keepMathCase(root) {
  const SEL =
    ".lp-group-h,.dp-brand,.dp-form-sec-title,.md-details>summary,.tot-agg,.th-g,.tot-l," +
    ".metric-cap,.served-nav-group,.served-nav-menu h3,.tip-kicker,.dash-group-h";
  const GREEK = /[Ͱ-Ͽἀ-῿]+/g;
  const doc = root.ownerDocument || root;
  const els = [...root.querySelectorAll(SEL)];
  if (root.matches && root.matches(SEL)) els.push(root);
  for (const el of els) {
    if (!/[Ͱ-Ͽἀ-῿]/.test(el.textContent || "")) continue;
    const walk = doc.createTreeWalker(el, 4 /* NodeFilter.SHOW_TEXT */);
    const texts = [];
    for (let n = walk.nextNode(); n; n = walk.nextNode()) {
      if (!n.parentElement.closest(".nocase")) texts.push(n);
    }
    for (const t of texts) {
      const s = t.nodeValue;
      GREEK.lastIndex = 0;
      if (!GREEK.test(s)) continue;
      const frag = doc.createDocumentFragment();
      let at = 0;
      GREEK.lastIndex = 0;
      for (let m = GREEK.exec(s); m; m = GREEK.exec(s)) {
        if (m.index > at) frag.append(s.slice(at, m.index));
        const span = doc.createElement("span");
        span.className = "nocase";
        span.textContent = m[0];
        frag.append(span);
        at = m.index + m[0].length;
      }
      if (at < s.length) frag.append(s.slice(at));
      t.replaceWith(frag);
    }
  }
}
