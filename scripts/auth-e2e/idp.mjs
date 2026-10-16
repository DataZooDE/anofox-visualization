// A local OIDC/JWKS test IdP for scripts/auth-e2e (no external services):
// HTTPS with a throwaway test CA (run.sh makes it).
//   ISSUER=https://127.0.0.1:8953 IDP_KEY=… IDP_CERT=… OIDC_SECRET=… node idp.mjs
// /.well-known/openid-configuration, /jwks, /authorize (a user picker),
// /token (code + PKCE + client secret), /mint (test helper: a bearer JWT).
import https from "node:https";
import crypto from "node:crypto";
import fs from "node:fs";

const PORT = Number(process.env.IDP_PORT || 8953);
const ISS = process.env.ISSUER || `https://localhost:${PORT}`;
const CLIENT = "dash";
const CLIENT_SECRET = process.env.OIDC_SECRET;
const { privateKey, publicKey } = crypto.generateKeyPairSync("ec", { namedCurve: "P-256" });
const jwk = { ...publicKey.export({ format: "jwk" }), kid: "k1", use: "sig", alg: "ES256" };
const USERS = {
  alice: { email: "alice@example.com", groups: ["sales"] },
  bob: { email: "bob@example.com", groups: ["sales"] },
  admin: { email: "root@example.com", groups: ["admins"] },
};
const codes = new Map();
const b64u = (b) => Buffer.from(b).toString("base64url");
export function sign(payload, header = { alg: "ES256", kid: "k1", typ: "JWT" }) {
  const h = b64u(JSON.stringify(header)), p = b64u(JSON.stringify(payload));
  const sig = crypto.sign("sha256", Buffer.from(`${h}.${p}`), { key: privateKey, dsaEncoding: "ieee-p1363" });
  return `${h}.${p}.${b64u(sig)}`;
}
const json = (res, code, v) => { res.writeHead(code, { "Content-Type": "application/json" }); res.end(JSON.stringify(v)); };
const log = [];
https.createServer({ key: fs.readFileSync(process.env.IDP_KEY), cert: fs.readFileSync(process.env.IDP_CERT) }, (req, res) => {
  const u = new URL(req.url, ISS);
  log.push(`${req.method} ${u.pathname}`);
  if (u.pathname === "/.well-known/openid-configuration")
    return json(res, 200, { issuer: ISS, jwks_uri: `${ISS}/jwks`, authorization_endpoint: `${ISS}/authorize`, token_endpoint: `${ISS}/token` });
  if (u.pathname === "/jwks") return json(res, 200, { keys: [jwk] });
  if (u.pathname === "/log") return json(res, 200, log);
  if (u.pathname === "/authorize") {
    const q = Object.fromEntries(u.searchParams);
    if (q.client_id !== CLIENT || q.response_type !== "code" || q.code_challenge_method !== "S256" || !q.code_challenge || !q.nonce || !q.state)
      return json(res, 400, { error: "invalid_request", q });
    const user = q.login_hint || u.searchParams.get("user");
    if (!user) {
      res.writeHead(200, { "Content-Type": "text/html" });
      return res.end(`<h1>Test IdP</h1>` + Object.keys(USERS).map((n) => `<a id="u-${n}" href="${u.pathname}${u.search}&user=${n}">${n}</a>`).join(" "));
    }
    const code = crypto.randomBytes(16).toString("hex");
    codes.set(code, { user, nonce: q.nonce, challenge: q.code_challenge, redirect: q.redirect_uri });
    const back = new URL(q.redirect_uri);
    back.searchParams.set("code", code); back.searchParams.set("state", q.state);
    res.writeHead(302, { Location: back.toString() }); return res.end();
  }
  if (u.pathname === "/token" && req.method === "POST") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      const f = Object.fromEntries(new URLSearchParams(body));
      const basic = Buffer.from((req.headers.authorization || "").replace(/^Basic /, ""), "base64").toString();
      if (basic !== `${CLIENT}:${CLIENT_SECRET}`) return json(res, 401, { error: "invalid_client" });
      const c = codes.get(f.code); codes.delete(f.code); // one use
      if (!c || f.grant_type !== "authorization_code" || f.redirect_uri !== c.redirect) return json(res, 400, { error: "invalid_grant" });
      if (crypto.createHash("sha256").update(f.code_verifier || "").digest("base64url") !== c.challenge) return json(res, 400, { error: "invalid_grant", why: "pkce" });
      const now = Math.floor(Date.now() / 1000), who = USERS[c.user];
      const id_token = sign({ iss: ISS, aud: CLIENT, sub: c.user, email: who.email, groups: who.groups, nonce: c.nonce, iat: now, exp: now + 300 });
      json(res, 200, { id_token, access_token: "opaque", token_type: "Bearer", expires_in: 300 });
    });
    return;
  }
  if (u.pathname === "/mint") {
    // A bearer token for the API tests: ?user=&aud=&exp_in=
    const now = Math.floor(Date.now() / 1000), who = USERS[u.searchParams.get("user") || "alice"];
    const p = { iss: ISS, aud: u.searchParams.get("aud") || "dash-api", sub: who.email, groups: who.groups, iat: now, exp: now + Number(u.searchParams.get("exp_in") || 300) };
    return json(res, 200, { token: sign(p), jwk });
  }
  json(res, 404, { error: "not found" });
}).listen(PORT, "127.0.0.1", () => console.log("test IdP on", ISS));
