//! Outgoing HTTPS for the login: OIDC discovery, JWKS and the token
//! exchange. HTTPS only (`https_only`, and every configured or discovered URL
//! is checked by [`require_https`]), no redirects, a short timeout and a small
//! response cap. TLS is rustls on aws-lc-rs with the Mozilla roots, or only the
//! roots in `ca_file` when one is configured (an internal IdP).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// The outgoing requests the login makes. A trait so the unit tests can stand
/// in an identity provider without a network.
pub trait Fetch: Send + Sync {
    /// GET `url` and parse the JSON body.
    fn get_json(&self, url: &str) -> Result<serde_json::Value, String>;
    /// POST an `application/x-www-form-urlencoded` body, optionally with HTTP
    /// Basic client credentials, and parse the JSON answer.
    fn post_form(
        &self,
        url: &str,
        form: &[(&str, &str)],
        basic: Option<(&str, &str)>,
    ) -> Result<serde_json::Value, String>;
}

/// Max response body of an identity provider request.
const MAX_BODY: u64 = 1 << 20;
const TIMEOUT: Duration = Duration::from_secs(10);

/// `url` must be `https://host…`: no userinfo, no fragment, nothing but
/// printable ASCII.
pub fn require_https(url: &str, what: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| format!("{what} must be an https:// URL (got '{url}')"))?;
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    if authority.is_empty()
        || authority.contains('@')
        || url.contains('#')
        || url.len() > 2048
        || !url.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err(format!("{what} is not a valid https:// URL"));
    }
    Ok(())
}

/// The real client (ureq + rustls).
pub struct Https {
    agent: ureq::Agent,
}

impl Https {
    pub fn new(ca_file: Option<&Path>) -> Result<Https, String> {
        use ureq::tls::{Certificate, PemItem, RootCerts, TlsConfig, TlsProvider};
        let roots = match ca_file {
            None => RootCerts::WebPki,
            Some(p) => {
                let pem = std::fs::read(p)
                    .map_err(|e| format!("ca_file: cannot read {}: {e}", p.display()))?;
                let certs: Vec<Certificate<'static>> = ureq::tls::parse_pem(&pem)
                    .filter_map(|i| match i {
                        Ok(PemItem::Certificate(c)) => Some(c),
                        _ => None,
                    })
                    .collect();
                if certs.is_empty() {
                    return Err(format!("ca_file: no PEM certificate in {}", p.display()));
                }
                RootCerts::new_with_certs(&certs)
            }
        };
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(roots)
            .unversioned_rustls_crypto_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(TIMEOUT))
            .build();
        Ok(Https {
            agent: ureq::Agent::new_with_config(config),
        })
    }

    fn json(
        mut resp: ureq::http::Response<ureq::Body>,
        url: &str,
    ) -> Result<serde_json::Value, String> {
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_string()
            .map_err(|e| format!("{url}: {e}"))?;
        if !(200..300).contains(&status) {
            // The body of an error may echo the request: keep it out.
            return Err(format!("{url}: HTTP {status}"));
        }
        serde_json::from_str(&body).map_err(|e| format!("{url}: not JSON: {e}"))
    }
}

impl Fetch for Https {
    fn get_json(&self, url: &str) -> Result<serde_json::Value, String> {
        require_https(url, "URL")?;
        let resp = self
            .agent
            .get(url)
            .header("Accept", "application/json")
            .call()
            .map_err(|e| format!("{url}: {e}"))?;
        Https::json(resp, url)
    }

    fn post_form(
        &self,
        url: &str,
        form: &[(&str, &str)],
        basic: Option<(&str, &str)>,
    ) -> Result<serde_json::Value, String> {
        require_https(url, "URL")?;
        let mut req = self.agent.post(url).header("Accept", "application/json");
        if let Some((id, secret)) = basic {
            use base64::Engine;
            // RFC 6749 §2.3.1: both parts form-urlencoded first.
            let cred = format!("{}:{}", form_encode(id), form_encode(secret));
            let b64 = base64::engine::general_purpose::STANDARD.encode(cred);
            req = req.header("Authorization", format!("Basic {b64}"));
        }
        let resp = req
            .send_form(form.iter().copied())
            .map_err(|e| format!("{url}: {e}"))?;
        Https::json(resp, url)
    }
}

/// `application/x-www-form-urlencoded` encoding of one value.
pub fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'*' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Percent-encoding for a URL query value (RFC 3986 unreserved kept).
pub fn query_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_urls() {
        assert!(require_https("https://idp.example.com/.well-known/jwks.json", "x").is_ok());
        assert!(require_https("https://127.0.0.1:8953/jwks", "x").is_ok());
        for bad in [
            "http://idp.example.com/jwks",
            "HTTPS://idp",
            "https://",
            "https:///path",
            "https://user:pw@idp.example.com/",
            "https://idp.example.com/#frag",
            "https://idp.example.com/a b",
            "file:///etc/passwd",
            "",
        ] {
            assert!(require_https(bad, "x").is_err(), "{bad}");
        }
    }

    #[test]
    fn encodings() {
        assert_eq!(form_encode("a b&c=d/é"), "a+b%26c%3Dd%2F%C3%A9");
        assert_eq!(query_encode("/d/x?y=1 2"), "%2Fd%2Fx%3Fy%3D1%202");
    }
}
