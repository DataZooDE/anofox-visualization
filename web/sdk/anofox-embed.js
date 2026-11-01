// <anofox-dashboard>: a locked anofox-visualization dashboard inside another
// web page (#771, docs/secure-serving.md "Embedding with <anofox-dashboard>").
//
//   <script type="module" src="https://dash.example.com/sdk/anofox-embed.js"></script>
//   <anofox-dashboard src="https://dash.example.com/d/sales" token-callback="getToken"></anofox-dashboard>
//   <script>
//     // Your backend calls POST /api/auth/token with its API key and returns the token.
//     async function getToken(dashboard) {
//       return (await (await fetch("/my-backend/anofox-token?d=" + dashboard)).json()).token;
//     }
//   </script>
//
// The dashboard runs in an iframe on its own origin: its CSP stays in force
// and its data never enters the host page's scripts. The element sizes the
// frame to the dashboard's height, asks for a new token when the page's
// session ends, and passes events both ways over postMessage, each checked
// against the dashboard's origin and frame:
//   events (on the element): "anofox-state" {vars} after every run,
//     "anofox-link" {href} when a link leaves the dashboard, "anofox-ready";
//   el.setVars({region: "EU", channel: ["app", "web"]}) sets variables (a
//     variable the token locks keeps its locked value — the server decides).

const MSG = "anofox";

class AnofoxDashboard extends HTMLElement {
  static get observedAttributes() {
    return ["src", "token-callback"];
  }

  constructor() {
    super();
    this._frame = null;
    this._origin = null;
    this._loading = null;
    this._ready = false;
    this._vars = null;
    this._onMessage = (e) => this._message(e);
  }

  connectedCallback() {
    if (!this._frame) {
      const root = this.attachShadow({ mode: "open" });
      const style = document.createElement("style");
      style.textContent = ":host{display:block}iframe{display:block;width:100%;height:600px;border:0}";
      this._frame = document.createElement("iframe");
      this._frame.setAttribute("title", this.getAttribute("title") || "Dashboard");
      this._frame.setAttribute("referrerpolicy", "origin");
      root.append(style, this._frame);
    }
    window.addEventListener("message", this._onMessage);
    this._load();
  }

  disconnectedCallback() {
    window.removeEventListener("message", this._onMessage);
  }

  attributeChangedCallback(_name, old, value) {
    if (this._frame && old !== value) this._load();
  }

  // The dashboard URL, its origin and its id (`…/d/<id>`).
  _target() {
    const src = this.getAttribute("src");
    if (!src) return null;
    const u = new URL(src, location.href);
    const m = /\/d\/([^/?#]+)\/?$/.exec(u.pathname);
    if (!/^https?:$/.test(u.protocol) || !m) {
      console.error("<anofox-dashboard>: src must be a dashboard URL like https://dash.example.com/d/sales");
      return null;
    }
    return { url: u, id: decodeURIComponent(m[1]) };
  }

  async _token(id) {
    const name = this.getAttribute("token-callback");
    const fn = typeof this.getToken === "function" ? this.getToken : name && window[name];
    if (typeof fn !== "function") throw new Error("<anofox-dashboard>: no token-callback (a global function) or el.getToken");
    const t = await fn(id);
    if (typeof t !== "string" || !t) throw new Error("<anofox-dashboard>: the token callback returned no token");
    return t;
  }

  // Load (or reload, with a fresh token) the dashboard into the frame.
  _load() {
    const t = this._target();
    if (!t) return;
    const run = (this._loading = (async () => {
      const token = await this._token(t.id);
      if (run !== this._loading) return; // a newer load started
      const u = new URL(t.url);
      u.searchParams.set("embed", token);
      this._origin = u.origin;
      this._ready = false;
      this._frame.src = u.href;
    })().catch((e) => {
      console.error(e);
      this.dispatchEvent(new CustomEvent("anofox-error", { detail: { message: String(e.message || e) } }));
    }));
  }

  _message(e) {
    if (!this._frame || e.source !== this._frame.contentWindow || e.origin !== this._origin) return;
    const d = e.data;
    if (!d || d.source !== MSG || typeof d.type !== "string") return;
    switch (d.type) {
      case "height":
        if (Number.isFinite(d.height) && d.height > 0) this._frame.style.height = Math.min(d.height, 100000) + "px";
        break;
      case "expired":
        this._load();
        break;
      case "ready":
        this._ready = true;
        this._sendVars();
        this.dispatchEvent(new CustomEvent("anofox-ready", { detail: d.detail || {} }));
        break;
      case "state":
      case "link":
        this.dispatchEvent(new CustomEvent("anofox-" + d.type, { detail: d.detail || {} }));
        break;
    }
  }

  /** Set dashboard variables: {name: value | [values]} (kept, and sent
   * again after a reload with a fresh token). */
  setVars(vars) {
    this._vars = vars;
    this._sendVars();
  }

  _sendVars() {
    if (!this._ready || !this._vars || !this._frame?.contentWindow || !this._origin) return;
    this._frame.contentWindow.postMessage({ source: MSG, type: "set-vars", vars: this._vars }, this._origin);
  }
}

if (!customElements.get("anofox-dashboard")) customElements.define("anofox-dashboard", AnofoxDashboard);

export { AnofoxDashboard };
