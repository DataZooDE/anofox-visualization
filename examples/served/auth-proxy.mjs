// A tiny login proxy for the served example — a stand-in for oauth2-proxy,
// Caddy forward_auth or Cloudflare Access. Node 18+, no dependencies.
//
//   UPSTREAM_PORT=8910 PROXY_PORT=8911 ANOFOX_PROXY_SECRET=… node auth-proxy.mjs
//
// It does what a real auth proxy must do in front of anofox_serve_dashboards
// with options.auth (PR #78):
//   * it logs the viewer in (here: pick a demo user — no passwords; a real
//     proxy asks an identity provider) and keeps a signed session cookie;
//   * on every request it REMOVES the identity headers a client may have sent
//     (X-Forwarded-User, X-Forwarded-Groups, the secret header) and sets its
//     own, plus the shared secret, so the server can tell the proxy from a
//     client that talks to it directly.
// Both listen on 127.0.0.1 only. The secret comes from the environment
// (serve.sh generates a fresh one per start); the cookie key is random per
// process, so a restart logs everyone out.
import http from "node:http";
import crypto from "node:crypto";

const UPSTREAM = Number(process.env.UPSTREAM_PORT || 8910);
const PORT = Number(process.env.PROXY_PORT || UPSTREAM + 1);
const SECRET = process.env.ANOFOX_PROXY_SECRET || "";
if (SECRET.length < 32) {
  console.error("auth-proxy: set ANOFOX_PROXY_SECRET (at least 32 characters)");
  process.exit(1);
}
const COOKIE = "demo_session";
const KEY = crypto.randomBytes(32);

// The demo directory: who exists and which groups they are in.
const USERS = {
  alice: { name: "Alice", groups: ["managers"], note: "manages the North" },
  bob: { name: "Bob", groups: ["managers"], note: "manages the Midlands and the South" },
  carol: { name: "Carol", groups: ["admins"], note: "head office, sees every store" },
};
// Identity headers the server reads (its auth defaults); never passed through.
const IDENTITY = ["x-forwarded-user", "x-forwarded-groups", "x-anofox-proxy-secret"];
const HOP = ["connection", "keep-alive", "proxy-connection", "transfer-encoding", "upgrade", "te", "trailer"];

const sign = (v) => crypto.createHmac("sha256", KEY).update(v).digest("base64url");
function sessionUser(req) {
  const m = (req.headers.cookie || "").match(new RegExp(`(?:^|;\\s*)${COOKIE}=([^;]+)`));
  if (!m) return null;
  const [user, mac] = m[1].split(".");
  if (!user || !mac || !USERS[user]) return null;
  const want = Buffer.from(sign(user));
  const got = Buffer.from(mac);
  return want.length === got.length && crypto.timingSafeEqual(want, got) ? user : null;
}

const esc = (s) => String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
function loginPage(res, current) {
  const buttons = Object.entries(USERS)
    .map(([id, u]) => `<button name="user" value="${id}">${esc(u.name)}<small>${esc(u.note)}</small></button>`)
    .join("");
  res.writeHead(200, {
    "Content-Type": "text/html; charset=utf-8",
    "Cache-Control": "no-store",
    "X-Frame-Options": "DENY",
    "Content-Security-Policy": "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'",
  });
  res.end(`<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>Sign in</title>
<style>
  body { font: 15px system-ui, sans-serif; background: #f3f4f6; color: #1f2937; margin: 0; padding: 48px 16px; }
  main { max-width: 420px; margin: auto; background: #fff; border: 1px solid #e5e7eb; border-radius: 10px; padding: 28px; }
  h1 { font-size: 20px; margin: 0 0 6px; } p { color: #6b7280; margin: 0 0 20px; }
  button { display: block; width: 100%; text-align: left; font: inherit; font-weight: 600; padding: 12px 14px;
           margin: 0 0 10px; border: 1px solid #d1d5db; border-radius: 8px; background: #fff; cursor: pointer; }
  button:hover { border-color: #2563eb; } small { display: block; font-weight: 400; color: #6b7280; }
  @media (prefers-color-scheme: dark) {
    body { background: #111827; color: #e5e7eb; } main { background: #1f2937; border-color: #374151; }
    button { background: #1f2937; color: #e5e7eb; border-color: #4b5563; } p, small { color: #9ca3af; }
  }
</style>
<main><h1>Sign in to the bakery dashboards</h1>
<p>${current ? `Signed in as ${esc(USERS[current].name)}. ` : ""}Demo login: pick a person.</p>
<form method="post" action="/_login">${buttons}</form>
${current ? `<form method="post" action="/_logout"><button>Sign out</button></form>` : ""}</main>`);
}

function readForm(req) {
  return new Promise((resolve) => {
    let body = "";
    req.on("data", (c) => {
      body += c;
      if (body.length > 1024) req.destroy();
    });
    req.on("end", () => resolve(new URLSearchParams(body)));
  });
}

// A login form may only be posted from this proxy's own pages.
const sameOrigin = (req) => !req.headers.origin || req.headers.origin === `http://${req.headers.host}`;

const server = http.createServer(async (req, res) => {
  const path = (req.url || "/").split("?")[0];
  if (path === "/_login" || path === "/_logout") {
    if (req.method !== "POST" || !sameOrigin(req)) {
      if (req.method === "GET") return loginPage(res, sessionUser(req));
      res.writeHead(403).end("forbidden");
      return;
    }
    const form = path === "/_login" ? await readForm(req) : new URLSearchParams();
    const user = form.get("user");
    const cookie =
      path === "/_login" && USERS[user]
        ? `${COOKIE}=${user}.${sign(user)}; Path=/; HttpOnly; SameSite=Strict`
        : `${COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0`;
    res.writeHead(303, { "Set-Cookie": cookie, Location: path === "/_login" && USERS[user] ? "/" : "/_login" });
    res.end();
    return;
  }

  const user = sessionUser(req);
  if (!user) {
    if (req.method === "GET" && /text\/html/.test(req.headers.accept || "")) {
      res.writeHead(303, { Location: "/_login" }).end();
    } else {
      res.writeHead(401, { "Content-Type": "text/plain" }).end("sign in at /_login");
    }
    return;
  }

  // Forward: drop hop-by-hop and identity headers and our own cookie, then
  // set the identity this proxy vouches for.
  const headers = {};
  for (const [k, v] of Object.entries(req.headers)) {
    const lk = k.toLowerCase();
    if (HOP.includes(lk) || IDENTITY.includes(lk)) continue;
    headers[k] = v;
  }
  const rest = (req.headers.cookie || "")
    .split(/;\s*/)
    .filter((c) => c && !c.startsWith(`${COOKIE}=`))
    .join("; ");
  if (rest) headers.cookie = rest;
  else delete headers.cookie;
  headers["x-forwarded-user"] = user;
  headers["x-forwarded-groups"] = USERS[user].groups.join(",");
  headers["x-anofox-proxy-secret"] = SECRET;

  const up = http.request(
    { host: "127.0.0.1", port: UPSTREAM, method: req.method, path: req.url, headers },
    (r) => {
      const out = { ...r.headers };
      for (const h of HOP) delete out[h];
      res.writeHead(r.statusCode || 502, out);
      r.pipe(res);
    },
  );
  up.on("error", () => {
    if (!res.headersSent) res.writeHead(502, { "Content-Type": "text/plain" });
    res.end("dashboard server unavailable");
  });
  req.pipe(up);
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`auth-proxy: http://127.0.0.1:${PORT}/ → 127.0.0.1:${UPSTREAM}`);
});
for (const s of ["SIGINT", "SIGTERM"]) process.on(s, () => process.exit(0));
