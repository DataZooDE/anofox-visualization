//! JWT verification against an identity provider's public keys (JWKS), and
//! the mapping of a verified token's fields to [`Claims`].
//!
//! Only asymmetric algorithms are accepted (`RS*`, `PS*`, `ES256`, `ES384`,
//! `EdDSA`): an `HS*` or `none` token is refused before any key is looked at,
//! and a symmetric (`oct`) JWK is never loaded, so a public key can never be
//! used as an HMAC secret (algorithm confusion). Each key is pinned to the
//! algorithms of its type and curve (and to its own `alg` when the JWK names
//! one); a token's `kid` must name a key of the set (a token without `kid` is
//! only accepted while the set holds exactly one key). `jku`, `x5u` and an
//! embedded `jwk` in the token header are never followed; a `crit` header is
//! refused. `iss` must match exactly, `aud` must contain a configured
//! audience, `exp` is required, `nbf` and `iat` are checked, all with a bounded
//! clock skew (`leeway_s`, at most 5 minutes).
//!
//! The key set is fetched over HTTPS ([`super::http`]) from `jwks_url` or the
//! issuer's discovery document, cached for `jwks_ttl_s`, refetched for an
//! unknown `kid` at most every [`MIN_REFETCH`], and dropped (fail closed) when
//! it could not be refreshed for [`MAX_STALE`].

use super::http::{require_https, Fetch};
use crate::host::access::{
    clean_claim, valid_claim_name, AuthError, Claims, MAX_ATTRS, MAX_GROUPS,
};
use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, Jwk, KeyAlgorithm, PublicKeyUse};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// An unknown `kid` refetches the key set at most this often.
pub const MIN_REFETCH: Duration = Duration::from_secs(30);
/// A key set that could not be refreshed for this long is dropped.
pub const MAX_STALE: Duration = Duration::from_secs(24 * 3600);
/// Upper bound of `leeway_s`.
pub const MAX_LEEWAY_S: u64 = 300;
/// Longest token accepted (bytes).
pub const MAX_TOKEN_LEN: usize = 16 * 1024;
/// RSA keys shorter than this are not loaded.
const MIN_RSA_BITS: usize = 2048;

/// The asymmetric algorithms a token may use.
pub const ASYMMETRIC: [Algorithm; 9] = [
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::PS256,
    Algorithm::PS384,
    Algorithm::PS512,
    Algorithm::ES256,
    Algorithm::ES384,
    Algorithm::EdDSA,
];

/// Parse a configured algorithm name; symmetric ones and `none` are refused.
pub fn parse_alg(name: &str) -> Result<Algorithm, String> {
    let alg: Algorithm = name
        .parse()
        .map_err(|_| format!("unknown JWT algorithm '{name}'"))?;
    if !ASYMMETRIC.contains(&alg) {
        return Err(format!(
            "JWT algorithm '{name}' is not allowed: only asymmetric algorithms (RS256, ES256, EdDSA, …) — never HS* or none"
        ));
    }
    Ok(alg)
}

/// The OIDC discovery document of an issuer (the parts the login needs),
/// fetched once and kept.
#[derive(Debug, Clone)]
pub struct Discovered {
    pub jwks_uri: String,
    pub authorization_endpoint: Option<String>,
    pub token_endpoint: Option<String>,
    pub end_session_endpoint: Option<String>,
}

/// An identity provider: its issuer, HTTP access and discovery cache.
pub struct Provider {
    pub issuer: String,
    pub fetch: Arc<dyn Fetch>,
    discovered: Mutex<(Option<Discovered>, Option<Instant>)>,
}

impl Provider {
    pub fn new(issuer: String, fetch: Arc<dyn Fetch>) -> Provider {
        Provider {
            issuer,
            fetch,
            discovered: Mutex::new((None, None)),
        }
    }

    /// `<issuer>/.well-known/openid-configuration`, validated: the document's
    /// `issuer` must be the configured one and every endpoint HTTPS.
    pub fn discover(&self) -> Result<Discovered, String> {
        let mut g = self.discovered.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(d) = &g.0 {
            return Ok(d.clone());
        }
        if g.1.is_some_and(|t| t.elapsed() < MIN_REFETCH) {
            return Err("identity provider discovery failed recently; retrying later".into());
        }
        g.1 = Some(Instant::now());
        let url = format!(
            "{}/.well-known/openid-configuration",
            self.issuer.trim_end_matches('/')
        );
        let doc = self.fetch.get_json(&url)?;
        if doc.get("issuer").and_then(Value::as_str) != Some(self.issuer.as_str()) {
            return Err(format!(
                "{url}: the document's issuer is not '{}'",
                self.issuer
            ));
        }
        let endpoint = |k: &str| -> Result<Option<String>, String> {
            match doc.get(k).and_then(Value::as_str) {
                None => Ok(None),
                Some(u) => {
                    require_https(u, k)?;
                    Ok(Some(u.to_string()))
                }
            }
        };
        let d = Discovered {
            jwks_uri: endpoint("jwks_uri")?.ok_or_else(|| format!("{url}: no jwks_uri"))?,
            authorization_endpoint: endpoint("authorization_endpoint")?,
            token_endpoint: endpoint("token_endpoint")?,
            end_session_endpoint: endpoint("end_session_endpoint")?,
        };
        g.0 = Some(d.clone());
        Ok(d)
    }
}

/// One usable key of the set.
struct Key {
    kid: Option<String>,
    /// The algorithms this key may verify.
    algs: Vec<Algorithm>,
    key: DecodingKey,
}

