// #1253: a Copy button on every code block of a ::MARKDOWN box (`.md-body
// pre`): prompts to paste into an assistant, setup commands, SQL to try.
// The button copies the block's text and says "Copied" for a moment; without
// the clipboard API (a file:// page, plain http) it selects the text instead,
// so Ctrl/⌘+C copies it.
//
// Self-contained (no imports, no app.js globals), like web/nocase.js.
export function addCopyButtons(root) {
  const doc = root.ownerDocument || root;
  for (const pre of root.querySelectorAll(".md-body pre")) {
    if (pre.querySelector(":scope > .md-copy")) continue;
    const b = doc.createElement("button");
    b.type = "button";
    b.className = "md-copy";
    b.textContent = "Copy";
    b.setAttribute("aria-label", "Copy code");
    pre.append(b);
  }
}

// One delegated click handler per document: copy the code next to the button.
export function wireCopyButtons(doc) {
  if (doc.__mdCopy) return;
  doc.__mdCopy = true;
  doc.addEventListener("click", (e) => {
    const b = e.target.closest && e.target.closest(".md-copy");
    if (!b) return;
    const code = b.parentElement.querySelector("code") || b.parentElement;
    const text = [...code.childNodes]
      .filter((n) => !(n.classList && n.classList.contains("md-copy")))
      .map((n) => n.textContent)
      .join("")
      .replace(/\n$/, "");
    const said = (label) => {
      b.textContent = label;
      clearTimeout(b.__t);
      b.__t = setTimeout(() => (b.textContent = "Copy"), 1500);
    };
    const select = () => {
      const r = doc.createRange();
      r.selectNodeContents(code);
      const s = doc.getSelection();
      s.removeAllRanges();
      s.addRange(r);
      said("Selected");
    };
    const clip = doc.defaultView && doc.defaultView.navigator.clipboard;
    if (clip && doc.defaultView.isSecureContext) clip.writeText(text).then(() => said("Copied"), select);
    else select();
  });
}
