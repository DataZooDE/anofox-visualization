//! The dispatcher: which credential counts, scopes, the login routes and the
//! audit log, across methods.

use super::jwt::now_s;
use super::jwt::testkit::*;
use super::*;
use serde_json::json;

const PROXY_SECRET: &str = "0123456789abcdef0123456789abcdef";
const EMBED_SECRET: &str = "embed-secret-0123456789abcdef0123456789";
/// The OIDC authorization code used in tests. Deliberately not a valid hex
/// or base64 substring (contains `-` outside base64url's position rules and
/// letters past `f`), so it can't appear by chance inside the random hex
/// IDs or base64 tokens that also end up in the audit log.
const TEST_AUTH_CODE: &str = "not-a-real-code-ZQ9";

fn tmpdir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "anofox-auth-{}",
        crate::host::serving::random_token()
    ));
    std::fs::create_dir(&d).unwrap();
    d
}

/// Every method at once, against a fake IdP; the audit log in `dir`.
fn all_methods(f: Arc<FakeIdp>, dir: &std::path::Path) -> (Auth, String) {
    let (key, id, hash) = apikey::new_key();
    let e = apikey::KeyEntry {
        id,
        sha256: hash,
        claims: Claims {
            user: "svc-etl".into(),
            groups: vec![],
            attrs: vec![],
        },
        dashboards: Some(vec!["sales".into()]),
        scopes: apikey::KeyScopes::parse(&["preview"]).unwrap(),
        expires: None,
        revoked: false,
    };
    let keys = dir.join("keys.json");
    apikey::write_file(&keys, &[apikey::entry_json(&e, None, None)]).unwrap();
    let cfg = json!([
        {"mode": "proxy", "shared_secret": PROXY_SECRET},
        {"mode": "jwt", "issuer": ISS, "audience": "dash-api", "groups_claim": "groups",
         "claims": {"tenant": "tid"}},
        {"mode": "oidc", "issuer": ISS, "client_id": "dash",
         "redirect_url": "https://dash.test/auth/callback"},
        {"mode": "embed", "secret": EMBED_SECRET, "claims": ["tenant"]},
        {"mode": "api_keys", "file": keys},
    ]);
    let fetch = move |_: Option<&std::path::Path>| -> Result<Arc<dyn http::Fetch>, String> {
        Ok(f.clone())
    };
    let mut a = Auth::from_json_with(&cfg, &|_| None, &fetch).unwrap();
    let audit = dir.join("audit.jsonl");
    a.set_audit(Arc::new(AuditLog::new(audit.to_str().unwrap()).unwrap()));
    (a, key)
}

fn req<'a>(method: &'a str, url: &'a str, headers: &'a [(&'a str, &'a str)]) -> Req<'a> {
    Req {
        method,
        url,
        peer: Some("127.0.0.1".parse().unwrap()),
        headers,
    }
}

fn jwt_payload(extra: serde_json::Value) -> serde_json::Value {
    let now = now_s();
    let mut p = json!({"iss": ISS, "aud": "dash-api", "sub": "alice", "groups": ["eu"],
        "tid": "acme", "exp": now + 300, "iat": now});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    p
}

fn embed_link(p: serde_json::Value) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &p,
        &jsonwebtoken::EncodingKey::from_secret(EMBED_SECRET.as_bytes()),
    )
    .unwrap()
}

#[test]
fn proxy_only_keeps_phase_one_behaviour() {
    let a = Auth::from_json(
        &json!({"mode": "proxy", "shared_secret": PROXY_SECRET}),
        |_| None,
    )
    .unwrap();
    assert_eq!(a.methods(), ["proxy"]);
    // no secret → bad_secret, even with other credentials around
    let h = [
        ("Authorization", "Bearer x.y.z"),
        ("X-Forwarded-User", "admin"),
    ];
    let d = a.authenticate(&req("GET", "/", &h)).unwrap_err();
    assert_eq!((d.reason, d.method), ("bad_secret", Some("proxy")));
    let h = [
        ("X-Anofox-Proxy-Secret", PROXY_SECRET),
        ("X-Forwarded-User", "alice"),
    ];
    let id = a.authenticate(&req("GET", "/", &h)).unwrap();
    assert_eq!((id.claims.user.as_str(), id.method), ("alice", "proxy"));
    assert_eq!(id.scope, Scope::default());
    // a P1 configuration still parses as before (object form)
    assert!(Auth::from_json(&json!({"mode": "nope"}), |_| None).is_err());
    assert!(Auth::from_json(&json!([]), |_| None).is_err());
    assert!(Auth::from_json(
        &json!([{"mode": "proxy", "shared_secret": PROXY_SECRET}, {"mode": "proxy", "shared_secret": PROXY_SECRET}]),
        |_| None
    )
    .is_err());
}