/// Load one JWK, or `None` for a key that must not (or can't) verify tokens.
fn load_key(jwk: &Jwk, allowed: &[Algorithm]) -> Option<Key> {
    if matches!(&jwk.common.public_key_use, Some(u) if *u != PublicKeyUse::Signature) {
        return None;
    }
    let mut algs: Vec<Algorithm> = match &jwk.algorithm {
        AlgorithmParameters::RSA(p) => {
            use base64::Engine;
            let n = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(p.n.trim_end_matches('='))
                .ok()?;
            let bits = n.iter().skip_while(|b| **b == 0).count() * 8;
            if bits < MIN_RSA_BITS {
                return None;
            }
            ASYMMETRIC[..6].to_vec()
        }
        AlgorithmParameters::EllipticCurve(p) => match p.curve {
            EllipticCurve::P256 => vec![Algorithm::ES256],
            EllipticCurve::P384 => vec![Algorithm::ES384],
            _ => return None,
        },
        AlgorithmParameters::OctetKeyPair(p) => match p.curve {
            EllipticCurve::Ed25519 => vec![Algorithm::EdDSA],
            _ => return None,
        },
        // `oct` (a shared secret) and anything else: never.
        _ => return None,
    };
    if let Some(ka) = &jwk.common.key_algorithm {
        let named = match ka {
            KeyAlgorithm::RS256 => Algorithm::RS256,
            KeyAlgorithm::RS384 => Algorithm::RS384,
            KeyAlgorithm::RS512 => Algorithm::RS512,
            KeyAlgorithm::PS256 => Algorithm::PS256,
            KeyAlgorithm::PS384 => Algorithm::PS384,
            KeyAlgorithm::PS512 => Algorithm::PS512,
            KeyAlgorithm::ES256 => Algorithm::ES256,
            KeyAlgorithm::ES384 => Algorithm::ES384,
            KeyAlgorithm::EdDSA => Algorithm::EdDSA,
            _ => return None,
        };
        algs.retain(|a| *a == named);
    }
    algs.retain(|a| allowed.contains(a));
    if algs.is_empty() {
        return None;
    }
    let key = DecodingKey::from_jwk(jwk).ok()?;
    Some(Key {
        kid: jwk.common.key_id.clone(),
        algs,
        key,
    })
}

#[derive(Default)]
struct JwksState {
    /// Shared with verifications in flight; replaced whole by a refetch.
    keys: Arc<Vec<Key>>,
    /// When the keys were last fetched successfully.
    fetched: Option<Instant>,
    /// When a fetch was last attempted.
    tried: Option<Instant>,
}

/// The cached key set of a provider. A refetch runs outside the state lock
/// and only one at a time (single-flight): meanwhile other requests verify
/// against the cached keys instead of waiting for the IdP.
pub struct Jwks {
    provider: Arc<Provider>,
    /// `None`: from the discovery document.
    url: Option<String>,
    ttl: Duration,
    allowed: Vec<Algorithm>,
    state: Mutex<JwksState>,
    fetching: Mutex<()>,
}

impl Jwks {
    /// Pretend the last fetch was `by` ago (tests of the refetch limits).
    #[cfg(test)]
    pub fn age(&self, by: Duration) {
        let mut st = self.state.lock().unwrap();
        st.tried = st.tried.and_then(|t| t.checked_sub(by));
        st.fetched = st.fetched.and_then(|t| t.checked_sub(by));
    }

