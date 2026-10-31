//! Prometheus metrics for the locked server (`options.metrics`, #765):
//! request counts and latencies by route, refusals by reason, panel cache
//! outcomes — counted here with atomics — plus gauges the server reads at
//! scrape time (snapshot epoch and age, refreshes, memory in flight, the
//! state and write-back stores). Exposed as `GET /metrics` in the Prometheus
//! text format (0.0.4), behind its own bearer token.
//!
//! Every label value is one of a fixed set chosen by the server (a route
//! name, a status class, a static refusal reason): never a path, a
//! dashboard id, a user or anything else from a request, so neither the
//! number of series nor what they say can be driven by a client.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// The routes requests are counted under.
pub const ROUTES: [&str; 11] = [
    "page", "panel", "export", "asset", "health", "submit", "save", "login", "metrics", "list",
    "other",
];
/// The status classes.
const CLASSES: [&str; 5] = ["1xx", "2xx", "3xx", "4xx", "5xx"];
/// Latency buckets (seconds).
pub const BUCKETS: [f64; 12] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
];
/// Most distinct refusal reasons / cache outcomes kept (a static set in
/// practice; the cap only bounds a mistake).
const MAX_LABELS: usize = 64;

/// Which [`ROUTES`] entry a request is counted under.
pub fn route_of(method: &str, path: &str) -> &'static str {
    match (method, path) {
        (_, "/api/panel") => "panel",
        (_, p) if p.starts_with("/api/export/") => "export",
        (_, "/api/submit") => "submit",
        (_, "/api/save") => "save",
        (_, "/health" | "/healthz") => "health",
        (_, "/metrics") => "metrics",
        (_, "/") => "list",
        (_, p) if p.starts_with("/auth/") || p.starts_with("/share/") => "login",
        (_, p) if p.starts_with("/d/") && p["/d/".len()..].contains("/assets/") => "asset",
        (_, p) if p.starts_with("/d/") => "page",
        ("GET" | "HEAD", _) => "asset",
        _ => "other",
    }
}

#[derive(Default)]
struct Histogram {
    buckets: [AtomicU64; BUCKETS.len()],
    count: AtomicU64,
    /// Microseconds.
    sum_us: AtomicU64,
}

/// The server's counters.
pub struct Metrics {
    requests: [[AtomicU64; CLASSES.len()]; ROUTES.len()],
    latency: [Histogram; ROUTES.len()],
    denied: Mutex<BTreeMap<&'static str, u64>>,
    cache: Mutex<BTreeMap<&'static str, u64>>,
}

impl Default for Metrics {
    fn default() -> Metrics {
        Metrics {
            requests: Default::default(),
            latency: Default::default(),
            denied: Mutex::new(BTreeMap::new()),
            cache: Mutex::new(BTreeMap::new()),
        }
    }
}

fn bump(m: &Mutex<BTreeMap<&'static str, u64>>, k: &'static str) {
    let mut m = m.lock().unwrap_or_else(|p| p.into_inner());
    if m.len() < MAX_LABELS || m.contains_key(k) {
        *m.entry(k).or_insert(0) += 1;
    }
}

/// A label value as Prometheus quotes it (`\`, `"` and newlines escaped).
fn label(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// A number as Prometheus reads it.
fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else if v.is_nan() {
        "NaN".into()
    } else if v > 0.0 {
        "+Inf".into()
    } else {
        "-Inf".into()
    }
}