#[test]
fn each_method_yields_the_same_claims_shape() {
    let dir = tmpdir();
    let k = es256("k1");
    let (a, key) = all_methods(idp(&[&k]), &dir);
    // JWT bearer
    let t = sign(&k, Some("k1"), &jwt_payload(json!({})));
    let auth = format!("Bearer {t}");
    let id = a
        .authenticate(&req("POST", "/api/panel", &[("Authorization", &auth)]))
        .unwrap();
    assert_eq!(id.method, "jwt");
    assert!(
        id.key_scopes.is_empty(),
        "only an API key carries scopes (#800)"
    );
    assert_eq!(
        id.claims.variables(),
        vec![
            ("__claim_user".to_string(), "'alice'".to_string()),
            ("__claim_groups".into(), "['eu']".into()),
            ("__claim_tenant".into(), "'acme'".into()),
        ]
    );
    // API key
    let auth = format!("Bearer {key}");
    let id = a
        .authenticate(&req("POST", "/api/panel", &[("authorization", &auth)]))
        .unwrap();
    assert_eq!((id.method, id.claims.user.as_str()), ("api_key", "svc-etl"));
    assert!(id.scope.allows("sales") && !id.scope.allows("payroll"));
    assert!(id.key_scopes.preview && !id.key_scopes.admin);
    // proxy (secret present wins over everything else)
    let h = [
        ("X-Anofox-Proxy-Secret", PROXY_SECRET),
        ("X-Forwarded-User", "bob"),
        ("Authorization", auth.as_str()),
    ];
    let id = a.authenticate(&req("GET", "/", &h)).unwrap();
    assert_eq!((id.method, id.claims.user.as_str()), ("proxy", "bob"));
    assert!(
        id.key_scopes.is_empty(),
        "the key's scopes never ride on another method"
    );
    // nothing → no_credentials (and a login redirect for a page view only)
    let d = a.authenticate(&req("GET", "/d/sales", &[])).unwrap_err();
    assert_eq!(d.reason, "no_credentials");
    let r = a
        .login_redirect(&req("GET", "/d/sales?v.x=1", &[]), &d)
        .unwrap();
    assert_eq!(r.status, 302);
    assert_eq!(
        r.header_value("Location"),
        Some("/auth/login?next=%2Fd%2Fsales%3Fv.x%3D1")
    );
    assert!(a
        .login_redirect(&req("POST", "/api/panel", &[]), &d)
        .is_none());
    // a bad embed link or key on a page is a 401, not a trip to the IdP
    let expired_link = Denied {
        reason: "expired",
        method: Some("embed"),
        audited: false,
    };
    assert!(a
        .login_redirect(&req("GET", "/d/sales", &[]), &expired_link)
        .is_none());
    // a broken revocation file: 503 + Retry-After, never the login loop
    let down = Denied {
        reason: "revocations_unavailable",
        method: Some("oidc"),
        audited: false,
    };
    assert!(a
        .login_redirect(&req("GET", "/d/sales", &[]), &down)
        .is_none());
    let r = down.unavailable().unwrap();
    assert_eq!((r.status, r.header_value("Retry-After")), (503, Some("10")));
    assert!(expired_link.unavailable().is_none());
    let ended = Denied {
        reason: "expired",
        method: Some("oidc"),
        audited: false,
    };
    assert!(a
        .login_redirect(&req("GET", "/d/sales", &[]), &ended)
        .is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn successful_uses_are_audited_once_per_window() {
    let dir = tmpdir();
    let k = es256("k1");
    let (a, key) = all_methods(idp(&[&k]), &dir);
    let bearer = format!("Bearer {key}");
    let t = format!("Bearer {}", sign(&k, Some("k1"), &jwt_payload(json!({}))));
    for _ in 0..5 {
        for h in [&bearer, &t] {
            assert!(a
                .authenticate(&req("POST", "/api/panel", &[("Authorization", h)]))
                .is_ok());
        }
    }
    let log = std::fs::read_to_string(dir.join("audit.jsonl")).unwrap();
    let ok: Vec<serde_json::Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .filter(|l: &serde_json::Value| l["audit"] == "auth" && l["outcome"] == "ok")
        .collect();
    assert_eq!(ok.len(), 2, "{log}");
    let k = ok.iter().find(|l| l["method"] == "api_key").unwrap();
    assert_eq!(
        (k["user"].as_str(), k["key_id"].as_str()),
        (Some("svc-etl"), Some(&key[4..16]))
    );
    assert!(ok
        .iter()
        .any(|l| l["method"] == "jwt" && l["user"] == "alice"));
    assert!(!log.contains(&key[17..]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn id_tokens_are_not_bearer_tokens() {
    let dir = tmpdir();
    let k = es256("k1");
    let (a, _) = all_methods(idp(&[&k]), &dir);
    for extra in [
        json!({"nonce": "n"}),
        json!({"at_hash": "x"}),
        json!({"token_use": "id"}),
        json!({"typ": "ID"}),
    ] {
        let t = sign(&k, Some("k1"), &jwt_payload(extra.clone()));
        let h = format!("Bearer {t}");
        let d = a
            .authenticate(&req("GET", "/api/panel", &[("Authorization", &h)]))
            .unwrap_err();
        assert_eq!(d.reason, "id_token", "{extra}");
    }
    // an RFC 9068 access token (header typ at+jwt) is no ID token
    assert!(!is_id_token(Some("at+jwt"), &json!({"nonce": "n"})));
    assert!(is_id_token(Some("JWT"), &json!({"typ": "ID"})));
    // a jwt audience equal to the oidc client_id fails the start
    let f = idp(&[]);
    let fetch = move |_: Option<&std::path::Path>| -> Result<Arc<dyn http::Fetch>, String> {
        Ok(f.clone())
    };
    let cfg = |accept: bool| {
        json!([
            {"mode": "oidc", "issuer": ISS, "client_id": "dash",
             "redirect_url": "https://dash.test/auth/callback"},
            {"mode": "jwt", "issuer": ISS, "audience": "dash", "accept_id_tokens": accept},
        ])
    };
    assert!(Auth::from_json_with(&cfg(false), &|_| None, &fetch).is_err());
    assert!(Auth::from_json_with(&cfg(true), &|_| None, &fetch).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_invalid_credential_never_falls_back() {
    let dir = tmpdir();
    let k = es256("k1");
    let (a, key) = all_methods(idp(&[&k]), &dir);
    let expired = sign(&k, Some("k1"), &jwt_payload(json!({"exp": now_s() - 3600})));
    let auth = format!("Bearer {expired}");
    let good_key = format!("Bearer {key}");
    // an expired JWT next to a valid API key header: refused
    let h = [
        ("Authorization", auth.as_str()),
        ("Authorization", good_key.as_str()),
    ];
    assert_eq!(
        a.authenticate(&req("GET", "/", &h)).unwrap_err().reason,
        "malformed_token"
    );
    let h = [("Authorization", auth.as_str())];
    assert_eq!(
        a.authenticate(&req("GET", "/", &h)).unwrap_err().reason,
        "expired"
    );
    // a wrong scheme
    let h = [("Authorization", "Basic YWRtaW46YWRtaW4=")];
    assert_eq!(
        a.authenticate(&req("GET", "/", &h)).unwrap_err().reason,
        "malformed_token"
    );
    // a revoked / unknown key
    let (other, _, _) = apikey::new_key();
    let h = format!("Bearer {other}");
    let d = a
        .authenticate(&req("GET", "/", &[("Authorization", &h)]))
        .unwrap_err();
    assert_eq!((d.reason, d.method), ("unknown_key", Some("api_key")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn embed_links_become_scoped_page_sessions() {
    let dir = tmpdir();
    let (a, _) = all_methods(idp(&[]), &dir);
    let link = embed_link(json!({"dashboard": "sales", "user": "acme-1",
        "claims": {"tenant": "acme"}, "vars": {"region": "EU"}, "exp": now_s() + 60}));
    let url = format!("/d/sales?embed={link}");
    let id = a.authenticate(&req("GET", &url, &[])).unwrap();
    assert_eq!(id.method, "embed");
    let page = id.page_session.clone().unwrap();
    assert_eq!(id.scope.dashboards, Some(vec!["sales".to_string()]));
    // the page session authenticates the page's requests, with the same scope
    let h = [(SESSION_HEADER, page.as_str())];
    let id2 = a.authenticate(&req("POST", "/api/panel", &h)).unwrap();
    assert_eq!(id2.claims, id.claims);
    assert_eq!(id2.scope, id.scope);
    assert!(!id2.scope.allows("payroll"));
    let mut vars = vec![("region".to_string(), "'US'".to_string())];
    id2.scope.lock_vars(&mut vars);
    assert_eq!(vars, vec![("region".to_string(), "'EU'".to_string())]);
    // the link on another dashboard, a tampered link, two links
    let other = format!("/d/payroll?embed={link}");
    assert_eq!(
        a.authenticate(&req("GET", &other, &[])).unwrap_err().reason,
        "out_of_scope"
    );
    let tampered = format!("/d/sales?embed={}x", link);
    assert!(a.authenticate(&req("GET", &tampered, &[])).is_err());
    let two = format!("/d/sales?embed={link}&embed={link}");
    assert_eq!(
        a.authenticate(&req("GET", &two, &[])).unwrap_err().reason,
        "malformed_token"
    );
    // never from the URL (#112 review): a query parameter is no credential
    let q = format!("/d/sales?embed_session={page}");
    assert_eq!(
        a.authenticate(&req("GET", &q, &[])).unwrap_err().reason,
        "no_credentials"
    );
    // serve --dashboards posts it in a form body instead
    let id3 = a
        .page_session(Some(&page), &req("POST", "/d/sales", &[]))
        .unwrap();
    assert_eq!(id3.scope, id.scope);
    assert_eq!(
        a.page_session(Some("x.y.z"), &req("POST", "/d/sales", &[]))
            .unwrap_err()
            .reason,
        "malformed_token"
    );
    // the page session is not a login cookie (another key and audience)
    let c = format!("{}={page}", oidc::SESSION_COOKIE);
    assert_eq!(
        a.authenticate(&req("GET", "/", &[("Cookie", &c)]))
            .unwrap_err()
            .reason,
        "bad_signature"
    );
    // a link is not accepted as a page session (signed by the host app's
    // secret, not by the server)
    let h = [(SESSION_HEADER, link.as_str())];
    assert_eq!(
        a.authenticate(&req("POST", "/api/panel", &h))
            .unwrap_err()
            .reason,
        "bad_signature"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn oidc_login_session_logout_and_audit() {
    let dir = tmpdir();
    let k = es256("k1");
    let f = idp(&[&k]);
    let (a, key) = all_methods(f.clone(), &dir);
    // login → IdP
    let r = a
        .route(&req("GET", "/auth/login?next=%2Fd%2Fsales", &[]))
        .unwrap();
    assert_eq!(r.status, 302);
    let loc = r.header_value("Location").unwrap().to_string();
    let q = crate::host::serving::parse_query(&loc);
    let state_cookie = r
        .header_value("Set-Cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    // IdP → callback
    let now = now_s();
    let idt = sign(
        &k,
        Some("k1"),
        &json!({"iss": ISS, "aud": "dash", "sub": "s1",
        "email": "alice@example.com", "email_verified": true, "nonce": q["nonce"], "iat": now, "exp": now + 60}),
    );
    f.set(&format!("{ISS}/token"), json!({"id_token": idt}));
    let cb = format!("/auth/callback?code={TEST_AUTH_CODE}&state={}", q["state"]);
    let h = [("Cookie", state_cookie.as_str())];
    let r = a.route(&req("GET", &cb, &h)).unwrap();
    assert_eq!(r.status, 303, "{:?}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.header_value("Location"), Some("/d/sales"));
    let cookies: Vec<&str> = r
        .headers
        .iter()
        .filter(|(k, _)| k == "Set-Cookie")
        .map(|(_, v)| v.as_str())
        .collect();
    let session = cookies
        .iter()
        .find(|c| c.starts_with(oidc::SESSION_COOKIE) && !c.contains("Max-Age=0"))
        .unwrap();
    assert!(
        session.contains("HttpOnly")
            && session.contains("Secure")
            && session.contains("SameSite=Lax")
    );
    let session = session.split(';').next().unwrap().to_string();
    // the session authenticates
    let h = [("Cookie", session.as_str())];
    let id = a.authenticate(&req("GET", "/d/sales", &h)).unwrap();
    assert_eq!(
        (id.method, id.claims.user.as_str()),
        ("oidc", "alice@example.com")
    );
    // two session cookies (cookie tossing) are refused
    let both = format!("{session}; {session}");
    let h2 = [("Cookie", both.as_str())];
    assert!(a.authenticate(&req("GET", "/", &h2)).is_err());
    // the callback can't be replayed: the state cookie is cleared, and a
    // second use of the same state + code would need the cookie again
    let r = a.route(&req("GET", &cb, &[])).unwrap();
    assert_eq!(r.status, 401);
    // logout revokes the session
    let r = a.route(&req("POST", "/auth/logout", &h)).unwrap();
    assert_eq!(r.status, 303);
    assert!(r.header_value("Set-Cookie").unwrap().contains("Max-Age=0"));
    assert_eq!(
        a.authenticate(&req("GET", "/d/sales", &h))
            .unwrap_err()
            .reason,
        "revoked"
    );
    // a failed API key, then check the audit log
    let last = if key.ends_with('0') { '1' } else { '0' };
    let bad = format!("Bearer {}{last}", &key[..key.len() - 1]);
    assert!(a
        .authenticate(&req("GET", "/api/panel", &[("Authorization", &bad)]))
        .is_err());
    let log = std::fs::read_to_string(dir.join("audit.jsonl")).unwrap();
    let lines: Vec<serde_json::Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let events: Vec<(String, String)> = lines
        .iter()
        .map(|l| {
            (
                l["audit"].as_str().unwrap().into(),
                l["outcome"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(events[0], ("start".into(), "ok".into()));
    assert!(events.contains(&("login".into(), "ok".into())));
    assert!(events.contains(&("login".into(), "deny".into())));
    assert!(events.contains(&("logout".into(), "ok".into())));
    let key_deny = lines.iter().find(|l| l["method"] == "api_key").unwrap();
    assert_eq!(key_deny["key_id"], &key[4..16]);
    assert_eq!(key_deny["reason"], "unknown_key");
    // no secret material ever reaches the audit log
    for secret in [
        &key[17..],
        &session[oidc::SESSION_COOKIE.len() + 1..],
        &idt,
        TEST_AUTH_CODE,
        PROXY_SECRET,
        EMBED_SECRET,
    ] {
        assert!(!log.contains(secret), "audit log leaks {secret}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn public_assets_only_with_embed_and_only_code() {
    let dir = tmpdir();
    let (a, _) = all_methods(idp(&[]), &dir);
    assert!(a.public_asset("GET", "/app.js"));
    assert!(a.public_asset("GET", "/pkg/anofox_visualization_bg.wasm"));
    for p in [
        "/",
        "/d/sales",
        "/api/panel",
        "/data/m5.parquet",
        "/examples/x.sql",
        "/pkg/../x.js",
        "/auth/x.js",
    ] {
        assert!(!a.public_asset("GET", p), "{p}");
    }
    assert!(!a.public_asset("POST", "/app.js"));
    let p = Auth::from_json(
        &json!({"mode": "proxy", "shared_secret": PROXY_SECRET}),
        |_| None,
    )
    .unwrap();
    assert!(!p.public_asset("GET", "/app.js"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_secret_never_plays_two_roles() {
    // #114: the embed link secret reused as the OIDC session secret would let
    // the host application forge login cookies for anyone.
    let f = idp(&[]);
    let fetch = move |_: Option<&std::path::Path>| -> Result<Arc<dyn http::Fetch>, String> {
        Ok(f.clone())
    };
    let env = |k: &str| match k {
        "SAME" => Some(EMBED_SECRET.to_string()),
        "OTHER" => Some("other-secret-0123456789abcdef0123456789".to_string()),
        _ => None,
    };
    let oidc = |sess: &str| {
        json!({"mode": "oidc", "issuer": ISS, "client_id": "dash",
               "redirect_url": "https://dash.test/auth/callback", "session_secret_env": sess})
    };
    for bad in [
        json!([{"mode": "embed", "secret_env": "SAME"}, oidc("SAME")]),
        json!([{"mode": "embed", "secret": EMBED_SECRET}, oidc("SAME")]),
        json!([{"mode": "embed", "secret_env": "SAME"}, {"mode": "proxy", "shared_secret_env": "SAME"}]),
        json!([{"mode": "embed", "secret_env": "OTHER"}, {"mode": "proxy", "shared_secret_env": "SAME"}, oidc("SAME")]),
    ] {
        let e = Auth::from_json_with(&bad, &env, &fetch).unwrap_err();
        assert!(e.contains("different secrets"), "{bad}: {e}");
    }
    // distinct secrets, and the two session secrets shared, are fine
    let ok = json!([{"mode": "embed", "secret_env": "SAME", "session_secret_env": "OTHER"}, oidc("OTHER")]);
    assert!(Auth::from_json_with(&ok, &env, &fetch).is_ok());
}

#[test]
fn misconfiguration_fails_closed() {
    let f = idp(&[]);
    let fetch = move |_: Option<&std::path::Path>| -> Result<Arc<dyn http::Fetch>, String> {
        Ok(f.clone())
    };
    for bad in [
        json!({"mode": "jwt", "issuer": ISS}), // no audience
        json!({"mode": "jwt", "issuer": ISS, "audience": "d", "algorithms": ["none"]}),
        json!({"mode": "jwt", "issuer": "http://idp", "audience": "d"}),
        json!({"mode": "jwt", "issuer": ISS, "audience": "d", "typo": 1}),
        json!({"mode": "embed"}),
        json!({"mode": "api_keys", "file": "/nonexistent/keys.json"}),
        json!({"mode": "oidc", "issuer": ISS}),
        json!([{"mode": "embed", "secret": EMBED_SECRET}, {"mode": "embed", "secret": EMBED_SECRET}]),
    ] {
        assert!(
            Auth::from_json_with(&bad, &|_| None, &fetch).is_err(),
            "{bad}"
        );
    }
}

/// #763: share links — a public dashboard opens without a credential, a
/// password-protected one after its password, each scoped to that one
/// dashboard, with the configured claims and locked variables.
fn share_auth(extra: Option<serde_json::Value>) -> (Auth, std::path::PathBuf) {
    let dir = tmpdir();
    let mut cfg = vec![json!({"mode": "share", "dashboards": {
        "sales": {"access": "public", "claims": {"tenant": "acme"}, "vars": {"region": "EU"}},
        "ops": {"access": "password", "password_hash": share::hash_password("a long enough password")}}})];
    cfg.extend(extra);
    let mut a = Auth::from_json(&serde_json::Value::Array(cfg), |_| None).unwrap();
    let audit = dir.join("audit.jsonl");
    a.set_audit(Arc::new(AuditLog::new(audit.to_str().unwrap()).unwrap()));
    (a, audit)
}

#[test]
fn share_public_dashboard() {
    let (a, _) = share_auth(None);
    assert_eq!(a.methods(), ["share"]);
    // its page: the shared identity and a page session
    let id = a.authenticate(&req("GET", "/d/sales?v.x=1", &[])).unwrap();
    assert_eq!(
        (id.method, id.claims.user.as_str()),
        ("share", "share:sales")
    );
    assert!(crate::host::access::is_reserved_user(&id.claims.user));
    assert_eq!(
        id.claims.attrs,
        [("tenant".to_string(), Some("acme".to_string()))]
    );
    assert_eq!(id.scope.dashboards, Some(vec!["sales".to_string()]));
    let page = id.page_session.clone().unwrap();
    // its assets: the identity, no new page session
    let asset = a
        .authenticate(&req("GET", "/d/sales/assets/logo.png", &[]))
        .unwrap();
    assert!(asset.page_session.is_none());
    // the page's requests carry the page session
    let h = [(SESSION_HEADER, page.as_str())];
    let api = a.authenticate(&req("POST", "/api/panel", &h)).unwrap();
    assert_eq!((api.method, api.scope.clone()), ("share", id.scope.clone()));
    let mut vars = vec![("region".to_string(), "'US'".to_string())];
    api.scope.lock_vars(&mut vars);
    assert_eq!(vars, [("region".to_string(), "'EU'".to_string())]);
    // without it, the API and everything else of the server stay closed
    for (m, p) in [
        ("POST", "/api/panel"),
        ("GET", "/"),
        ("GET", "/d/payroll"),
        ("GET", "/d/salesx"),
    ] {
        let d = a.authenticate(&req(m, p, &[])).unwrap_err();
        assert_eq!(d.reason, "no_credentials", "{m} {p}");
        assert!(a.login_redirect(&req(m, p, &[]), &d).is_none(), "{m} {p}");
    }
    // a forged or foreign page session is refused, not ignored
    let d = a
        .authenticate(&req("POST", "/api/panel", &[(SESSION_HEADER, "x.y.z")]))
        .unwrap_err();
    assert_eq!(d.method, Some("share"));
    // a POST to the page is not a share credential
    assert!(a.authenticate(&req("POST", "/d/sales", &[])).is_err());
}

#[test]
fn share_password_dashboard() {
    let (a, audit) = share_auth(None);
    // no cookie: the password page (not a login redirect, not the dashboard)
    let d = a
        .authenticate(&req("GET", "/d/ops?v.x=1", &[]))
        .unwrap_err();
    assert_eq!((d.reason, d.method), ("no_credentials", Some("share")));
    let p = a
        .login_redirect(&req("GET", "/d/ops?v.x=1", &[]), &d)
        .unwrap();
    let body = String::from_utf8(p.body.clone()).unwrap();
    assert!(
        body.contains("action=\"/share/ops\"") && body.contains("value=\"/d/ops?v.x=1\""),
        "{body}"
    );
    // its assets without the cookie: refused, no password page
    let d = a
        .authenticate(&req("GET", "/d/ops/assets/x.png", &[]))
        .unwrap_err();
    assert!(a
        .login_redirect(&req("GET", "/d/ops/assets/x.png", &[]), &d)
        .is_none());
    // a wrong password: the page again, 401
    let wrong = a
        .share_login(
            &req("POST", "/share/ops", &[]),
            "password=nope&next=%2Fd%2Fops%3Fv.x%3D1",
        )
        .unwrap();
    assert_eq!(wrong.status, 401);
    assert!(wrong.header_value("Set-Cookie").is_none());
    // the right one: a cookie for /d/ops, back to the page it came from
    let ok = a
        .share_login(
            &req("POST", "/share/ops", &[]),
            "password=a+long+enough+password&next=%2Fd%2Fops%3Fv.x%3D1",
        )
        .unwrap();
    assert_eq!(ok.status, 303);
    assert_eq!(ok.header_value("Location"), Some("/d/ops?v.x=1"));
    let set = ok.header_value("Set-Cookie").unwrap().to_string();
    assert!(set.contains("Path=/d/ops;") && set.contains("HttpOnly") && set.contains("Secure"));
    let token = set
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("__Secure-anofox_share=")
        .to_string();
    let cookie = format!("__Secure-anofox_share={token}");
    let h = [("Cookie", cookie.as_str())];
    let id = a.authenticate(&req("GET", "/d/ops", &h)).unwrap();
    assert_eq!((id.method, id.claims.user.as_str()), ("share", "share:ops"));
    assert!(id.page_session.is_some());
    assert!(a
        .authenticate(&req("GET", "/d/ops/assets/x.png", &h))
        .is_ok());
    // the cookie is not a credential for anything else
    assert!(a.authenticate(&req("POST", "/api/panel", &h)).is_err());
    // an open redirect is not possible through `next`
    let ok2 = a
        .share_login(
            &req("POST", "/share/ops", &[]),
            "password=a+long+enough+password&next=https%3A%2F%2Fevil.example%2F",
        )
        .unwrap();
    assert_eq!(ok2.header_value("Location"), Some("/d/ops"));
    // unknown or public dashboards have no password route
    assert_eq!(
        a.share_login(&req("POST", "/share/sales", &[]), "password=x")
            .unwrap()
            .status,
        404
    );
    assert_eq!(
        a.share_login(&req("POST", "/share/zzz", &[]), "password=x")
            .unwrap()
            .status,
        404
    );
    assert_eq!(
        a.share_login(&req("GET", "/share/ops", &[]), "")
            .unwrap()
            .status,
        405
    );
    assert!(a.share_login(&req("POST", "/auth/x", &[]), "").is_none());
    // the audit log: the login and the refusal, never the password
    let log = std::fs::read_to_string(&audit).unwrap();
    assert!(
        log.contains("\"audit\":\"share_login\"") && log.contains("\"wrong_password\""),
        "{log}"
    );
    assert!(!log.contains("a long enough password") && !log.contains("nope"));
    assert!(
        !log.contains("zzz"),
        "an unknown id from the URL is not logged"
    );
}

#[test]
fn share_lockout_and_other_logins_first() {
    let (a, _) = share_auth(Some(
        json!({"mode": "embed", "secret": EMBED_SECRET, "claims": ["tenant"]}),
    ));
    for _ in 0..share::MAX_FAILS_PER_PEER {
        assert_eq!(
            a.share_login(&req("POST", "/share/ops", &[]), "password=x")
                .unwrap()
                .status,
            401
        );
    }
    let locked = a
        .share_login(
            &req("POST", "/share/ops", &[]),
            "password=a+long+enough+password",
        )
        .unwrap();
    assert_eq!(locked.status, 429);
    assert!(locked.header_value("Retry-After").is_some());
    // an embed page session still works next to share
    let link = embed_link(json!({"dashboard": "sales", "user": "acme-1", "exp": now_s() + 60}));
    let id = a
        .authenticate(&req("GET", &format!("/d/sales?embed={link}"), &[]))
        .unwrap();
    assert_eq!(id.method, "embed");
    let page = id.page_session.unwrap();
    let api = a
        .authenticate(&req(
            "POST",
            "/api/panel",
            &[(SESSION_HEADER, page.as_str())],
        ))
        .unwrap();
    assert_eq!((api.method, api.claims.user.as_str()), ("embed", "acme-1"));
    // the static server's form posts: a share page session in `embed_session`
    let sp = a
        .authenticate(&req("GET", "/d/sales", &[]))
        .unwrap()
        .page_session
        .unwrap();
    assert_eq!(
        a.page_session(Some(&sp), &req("POST", "/d/sales", &[]))
            .unwrap()
            .method,
        "share"
    );
    assert!(a.public_asset("GET", "/app.js") && !a.public_asset("GET", "/d/ops"));
}

#[test]
fn share_config_errors() {
    let e = Auth::from_json(
        &json!({"mode": "share", "dashboards": {"x": {"access": "public"}},
        "session_secret": "0123456789abcdef0123456789abcdef"}),
        |_| None,
    );
    assert!(e.is_ok());
    let both = Auth::from_json(
        &json!([
        {"mode": "share", "dashboards": {"x": {"access": "public"}}, "session_secret": PROXY_SECRET},
        {"mode": "proxy", "shared_secret": PROXY_SECRET}]),
        |_| None,
    );
    assert!(both.unwrap_err().contains("different secrets"));
    // the proxy may never pose as a share user
    let p = Auth::from_json(
        &json!({"mode": "proxy", "shared_secret": PROXY_SECRET}),
        |_| None,
    )
    .unwrap();
    let h = [
        ("X-Anofox-Proxy-Secret", PROXY_SECRET),
        ("X-Forwarded-User", "share:ops"),
    ];
    assert!(p.authenticate(&req("GET", "/", &h)).is_err());
}