    pub fn new(
        provider: Arc<Provider>,
        url: Option<String>,
        ttl: Duration,
        allowed: Vec<Algorithm>,
    ) -> Jwks {
        Jwks {
            provider,
            url,
            ttl,
            allowed,
            state: Mutex::new(JwksState::default()),
            fetching: Mutex::new(()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, JwksState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Download and parse the key set (no lock held).
    fn download(&self) -> Result<Vec<Key>, String> {
        let url = match &self.url {
            Some(u) => u.clone(),
            None => self.provider.discover()?.jwks_uri,
        };
        let doc = self.provider.fetch.get_json(&url)?;
        let keys = doc
            .get("keys")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{url}: not a JWK set"))?;
        // Keys of unknown or unusable types are skipped one by one.
        let keys: Vec<Key> = keys
            .iter()
            .filter_map(|k| serde_json::from_value::<Jwk>(k.clone()).ok())
            .filter_map(|k| load_key(&k, &self.allowed))
            .collect();
        if keys.is_empty() {
            return Err(format!("{url}: no usable signing key"));
        }
        Ok(keys)
    }

    fn may_fetch(st: &JwksState) -> bool {
        st.tried.is_none_or(|t| t.elapsed() >= MIN_REFETCH)
    }

    /// Refetch unless one runs already (`wait`: wait for it instead) or one
    /// ran within [`MIN_REFETCH`] (unless `force`).
    fn refresh(&self, wait: bool, force: bool) -> Result<(), String> {
        let _flight = if wait {
            self.fetching.lock().unwrap_or_else(|p| p.into_inner())
        } else {
            match self.fetching.try_lock() {
                Ok(g) => g,
                Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => return Ok(()),
            }
        };
        {
            let mut st = self.lock();
            if !force && !Jwks::may_fetch(&st) {
                return Ok(());
            }
            st.tried = Some(Instant::now());
        }
        let keys = self.download()?;
        let mut st = self.lock();
        st.keys = Arc::new(keys);
        st.fetched = Some(Instant::now());
        Ok(())
    }

    /// Fetch the keys now (at startup; errors are only reported).
    pub fn prefetch(&self) -> Result<usize, String> {
        self.refresh(true, true)?;
        Ok(self.lock().keys.len())
    }

    /// The current keys (empty once stale for [`MAX_STALE`]: fail closed),
    /// and whether a refetch is due.
    fn current(&self) -> (Arc<Vec<Key>>, bool, bool) {
        let st = self.lock();
        let old = st.fetched.is_none_or(|t| t.elapsed() >= self.ttl);
        let keys = if st.fetched.is_some_and(|t| t.elapsed() >= MAX_STALE) {
            Arc::new(Vec::new())
        } else {
            st.keys.clone()
        };
        (keys, old, Jwks::may_fetch(&st))
    }

    /// Run `f` with the key for (`kid`, `alg`), refreshing the set when it
    /// is old or the `kid` is unknown (rate-limited, single-flight).
    fn with_key<T>(
        &self,
        kid: Option<&str>,
        alg: Algorithm,
        f: impl FnOnce(&DecodingKey) -> T,
    ) -> Result<T, &'static str> {
        let (mut keys, old, may) = self.current();
        // A first fetch is waited for; a periodic one isn't.
        if old && may {
            let _ = self.refresh(keys.is_empty(), false);
            keys = self.current().0;
        }
        let find = |keys: &[Key]| -> Option<usize> {
            match kid {
                Some(kid) => keys.iter().position(|k| k.kid.as_deref() == Some(kid)),
                // No kid: only unambiguous with a single key.
                None if keys.len() == 1 => Some(0),
                None => None,
            }
        };
        let mut i = find(&keys);
        if i.is_none() && kid.is_some() && self.current().2 {
            let _ = self.refresh(false, false);
            keys = self.current().0;
            i = find(&keys);
        }
        let key = i.map(|i| &keys[i]).ok_or("unknown_kid")?;
        if !key.algs.contains(&alg) {
            return Err("bad_alg");
        }
        Ok(f(&key.key))
    }
}

/// How a verified token's fields become [`Claims`] (`user_claim`,
/// `groups_claim`, `claims`).
#[derive(Debug, Clone)]
pub struct ClaimMap {
    pub user: String,
    pub groups: Option<String>,
    pub groups_required: bool,
    /// `(claim name, token field, required)`.
    pub attrs: Vec<(String, String, bool)>,
}

/// A token field name: printable ASCII, at most 256 bytes (namespaced claims
/// like `https://example.com/groups` included).
fn valid_field(f: &str) -> bool {
    !f.is_empty() && f.len() <= 256 && f.bytes().all(|b| b.is_ascii_graphic())
}

impl ClaimMap {
    /// Read `user_claim` (default `default_user`), `groups_claim` and
    /// `claims` ({name: "field"} or {name: {"field": …, "required": true}})
    /// from a method's configuration.
    pub fn from_json(
        o: &serde_json::Map<String, Value>,
        default_user: &str,
    ) -> Result<ClaimMap, String> {
        let user = match o.get("user_claim") {
            None => default_user.to_string(),
            Some(v) => v
                .as_str()
                .filter(|f| valid_field(f))
                .ok_or("auth.user_claim must be a token field name")?
                .to_string(),
        };
        let field = |key: &str, v: &Value| -> Result<(String, bool), String> {
            let bad = || {
                format!("auth.{key} must be a token field name or {{\"field\": name, \"required\": bool}}")
            };
            match v {
                Value::String(f) if valid_field(f) => Ok((f.clone(), false)),
                Value::Object(m) => {
                    if m.keys().any(|k| k != "field" && k != "required") {
                        return Err(bad());
                    }
                    let f = m
                        .get("field")
                        .and_then(Value::as_str)
                        .filter(|f| valid_field(f))
                        .ok_or_else(bad)?;
                    let req = match m.get("required") {
                        None => false,
                        Some(r) => r.as_bool().ok_or_else(bad)?,
                    };
                    Ok((f.to_string(), req))
                }
                _ => Err(bad()),
            }
        };
        let (groups, groups_required) = match o.get("groups_claim") {
            None | Some(Value::Null) => (None, false),
            Some(v) => {
                let (f, r) = field("groups_claim", v)?;
                (Some(f), r)
            }
        };
        let mut attrs = Vec::new();
        if let Some(c) = o.get("claims") {
            let c = c
                .as_object()
                .ok_or("auth.claims must map claim names to token fields")?;
            if c.len() > MAX_ATTRS {
                return Err(format!("auth.claims: at most {MAX_ATTRS} claims"));
            }
            for (name, v) in c {
                if !valid_claim_name(name) {
                    return Err(format!("auth.claims: invalid claim name '{name}'"));
                }
                let (f, r) = field(&format!("claims.{name}"), v)?;
                attrs.push((name.to_ascii_lowercase(), f, r));
            }
        }
        Ok(ClaimMap {
            user,
            groups,
            groups_required,
            attrs,
        })
    }

    /// The claims of a verified token. The variable names come from the
    /// configuration only; the token supplies values, which are cleaned like
    /// proxy headers (length, control characters) and later bound as literals.
    pub fn claims(&self, payload: &Value) -> Result<Claims, AuthError> {
        let user = match lookup(payload, &self.user) {
            Some(Value::String(s)) => clean_claim(s)?,
            Some(Value::Number(n)) => n.to_string(),
            None | Some(Value::Null) => return Err(AuthError::NoUser),
            Some(_) => return Err(AuthError::Malformed),
        };
        if user.is_empty() {
            return Err(AuthError::NoUser);
        }
        // An e-mail address as the identity only when the IdP verified it:
        // on IdPs where users can set their own address, an unverified one
        // would let anyone become anyone (nOAuth, #115).
        let unverified = !verified(payload.get("email_verified"));
        if unverified && is_email_field(&self.user) {
            return Err(AuthError::Unverified);
        }
        let mut groups = Vec::new();
        if let Some(f) = &self.groups {
            match lookup(payload, f) {
                None | Some(Value::Null) => {}
                Some(Value::String(s)) => groups.push(clean_claim(s)?),
                Some(Value::Array(a)) => {
                    if a.len() > MAX_GROUPS {
                        return Err(AuthError::Malformed);
                    }
                    for g in a {
                        groups.push(clean_claim(g.as_str().ok_or(AuthError::Malformed)?)?);
                    }
                }
                Some(_) => return Err(AuthError::Malformed),
            }
            groups.retain(|g| !g.is_empty());
            if self.groups_required && groups.is_empty() {
                return Err(AuthError::MissingClaim);
            }
        }
        groups.sort();
        groups.dedup();
        let mut attrs = Vec::new();
        for (name, f, required) in &self.attrs {
            let v = match lookup(payload, f) {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(clean_claim(s)?),
                Some(Value::Number(n)) => Some(n.to_string()),
                Some(Value::Bool(b)) => Some(b.to_string()),
                Some(_) => return Err(AuthError::Malformed),
            }
            .filter(|v| !v.is_empty());
            // A claim taken from an e-mail field (a policy may key on it)
            // obeys the same rule as the user.
            if v.is_some() && unverified && is_email_field(f) {
                return Err(AuthError::Unverified);
            }
            if *required && v.is_none() {
                return Err(AuthError::MissingClaim);
            }
            attrs.push((name.clone(), v));
        }
        Ok(Claims {
            user,
            groups,
            attrs,
        })
    }
}

/// Is token field `f` an e-mail address: `email`, or a path or namespaced
/// name ending in it (`profile.email`, `https://example.com/email`)?
fn is_email_field(f: &str) -> bool {
    f.rsplit(['.', '/', ':'])
        .next()
        .is_some_and(|last| last.eq_ignore_ascii_case("email"))
}

/// `email_verified` is `true` (some IdPs send the string `"true"`).
fn verified(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Bool(true))) || matches!(v, Some(Value::String(s)) if s == "true")
}