/// A gauge read at scrape time.
pub struct Gauge {
    pub name: &'static str,
    pub help: &'static str,
    /// `(labels, value)`; labels as `key="value"` pairs the server chose.
    pub values: Vec<(Vec<(&'static str, String)>, f64)>,
}

impl Gauge {
    pub fn one(name: &'static str, help: &'static str, v: f64) -> Gauge {
        Gauge {
            name,
            help,
            values: vec![(Vec::new(), v)],
        }
    }
}

impl Metrics {
    /// Count one finished request.
    pub fn record(&self, route: &'static str, status: u16, took: Duration) {
        let r = ROUTES
            .iter()
            .position(|x| *x == route)
            .unwrap_or(ROUTES.len() - 1);
        let c = (status / 100).clamp(1, 5) as usize - 1;
        self.requests[r][c].fetch_add(1, Ordering::Relaxed);
        let h = &self.latency[r];
        let s = took.as_secs_f64();
        for (i, b) in BUCKETS.iter().enumerate() {
            if s <= *b {
                h.buckets[i].fetch_add(1, Ordering::Relaxed);
            }
        }
        h.count.fetch_add(1, Ordering::Relaxed);
        h.sum_us.fetch_add(
            took.as_micros().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }

    /// Count a refused request by its (static) reason.
    pub fn denied(&self, reason: &'static str) {
        bump(&self.denied, reason);
    }

    /// Count a panel request by how it was served (`hit`, `shared`, `run`).
    pub fn cache(&self, how: &'static str) {
        bump(&self.cache, how);
    }

    /// The text exposition: the counters, then `gauges`.
    pub fn render(&self, gauges: &[Gauge]) -> String {
        let mut o = String::new();
        let _ = writeln!(
            o,
            "# HELP anofox_http_requests_total HTTP requests by route and status class."
        );
        let _ = writeln!(o, "# TYPE anofox_http_requests_total counter");
        for (r, route) in ROUTES.iter().enumerate() {
            for (c, class) in CLASSES.iter().enumerate() {
                let n = self.requests[r][c].load(Ordering::Relaxed);
                if n > 0 {
                    let _ = writeln!(
                        o,
                        "anofox_http_requests_total{{route=\"{route}\",code=\"{class}\"}} {n}"
                    );
                }
            }
        }
        let _ = writeln!(
            o,
            "# HELP anofox_http_request_duration_seconds Time to answer a request, by route."
        );
        let _ = writeln!(o, "# TYPE anofox_http_request_duration_seconds histogram");
        for (r, route) in ROUTES.iter().enumerate() {
            let h = &self.latency[r];
            let count = h.count.load(Ordering::Relaxed);
            if count == 0 {
                continue;
            }
            for (i, b) in BUCKETS.iter().enumerate() {
                let _ = writeln!(
                    o,
                    "anofox_http_request_duration_seconds_bucket{{route=\"{route}\",le=\"{}\"}} {}",
                    num(*b),
                    h.buckets[i].load(Ordering::Relaxed)
                );
            }
            let _ = writeln!(o, "anofox_http_request_duration_seconds_bucket{{route=\"{route}\",le=\"+Inf\"}} {count}");
            let sum = h.sum_us.load(Ordering::Relaxed) as f64 / 1e6;
            let _ = writeln!(
                o,
                "anofox_http_request_duration_seconds_sum{{route=\"{route}\"}} {}",
                num(sum)
            );
            let _ = writeln!(
                o,
                "anofox_http_request_duration_seconds_count{{route=\"{route}\"}} {count}"
            );
        }
        for (name, help, m, key) in [
            ("anofox_http_denied_total", "Refused requests by reason (no login, bad credential, out of scope, …).", &self.denied, "reason"),
            ("anofox_panel_requests_total", "Panel requests by how they were served: hit (result cache), shared (joined a run in flight), run.", &self.cache, "cache"),
        ] {
            let _ = writeln!(o, "# HELP {name} {help}");
            let _ = writeln!(o, "# TYPE {name} counter");
            let m = m.lock().unwrap_or_else(|p| p.into_inner());
            for (k, n) in m.iter() {
                let _ = writeln!(o, "{name}{{{key}=\"{}\"}} {n}", label(k));
            }
        }
        for g in gauges {
            let _ = writeln!(o, "# HELP {} {}", g.name, g.help);
            let _ = writeln!(o, "# TYPE {} gauge", g.name);
            for (labels, v) in &g.values {
                let l = if labels.is_empty() {
                    String::new()
                } else {
                    format!(
                        "{{{}}}",
                        labels
                            .iter()
                            .map(|(k, v)| format!("{k}=\"{}\"", label(v)))
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                };
                let _ = writeln!(o, "{}{l} {}", g.name, num(*v));
            }
        }
        o
    }
}

/// The content type of [`Metrics::render`]'s output.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes() {
        assert_eq!(route_of("POST", "/api/panel"), "panel");
        assert_eq!(route_of("GET", "/api/export/sales.svg"), "export");
        assert_eq!(route_of("GET", "/d/sales"), "page");
        assert_eq!(route_of("GET", "/d/sales/assets/x.png"), "asset");
        assert_eq!(route_of("GET", "/app.js"), "asset");
        assert_eq!(route_of("POST", "/share/ops"), "login");
        assert_eq!(route_of("GET", "/auth/login"), "login");
        assert_eq!(route_of("GET", "/healthz"), "health");
        assert_eq!(route_of("GET", "/"), "list");
        assert_eq!(route_of("DELETE", "/x"), "other");
    }

    #[test]
    fn renders_counters_histograms_and_gauges() {
        let m = Metrics::default();
        m.record("panel", 200, Duration::from_millis(30));
        m.record("panel", 200, Duration::from_secs(2));
        m.record("panel", 503, Duration::from_millis(1));
        m.record("page", 401, Duration::from_millis(1));
        m.denied("no_credentials");
        m.denied("no_credentials");
        m.cache("hit");
        let out = m.render(&[
            Gauge::one("anofox_snapshot_epoch", "The snapshot's generation.", 3.0),
            Gauge {
                name: "anofox_build_info",
                help: "The build.",
                values: vec![(vec![("version", "v1 \"x\"".into())], 1.0)],
            },
        ]);
        for want in [
            "anofox_http_requests_total{route=\"panel\",code=\"2xx\"} 2\n",
            "anofox_http_requests_total{route=\"panel\",code=\"5xx\"} 1\n",
            "anofox_http_requests_total{route=\"page\",code=\"4xx\"} 1\n",
            "anofox_http_request_duration_seconds_bucket{route=\"panel\",le=\"0.05\"} 2\n",
            "anofox_http_request_duration_seconds_bucket{route=\"panel\",le=\"2.5\"} 3\n",
            "anofox_http_request_duration_seconds_bucket{route=\"panel\",le=\"+Inf\"} 3\n",
            "anofox_http_request_duration_seconds_count{route=\"panel\"} 3\n",
            "anofox_http_denied_total{reason=\"no_credentials\"} 2\n",
            "anofox_panel_requests_total{cache=\"hit\"} 1\n",
            "# TYPE anofox_snapshot_epoch gauge\nanofox_snapshot_epoch 3\n",
            "anofox_build_info{version=\"v1 \\\"x\\\"\"} 1\n",
        ] {
            assert!(out.contains(want), "missing {want:?} in\n{out}");
        }
        // routes with no traffic have no series
        assert!(!out.contains("route=\"save\""));
        // every non-comment line is `name{labels} value`
        for l in out.lines().filter(|l| !l.starts_with('#')) {
            let v = l.rsplit(' ').next().unwrap();
            assert!(v.parse::<f64>().is_ok() || v == "+Inf", "{l}");
        }
    }
}