/// A token field: the top-level key `f` (namespaced URLs contain dots), or
/// else a dotted path into nested objects (`realm_access.roles`).
fn lookup<'a>(payload: &'a Value, f: &str) -> Option<&'a Value> {
    if let Some(v) = payload.get(f) {
        return Some(v);
    }
    let mut cur = payload;
    for part in f.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

/// Seconds since the epoch.
pub fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Is `t` shaped like a compact JWS (three base64url parts)?
pub fn looks_like_jwt(t: &str) -> bool {
    t.len() <= MAX_TOKEN_LEN
        && t.split('.').count() == 3
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// Map a jsonwebtoken error to a log reason.
pub fn reason(e: &jsonwebtoken::errors::Error) -> &'static str {
    use jsonwebtoken::errors::ErrorKind as K;
    match e.kind() {
        K::ExpiredSignature => "expired",
        K::ImmatureSignature => "not_yet_valid",
        K::InvalidIssuer => "bad_issuer",
        K::InvalidAudience => "bad_audience",
        K::InvalidSignature => "bad_signature",
        K::InvalidAlgorithm | K::InvalidAlgorithmName | K::MissingAlgorithm => "bad_alg",
        K::MissingRequiredClaim(_) | K::InvalidClaimFormat(_) => "missing_claim",
        _ => "malformed_token",
    }
}

/// Checks on top of jsonwebtoken's: `iat` not in the future, and not older
/// than `max_age` when set.
pub fn check_iat(payload: &Value, leeway: u64, max_age: Option<u64>) -> Result<(), &'static str> {
    let now = now_s();
    match payload.get("iat") {
        None if max_age.is_some() => Err("missing_claim"),
        None => Ok(()),
        Some(v) => {
            let iat = v.as_u64().ok_or("missing_claim")?;
            if iat > now + leeway {
                return Err("not_yet_valid");
            }
            match max_age {
                Some(m) if now.saturating_sub(iat) > m + leeway => Err("expired"),
                _ => Ok(()),
            }
        }
    }
}

/// Verifies bearer tokens (and OIDC ID tokens) of one issuer.
pub struct Verifier {
    pub provider: Arc<Provider>,
    pub audiences: Vec<String>,
    pub algorithms: Vec<Algorithm>,
    pub leeway: u64,
    pub max_age: Option<u64>,
    pub jwks: Jwks,
}

impl Verifier {
    /// Verify `token`; returns its payload, or the reason it was refused.
    pub fn verify(&self, token: &str) -> Result<Value, &'static str> {
        self.verify_typed(token).map(|(p, _)| p)
    }

    /// [`Verifier::verify`], also returning the header's `typ`.
    pub fn verify_typed(&self, token: &str) -> Result<(Value, Option<String>), &'static str> {
        if !looks_like_jwt(token) {
            return Err("malformed_token");
        }
        let header = jsonwebtoken::decode_header(token).map_err(|e| reason(&e))?;
        // No extension we'd have to understand; no key from the token itself.
        if header.crit.is_some() {
            return Err("malformed_token");
        }
        if !self.algorithms.contains(&header.alg) {
            return Err("bad_alg");
        }
        let mut v = Validation::new(header.alg);
        v.algorithms = vec![header.alg];
        v.leeway = self.leeway;
        v.validate_exp = true;
        v.validate_nbf = true;
        v.validate_aud = true;
        v.set_required_spec_claims(&["exp", "iss", "aud"]);
        v.set_issuer(&[&self.provider.issuer]);
        v.set_audience(&self.audiences);
        let data = self
            .jwks
            .with_key(header.kid.as_deref(), header.alg, |key| {
                jsonwebtoken::decode::<Value>(token, key, &v)
            })?
            .map_err(|e| reason(&e))?;
        let payload = data.claims;
        // A single issuer string, exactly (jsonwebtoken also takes a list).
        if payload.get("iss").and_then(Value::as_str) != Some(self.provider.issuer.as_str()) {
            return Err("bad_issuer");
        }
        check_iat(&payload, self.leeway, self.max_age)?;
        Ok((payload, header.typ))
    }
}

/// Parse the verification settings shared by `jwt` and `oidc`: `issuer`
/// (HTTPS), `algorithms`, `leeway_s`, `max_age_s`, `jwks_url`, `jwks_ttl_s`.
pub fn verifier_from_json(
    o: &serde_json::Map<String, Value>,
    audiences: Vec<String>,
    fetch: Arc<dyn Fetch>,
) -> Result<Verifier, String> {
    let issuer = o
        .get("issuer")
        .and_then(Value::as_str)
        .ok_or("auth.issuer is required (the identity provider's issuer URL)")?
        .to_string();
    require_https(&issuer, "auth.issuer")?;
    let algorithms = match o.get("algorithms") {
        None => vec![Algorithm::RS256, Algorithm::ES256, Algorithm::EdDSA],
        Some(v) => {
            let a = v
                .as_array()
                .filter(|a| !a.is_empty())
                .ok_or("auth.algorithms must be a non-empty list")?;
            a.iter()
                .map(|x| {
                    x.as_str()
                        .ok_or_else(|| "auth.algorithms must be strings".to_string())
                        .and_then(parse_alg)
                        .map_err(|e| format!("auth.algorithms: {e}"))
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    let secs = |k: &str, default: u64, max: u64| -> Result<u64, String> {
        match o.get(k) {
            None => Ok(default),
            Some(v) => v
                .as_u64()
                .filter(|n| *n <= max)
                .ok_or_else(|| format!("auth.{k} must be an integer 0..={max}")),
        }
    };
    let leeway = secs("leeway_s", 60, MAX_LEEWAY_S)?;
    let max_age = match o.get("max_age_s") {
        None => None,
        Some(_) => Some(secs("max_age_s", 0, 30 * 86400)?),
    };
    let ttl = secs("jwks_ttl_s", 3600, 86400)?.max(60);
    let jwks_url = match o.get("jwks_url") {
        None => None,
        Some(v) => {
            let u = v.as_str().ok_or("auth.jwks_url must be a string")?;
            require_https(u, "auth.jwks_url")?;
            Some(u.to_string())
        }
    };
    let provider = Arc::new(Provider::new(issuer, fetch));
    Ok(Verifier {
        jwks: Jwks::new(
            provider.clone(),
            jwks_url,
            Duration::from_secs(ttl),
            algorithms.clone(),
        ),
        provider,
        audiences,
        algorithms,
        leeway,
        max_age,
    })
}

#[cfg(test)]
pub(crate) mod testkit {
    //! Keys and a fake identity provider for the auth tests.
    use super::*;
    use aws_lc_rs::signature::{
        EcdsaKeyPair, Ed25519KeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING,
    };
    use base64::Engine;
    use jsonwebtoken::{EncodingKey, Header};
    use std::collections::HashMap;

    pub const ISS: &str = "https://idp.test";

    pub fn b64(b: &[u8]) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
    }

    /// A signing key and its public JWK.
    pub struct TestKey {
        pub enc: EncodingKey,
        pub alg: Algorithm,
        pub jwk: Value,
    }

    pub fn es256(kid: &str) -> TestKey {
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            &aws_lc_rs::rand::SystemRandom::new(),
        )
        .unwrap();
        let kp =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref()).unwrap();
        let pubk = kp.public_key().as_ref(); // 0x04 || x || y
        TestKey {
            enc: EncodingKey::from_ec_der(pkcs8.as_ref()),
            alg: Algorithm::ES256,
            jwk: serde_json::json!({"kty": "EC", "crv": "P-256", "kid": kid, "use": "sig",
                "x": b64(&pubk[1..33]), "y": b64(&pubk[33..65])}),
        }
    }

    pub fn ed25519(kid: &str) -> TestKey {
        let kp = Ed25519KeyPair::generate().unwrap();
        let pkcs8 = kp.to_pkcs8().unwrap();
        TestKey {
            enc: EncodingKey::from_ed_der(pkcs8.as_ref()),
            alg: Algorithm::EdDSA,
            jwk: serde_json::json!({"kty": "OKP", "crv": "Ed25519", "kid": kid,
                "x": b64(kp.public_key().as_ref())}),
        }
    }

    /// A fixed 2048-bit key (aws-lc-rs exports generated RSA keys as PKCS#8
    /// only; jsonwebtoken signs with PKCS#1). Test-only, never a real key.
    pub fn rs256(kid: &str) -> TestKey {
        let der: &[u8] = include_bytes!("testdata/rsa2048-test-only.der");
        let kp = aws_lc_rs::rsa::KeyPair::from_der(der).unwrap();
        let pk = kp.public_key();
        TestKey {
            enc: EncodingKey::from_rsa_der(der),
            alg: Algorithm::RS256,
            jwk: serde_json::json!({"kty": "RSA", "kid": kid, "alg": "RS256",
                "n": b64(pk.modulus().big_endian_without_leading_zero()),
                "e": b64(pk.exponent().big_endian_without_leading_zero())}),
        }
    }

    pub fn sign(k: &TestKey, kid: Option<&str>, payload: &Value) -> String {
        let mut h = Header::new(k.alg);
        h.kid = kid.map(str::to_string);
        jsonwebtoken::encode(&h, payload, &k.enc).unwrap()
    }

    /// A recorded POST: the URL and its form.
    pub type Post = (String, Vec<(String, String)>);

    /// A fake IdP: fixed JSON per URL, counting requests.
    #[derive(Default)]
    pub struct FakeIdp {
        pub docs: Mutex<HashMap<String, Value>>,
        pub gets: Mutex<Vec<String>>,
        pub posts: Mutex<Vec<Post>>,
        /// Every GET takes this long (a slow IdP).
        pub delay: Mutex<Duration>,
    }

    impl FakeIdp {
        pub fn set(&self, url: &str, v: Value) {
            self.docs.lock().unwrap().insert(url.to_string(), v);
        }
        pub fn count(&self, url: &str) -> usize {
            self.gets
                .lock()
                .unwrap()
                .iter()
                .filter(|u| *u == url)
                .count()
        }
    }

    impl Fetch for FakeIdp {
        fn get_json(&self, url: &str) -> Result<Value, String> {
            let d = *self.delay.lock().unwrap();
            std::thread::sleep(d);
            self.gets.lock().unwrap().push(url.to_string());
            self.docs
                .lock()
                .unwrap()
                .get(url)
                .cloned()
                .ok_or_else(|| format!("{url}: 404"))
        }
        fn post_form(
            &self,
            url: &str,
            form: &[(&str, &str)],
            _basic: Option<(&str, &str)>,
        ) -> Result<Value, String> {
            self.posts.lock().unwrap().push((
                url.to_string(),
                form.iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect(),
            ));
            self.docs
                .lock()
                .unwrap()
                .get(url)
                .cloned()
                .ok_or_else(|| format!("{url}: 404"))
        }
    }

    /// An IdP at [`ISS`] with a discovery document and these public keys.
    pub fn idp(keys: &[&TestKey]) -> Arc<FakeIdp> {
        let f = Arc::new(FakeIdp::default());
        f.set(
            &format!("{ISS}/.well-known/openid-configuration"),
            serde_json::json!({
                "issuer": ISS,
                "jwks_uri": format!("{ISS}/jwks"),
                "authorization_endpoint": format!("{ISS}/authorize"),
                "token_endpoint": format!("{ISS}/token"),
            }),
        );
        f.set(
            &format!("{ISS}/jwks"),
            serde_json::json!({"keys": keys.iter().map(|k| k.jwk.clone()).collect::<Vec<_>>()}),
        );
        f
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::*;
    use super::*;
    use serde_json::json;

    fn verifier(f: Arc<FakeIdp>, extra: Value) -> Verifier {
        let mut o = json!({"issuer": ISS});
        for (k, v) in extra.as_object().unwrap() {
            o[k] = v.clone();
        }
        verifier_from_json(o.as_object().unwrap(), vec!["dash".into()], f).unwrap()
    }

    fn good(now: u64) -> Value {
        json!({"iss": ISS, "aud": "dash", "sub": "alice", "exp": now + 300, "iat": now})
    }

    #[test]
    fn valid_tokens_of_each_key_type() {
        let (e, d, r) = (es256("e1"), ed25519("d1"), rs256("r1"));
        let f = idp(&[&e, &d, &r]);
        let v = verifier(f, json!({"algorithms": ["ES256", "EdDSA", "RS256"]}));
        let now = now_s();
        for (k, kid) in [(&e, "e1"), (&d, "d1"), (&r, "r1")] {
            let p = v.verify(&sign(k, Some(kid), &good(now))).unwrap();
            assert_eq!(p["sub"], "alice");
        }
        // the default algorithm list has no ES384/PS256 but has these three
        let v = verifier(idp(&[&e]), json!({}));
        assert!(v.verify(&sign(&e, Some("e1"), &good(now))).is_ok());
    }

    #[test]
    fn alg_none_and_hmac_confusion_are_refused() {
        let e = es256("e1");
        let f = idp(&[&e]);
        let v = verifier(f.clone(), json!({}));
        let now = now_s();
        // alg none (unsigned)
        let h = b64(br#"{"alg":"none","kid":"e1"}"#);
        let p = b64(good(now).to_string().as_bytes());
        assert_eq!(v.verify(&format!("{h}.{p}.")), Err("malformed_token"));
        assert_eq!(v.verify(&format!("{h}.{p}.x")), Err("malformed_token"));
        // HS256 signed with the public key's bytes as the secret (classic
        // confusion): refused on the algorithm, before any key is used
        let secret = e.jwk.to_string();
        let mut hdr = jsonwebtoken::Header::new(Algorithm::HS256);
        hdr.kid = Some("e1".into());
        let t = jsonwebtoken::encode(
            &hdr,
            &good(now),
            &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();
        assert_eq!(v.verify(&t), Err("bad_alg"));
        // even when the IdP publishes an `oct` key, it is never loaded
        f.set(
            &format!("{ISS}/jwks"),
            json!({"keys": [e.jwk.clone(), {"kty": "oct", "kid": "h1", "k": b64(b"0123456789abcdef0123456789abcdef")}]}),
        );
        let v = verifier(f, json!({}));
        let mut hdr = jsonwebtoken::Header::new(Algorithm::HS256);
        hdr.kid = Some("h1".into());
        let t = jsonwebtoken::encode(
            &hdr,
            &good(now),
            &jsonwebtoken::EncodingKey::from_secret(b"0123456789abcdef0123456789abcdef"),
        )
        .unwrap();
        assert_eq!(v.verify(&t), Err("bad_alg"));
        // configuring HS* or none is an error
        for bad in ["HS256", "HS512", "none"] {
            let f = idp(&[&e]);
            let o = json!({"issuer": ISS, "algorithms": [bad]});
            assert!(
                verifier_from_json(o.as_object().unwrap(), vec![], f).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_key_only_verifies_its_own_algorithm() {
        // an EC key with an RS256 header: refused (not a family mix-up)
        let e = es256("k");
        let r = rs256("k2");
        let f = idp(&[&e, &r]);
        let v = verifier(f, json!({"algorithms": ["ES256", "RS256", "EdDSA"]}));
        let now = now_s();
        let t = sign(&r, Some("k"), &good(now)); // RS256 token naming the EC key's kid
        assert_eq!(v.verify(&t), Err("bad_alg"));
        // an RSA JWK pinned to RS256 (`alg`) won't verify PS256
        let f = idp(&[&r]);
        let v = verifier(f, json!({"algorithms": ["RS256", "PS256"]}));
        let mut h = jsonwebtoken::Header::new(Algorithm::PS256);
        h.kid = Some("k2".into());
        let t = jsonwebtoken::encode(&h, &good(now), &r.enc).unwrap();
        assert_eq!(v.verify(&t), Err("bad_alg"));
    }

    #[test]
    fn time_issuer_and_audience_are_checked() {
        let e = es256("e1");
        let v = verifier(idp(&[&e]), json!({"leeway_s": 30}));
        let now = now_s();
        let tok = |p: Value| sign(&e, Some("e1"), &p);
        let with = |k: &str, val: Value| {
            let mut p = good(now);
            p[k] = val;
            tok(p)
        };
        assert!(v.verify(&tok(good(now))).is_ok());
        assert_eq!(v.verify(&with("exp", json!(now - 31))), Err("expired"));
        assert!(v.verify(&with("exp", json!(now - 10))).is_ok()); // within leeway
        assert_eq!(
            v.verify(&with("nbf", json!(now + 120))),
            Err("not_yet_valid")
        );
        assert_eq!(
            v.verify(&with("iat", json!(now + 120))),
            Err("not_yet_valid")
        );
        assert_eq!(
            v.verify(&with("iss", json!("https://evil.test"))),
            Err("bad_issuer")
        );
        assert_eq!(
            v.verify(&with("iss", json!([ISS, "https://evil.test"]))),
            Err("bad_issuer")
        );
        assert_eq!(v.verify(&with("aud", json!("other"))), Err("bad_audience"));
        assert!(v.verify(&with("aud", json!(["other", "dash"]))).is_ok());
        // exp as a string, or missing exp / iss / aud
        assert_eq!(
            v.verify(&with("exp", json!("99999999999"))),
            Err("missing_claim")
        );
        for k in ["exp", "iss", "aud"] {
            let mut p = good(now);
            p.as_object_mut().unwrap().remove(k);
            assert!(v.verify(&tok(p)).is_err(), "{k}");
        }
        // max_age needs iat and bounds it
        let v = verifier(idp(&[&e]), json!({"max_age_s": 60, "leeway_s": 0}));
        assert_eq!(v.verify(&with("iat", json!(now - 3600))), Err("expired"));
        let mut p = good(now);
        p.as_object_mut().unwrap().remove("iat");
        assert_eq!(v.verify(&tok(p)), Err("missing_claim"));
        // leeway is bounded
        let o = json!({"issuer": ISS, "leeway_s": 3600});
        assert!(verifier_from_json(o.as_object().unwrap(), vec![], idp(&[])).is_err());
    }

    #[test]
    fn tampered_and_foreign_tokens_are_refused() {
        let e = es256("e1");
        let other = es256("e1"); // same kid, a key the IdP never published
        let v = verifier(idp(&[&e]), json!({}));
        let now = now_s();
        let t = sign(&e, Some("e1"), &good(now));
        // flip the payload (sub=alice → sub=admin) keeping the signature
        let parts: Vec<&str> = t.split('.').collect();
        let mut p = good(now);
        p["sub"] = json!("admin");
        let forged = format!(
            "{}.{}.{}",
            parts[0],
            b64(p.to_string().as_bytes()),
            parts[2]
        );
        assert_eq!(v.verify(&forged), Err("bad_signature"));
        assert_eq!(
            v.verify(&sign(&other, Some("e1"), &good(now))),
            Err("bad_signature")
        );
        // garbage, oversize, a crit header
        assert_eq!(v.verify("abc"), Err("malformed_token"));
        assert_eq!(v.verify(&"a.b.c".repeat(10_000)), Err("malformed_token"));
        let mut h = jsonwebtoken::Header::new(Algorithm::ES256);
        h.kid = Some("e1".into());
        h.crit = Some(vec!["exp".into()]);
        let t = jsonwebtoken::encode(&h, &good(now), &e.enc).unwrap();
        assert_eq!(v.verify(&t), Err("malformed_token"));
    }

    #[test]
    fn kid_handling_and_jwks_refetch() {
        let (a, b) = (es256("a"), es256("b"));
        let f = idp(&[&a]);
        let v = verifier(f.clone(), json!({}));
        let now = now_s();
        let jwks = format!("{ISS}/jwks");
        assert!(v.verify(&sign(&a, Some("a"), &good(now))).is_ok());
        // no kid with a single key: fine
        assert!(v.verify(&sign(&a, None, &good(now))).is_ok());
        assert_eq!(f.count(&jwks), 1);
        // the IdP rotates in key b: the unknown kid triggers one refetch
        f.set(&jwks, json!({"keys": [a.jwk.clone(), b.jwk.clone()]}));
        // ...but not within MIN_REFETCH of the last fetch
        assert_eq!(
            v.verify(&sign(&b, Some("b"), &good(now))),
            Err("unknown_kid")
        );
        assert_eq!(f.count(&jwks), 1);
        v.jwks.age(MIN_REFETCH);
        assert!(v.verify(&sign(&b, Some("b"), &good(now))).is_ok());
        assert_eq!(f.count(&jwks), 2);
        // now two keys: a token without kid is ambiguous → refused
        assert_eq!(v.verify(&sign(&a, None, &good(now))), Err("unknown_kid"));
        // unknown kids don't hammer the IdP (rate-limited refetch)
        for i in 0..20 {
            let k = format!("x{i}");
            assert_eq!(
                v.verify(&sign(&a, Some(&k), &good(now))),
                Err("unknown_kid")
            );
        }
        assert_eq!(f.count(&jwks), 2);
        // discovery is fetched once
        assert_eq!(
            f.count(&format!("{ISS}/.well-known/openid-configuration")),
            1
        );
    }

    #[test]
    fn a_slow_jwks_refetch_blocks_nobody_else() {
        let a = es256("a");
        let f = idp(&[&a]);
        let v = Arc::new(verifier(f.clone(), json!({})));
        let now = now_s();
        assert!(v.verify(&sign(&a, Some("a"), &good(now))).is_ok());
        // the set is due for a refresh and the IdP now takes 1 s to answer
        v.jwks.age(Duration::from_secs(7200));
        *f.delay.lock().unwrap() = Duration::from_millis(1000);
        let t = sign(&a, Some("a"), &good(now));
        let (v1, t1) = (v.clone(), t.clone());
        let slow = std::thread::spawn(move || v1.verify(&t1));
        std::thread::sleep(Duration::from_millis(100));
        // meanwhile: the cached key answers at once (no wait on the fetch)
        let t0 = Instant::now();
        assert!(v.verify(&t).is_ok());
        assert!(
            t0.elapsed() < Duration::from_millis(500),
            "{:?}",
            t0.elapsed()
        );
        assert!(slow.join().unwrap().is_ok());
    }

    #[test]
    fn discovery_must_match_the_issuer_and_use_https() {
        let e = es256("e1");
        let f = idp(&[&e]);
        f.set(
            &format!("{ISS}/.well-known/openid-configuration"),
            json!({"issuer": "https://evil.test", "jwks_uri": format!("{ISS}/jwks")}),
        );
        let v = verifier(f.clone(), json!({}));
        assert_eq!(
            v.verify(&sign(&e, Some("e1"), &good(now_s()))),
            Err("unknown_kid")
        );
        let f = idp(&[&e]);
        f.set(
            &format!("{ISS}/.well-known/openid-configuration"),
            json!({"issuer": ISS, "jwks_uri": "http://idp.test/jwks"}),
        );
        let v = verifier(f, json!({}));
        assert_eq!(
            v.verify(&sign(&e, Some("e1"), &good(now_s()))),
            Err("unknown_kid")
        );
        // configured URLs must be https
        for o in [
            json!({"issuer": "http://idp.test"}),
            json!({"issuer": ISS, "jwks_url": "http://idp.test/jwks"}),
        ] {
            assert!(verifier_from_json(o.as_object().unwrap(), vec![], idp(&[])).is_err());
        }
    }

    #[test]
    fn weak_rsa_keys_are_not_loaded() {
        let k: Jwk = serde_json::from_value(json!({"kty": "RSA", "kid": "w",
            "n": b64(&[0xc5; 128]), "e": "AQAB"}))
        .unwrap();
        assert!(load_key(&k, &ASYMMETRIC).is_none());
        let k: Jwk = serde_json::from_value(
            json!({"kty": "EC", "crv": "P-256", "kid": "enc", "use": "enc",
            "x": b64(&[1; 32]), "y": b64(&[2; 32])}),
        )
        .unwrap();
        assert!(load_key(&k, &ASYMMETRIC).is_none());
    }

    #[test]
    fn claim_mapping_never_names_variables_from_the_token() {
        let m = ClaimMap::from_json(
            json!({"user_claim": "email", "groups_claim": "realm_access.roles",
                   "claims": {"tenant": "https://example.com/tenant", "level": "lvl"}})
            .as_object()
            .unwrap(),
            "sub",
        )
        .unwrap();
        let p = json!({
            "email": "o'brien@example.com",
            "email_verified": true,
            "realm_access": {"roles": ["b", "a", "b", ""]},
            "https://example.com/tenant": "acme'; SET VARIABLE __claim_user = 'admin",
            "lvl": 3,
            // fields named like reserved variables are just ignored
            "__claim_user": "admin", "__claim_groups": ["admins"], "groups": ["admins"],
        });
        let c = m.claims(&p).unwrap();
        assert_eq!(c.user, "o'brien@example.com");
        assert_eq!(c.groups, vec!["a".to_string(), "b".to_string()]);
        let vars = c.variables();
        let names: Vec<&str> = vars.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "__claim_user",
                "__claim_groups",
                "__claim_tenant",
                "__claim_level"
            ]
        );
        assert_eq!(vars[0].1, "'o''brien@example.com'");
        assert_eq!(vars[2].1, "'acme''; SET VARIABLE __claim_user = ''admin'");
        assert_eq!(vars[3].1, "'3'");
        // bad values: control characters, objects, missing user, too many groups
        assert_eq!(
            m.claims(&json!({"email": "a\nb"})),
            Err(AuthError::Malformed)
        );
        assert_eq!(
            m.claims(&json!({"email": {"x": 1}})),
            Err(AuthError::Malformed)
        );
        assert_eq!(m.claims(&json!({"sub": "x"})), Err(AuthError::NoUser));
        // #115: an e-mail identity needs email_verified = true
        for ev in [json!(false), json!("false"), json!(null), json!(1)] {
            assert_eq!(
                m.claims(&json!({"email": "victim@example.com", "email_verified": ev})),
                Err(AuthError::Unverified),
                "{ev}"
            );
        }
        assert_eq!(
            m.claims(&json!({"email": "victim@example.com"})),
            Err(AuthError::Unverified)
        );
        assert!(m
            .claims(&json!({"email": "v@example.com", "email_verified": "true"}))
            .is_ok());
        // a path or namespaced field ending in email, and a claim mapped from one
        for o in [
            json!({"user_claim": "profile.email"}),
            json!({"user_claim": "https://example.com/email"}),
            json!({"claims": {"mail": "email"}}),
        ] {
            let m = ClaimMap::from_json(o.as_object().unwrap(), "sub").unwrap();
            let p = json!({"sub": "s1", "email": "victim@example.com",
                "profile": {"email": "victim@example.com"},
                "https://example.com/email": "victim@example.com"});
            assert_eq!(m.claims(&p), Err(AuthError::Unverified), "{o}");
            let mut p = p;
            p["email_verified"] = json!(true);
            assert!(m.claims(&p).is_ok(), "{o}");
        }
        let many: Vec<String> = (0..300).map(|i| i.to_string()).collect();
        assert_eq!(
            m.claims(
                &json!({"email": "a", "email_verified": true, "realm_access": {"roles": many}})
            ),
            Err(AuthError::Malformed)
        );
        // configured names must be identifiers (no `__claim_x; DROP`)
        for bad in [
            json!({"claims": {"a-b": "x"}}),
            json!({"claims": {"user": "x"}}),
            json!({"claims": {"x": 1}}),
        ] {
            assert!(
                ClaimMap::from_json(bad.as_object().unwrap(), "sub").is_err(),
                "{bad}"
            );
        }
        // required claims
        let m = ClaimMap::from_json(
            json!({"groups_claim": {"field": "groups", "required": true},
                   "claims": {"tenant": {"field": "tid", "required": true}}})
            .as_object()
            .unwrap(),
            "sub",
        )
        .unwrap();
        assert_eq!(
            m.claims(&json!({"sub": "a", "groups": ["g"]})),
            Err(AuthError::MissingClaim)
        );
        assert_eq!(
            m.claims(&json!({"sub": "a", "tid": "t"})),
            Err(AuthError::MissingClaim)
        );
        assert!(m
            .claims(&json!({"sub": "a", "groups": "g", "tid": "t"}))
            .is_ok());
    }
}
