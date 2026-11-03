//! The installation's tile basemap (`options.basemap`, #760): where tiles
//! come from, the narrow fetch behind `GET /api/tiles/{z}/{x}/{y}` and its
//! bounded disk cache. Threats and design: docs/security/basemap-760-review.md.
//!
//! - The source is the installation's (an `https://` URL template or a local
//!   directory template), never a dashboard's. Only the three integers of a
//!   request reach it, each checked against the zoom range ([`Tiles::get`]).
//! - Upstream: no proxy from the environment, no redirects, short timeouts,
//!   only PNG / JPEG bodies (checked by their bytes, not the header) up to
//!   [`MAX_TILE_BYTES`].
//! - The cache holds files named by `z-x-y` only, 0600 in a 0700 directory
//!   this process owns, and is trimmed to `cache_mb` (oldest first).

use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

pub use crate::map::tile_store::{MAX_TILE_BYTES, MAX_ZOOM as MAX_MAX_ZOOM};
pub const DEFAULT_MAX_ZOOM: u32 = 16;
pub const DEFAULT_CACHE_MB: u64 = 256;
pub const ATTRIBUTION_MAX_CHARS: usize = 200;
/// Tile requests per viewer and minute (a map needs about 6–20 tiles).
pub const RATE_PER_MINUTE: u32 = 600;
pub const RATE_BURST: u32 = 200;
/// Upstream fetches per minute for the whole server (cache misses only).
pub const DEFAULT_UPSTREAM_PER_MINUTE: u32 = 600;
/// Upstream fetches one server runs at once (more: `503`, retried).
pub const MAX_INFLIGHT: usize = 8;

const KEYS: &str = "url, attribution, max_zoom, cache_dir, cache_mb, upstream_per_minute";

/// Where tiles come from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// `https://…/{z}/{x}/{y}.png`.
    Https(String),
    /// `/srv/tiles/{z}/{x}/{y}.png` — an absolute path template.
    Dir(String),
}

/// Parsed, structurally valid `options.basemap`.
#[derive(Debug, Clone, PartialEq)]
pub struct BasemapOptions {
    pub source: Source,
    /// Drawn on every tiled map (most providers' terms require it).
    pub attribution: String,
    pub max_zoom: u32,
    /// The cache directory (`None`: no cache; a local directory source
    /// never needs one).
    pub cache_dir: Option<PathBuf>,
    pub cache_mb: u64,
    pub upstream_per_minute: u32,
}

/// A URL / path template: `{z}`, `{x}` and `{y}` each exactly once, and no
/// other `{`, `}`, `?`… placeholder anything could fill.
fn check_template(t: &str) -> Result<(), String> {
    for p in ["{z}", "{x}", "{y}"] {
        if t.matches(p).count() != 1 {
            return Err(format!("options.basemap.url must contain {p} once"));
        }
    }
    let rest = t.replace("{z}", "").replace("{x}", "").replace("{y}", "");
    if rest.contains(['{', '}']) {
        return Err("options.basemap.url: only {z}, {x} and {y} are filled in".into());
    }
    if t.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("options.basemap.url can't hold spaces or control characters".into());
    }
    Ok(())
}

impl BasemapOptions {
    pub fn parse(v: &Value) -> Result<BasemapOptions, String> {
        let m = v.as_object().ok_or("options.basemap must be an object")?;
        let mut source = None;
        let mut attribution = None;
        let mut o = BasemapOptions {
            source: Source::Dir(String::new()),
            attribution: String::new(),
            max_zoom: DEFAULT_MAX_ZOOM,
            cache_dir: None,
            cache_mb: DEFAULT_CACHE_MB,
            upstream_per_minute: DEFAULT_UPSTREAM_PER_MINUTE,
        };
        for (k, val) in m {
            match k.as_str() {
                "url" => {
                    let u = val
                        .as_str()
                        .map(str::trim)
                        .ok_or("options.basemap.url must be a string")?;
                    check_template(u)?;
                    source = Some(if let Some(rest) = u.strip_prefix("https://") {
                        let host = rest.split(['/', ':']).next().unwrap_or("");
                        if host.is_empty() || host.contains('@') || u.contains('@') {
                            return Err(
                                "options.basemap.url needs a host (and no user info)".into()
                            );
                        }
                        Source::Https(u.to_string())
                    } else if u.starts_with('/') {
                        if u.split('/').any(|seg| seg == "..") {
                            return Err(
                                "options.basemap.url: a directory template can't hold '..'".into(),
                            );
                        }
                        Source::Dir(u.to_string())
                    } else {
                        return Err(
                            "options.basemap.url must start with https:// or be an absolute \
                             directory template (/srv/tiles/{z}/{x}/{y}.png)"
                                .into(),
                        );
                    });
                }
                "attribution" => {
                    let a = val
                        .as_str()
                        .map(str::trim)
                        .filter(|a| !a.is_empty())
                        .ok_or("options.basemap.attribution must be the source's credit line")?;
                    if a.chars().count() > ATTRIBUTION_MAX_CHARS || a.chars().any(char::is_control)
                    {
                        return Err(format!(
                            "options.basemap.attribution: at most {ATTRIBUTION_MAX_CHARS} \
                             characters, no control characters"
                        ));
                    }
                    attribution = Some(a.to_string());
                }
                "max_zoom" => {
                    o.max_zoom = val
                        .as_u64()
                        .filter(|z| *z <= MAX_MAX_ZOOM as u64)
                        .ok_or(format!("options.basemap.max_zoom must be 0–{MAX_MAX_ZOOM}"))?
                        as u32
                }
                "cache_dir" => {
                    let p = val
                        .as_str()
                        .map(str::trim)
                        .filter(|p| p.starts_with('/'))
                        .ok_or("options.basemap.cache_dir must be an absolute directory path")?;
                    o.cache_dir = Some(PathBuf::from(p));
                }
                "cache_mb" => {
                    o.cache_mb = val
                        .as_u64()
                        .ok_or("options.basemap.cache_mb must be a whole number (0: no cache)")?
                        .min(16 * 1024)
                }
                "upstream_per_minute" => {
                    o.upstream_per_minute = val
                        .as_u64()
                        .filter(|n| *n > 0)
                        .ok_or("options.basemap.upstream_per_minute must be a positive integer")?
                        .min(100_000) as u32
                }
                other => return Err(format!("options.basemap: unknown key '{other}' ({KEYS})")),
            }
        }
        o.source = source.ok_or("options.basemap needs a url (the tile URL template)")?;
        o.attribution = attribution.ok_or(
            "options.basemap needs an attribution (the tile source's credit line, drawn on \
             every tiled map)",
        )?;
        if o.cache_mb == 0 || matches!(o.source, Source::Dir(_)) {
            o.cache_dir = None;
        } else if o.cache_dir.is_none() {
            return Err(
                "options.basemap needs a cache_dir for an https:// source (or cache_mb: 0)".into(),
            );
        }
        Ok(o)
    }
}

/// Whether a dashboard's SQL draws a tile basemap (`::MAPTILES` /
/// `::MAP_TILES`, any case) — the server only hands tiles to those.
pub fn uses_map_tiles(sql: &str) -> bool {
    let u = sql.to_ascii_uppercase();
    u.contains("::MAPTILES") || u.contains("::MAP_TILES")
}

pub use crate::map::tile_store::{image_type, valid_tile};

/// `/api/tiles/{z}/{x}/{y}` → the three numbers (plain decimal digits only).
pub fn parse_path(path: &str) -> Option<(u32, u32, u32)> {
    let rest = path.strip_prefix("/api/tiles/")?;
    let mut it = rest.split('/');
    let mut num = || {
        let s = it.next()?;
        (!s.is_empty() && s.len() <= 7 && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<u32>().ok())
            .flatten()
    };
    let (z, x, y) = (num()?, num()?, num()?);
    it.next().is_none().then_some((z, x, y))
}

fn fill(t: &str, z: u32, x: u32, y: u32) -> String {
    t.replace("{z}", &z.to_string())
        .replace("{x}", &x.to_string())
        .replace("{y}", &y.to_string())
}

/// Fetches one tile's bytes from the source. A trait so tests stand in a
/// fake upstream.
pub trait Fetch: Send + Sync {
    /// `Ok(None)`: the source has no such tile.
    fn fetch(&self, url: &str) -> Result<Option<Vec<u8>>, String>;
}

/// The real client (ureq + rustls on aws-lc-rs, the Mozilla roots).
pub struct HttpFetch {
    agent: ureq::Agent,
}

impl HttpFetch {
    pub fn new() -> HttpFetch {
        use std::sync::Arc;
        use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::WebPki)
            .unversioned_rustls_crypto_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .https_only(true)
            .proxy(None)
            // a redirect could point anywhere (T4)
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent("anofox-visualization")
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_global(Some(Duration::from_secs(15)))
            .build();
        HttpFetch {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl Default for HttpFetch {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetch for HttpFetch {
    fn fetch(&self, url: &str) -> Result<Option<Vec<u8>>, String> {
        let mut resp = self
            .agent
            .get(url)
            .header("accept", "image/png, image/jpeg")
            .call()
            .map_err(|e| e.to_string())?;
        match resp.status().as_u16() {
            200 => {}
            404 | 204 => return Ok(None),
            s => return Err(format!("the tile source answered {s}")),
        }
        let mut body = Vec::new();
        resp.body_mut()
            .as_reader()
            .take(MAX_TILE_BYTES as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|e| e.to_string())?;
        Ok(Some(body))
    }
}

/// Why a tile couldn't be served.
#[derive(Debug, PartialEq)]
pub enum TileError {
    /// Not a tile of this source (out of the zoom range or the grid).
    Invalid,
    /// The source has none.
    Missing,
    /// The source failed or answered something that isn't a tile (logged
    /// by the server, not shown to the viewer).
    Upstream(String),
}

/// The basemap a server or a CLI render uses.
pub struct Tiles {
    pub opts: BasemapOptions,
    fetch: Box<dyn Fetch>,
    cache: Option<Cache>,
}

impl Tiles {
    /// Opens the cache directory (creating it 0700 when missing).
    pub fn open(opts: BasemapOptions) -> Result<Tiles, String> {
        Self::with_fetch(opts, Box::new(HttpFetch::new()))
    }

    pub fn with_fetch(opts: BasemapOptions, fetch: Box<dyn Fetch>) -> Result<Tiles, String> {
        let cache = match &opts.cache_dir {
            Some(d) => Some(Cache::open(d, opts.cache_mb * 1024 * 1024)?),
            None => None,
        };
        Ok(Tiles { opts, fetch, cache })
    }

    /// Whether a fetch of `z/x/y` would go upstream (not cached, https).
    pub fn is_upstream(&self, z: u32, x: u32, y: u32) -> bool {
        matches!(self.opts.source, Source::Https(_))
            && !self.cache.as_ref().is_some_and(|c| c.has(z, x, y))
    }

    /// Tile `z/x/y`: its bytes and type.
    pub fn get(&self, z: u32, x: u32, y: u32) -> Result<(Vec<u8>, &'static str), TileError> {
        if !valid_tile(z, x, y, self.opts.max_zoom) {
            return Err(TileError::Invalid);
        }
        if let Some(b) = self.cache.as_ref().and_then(|c| c.get(z, x, y)) {
            if let Some(t) = image_type(&b) {
                return Ok((b, t));
            }
        }
        let bytes = match &self.opts.source {
            Source::Dir(t) => match read_tile_file(Path::new(&fill(t, z, x, y))) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(TileError::Missing)
                }
                Err(e) => return Err(TileError::Upstream(e.to_string())),
            },
            Source::Https(t) => match self.fetch.fetch(&fill(t, z, x, y)) {
                Ok(Some(b)) => b,
                Ok(None) => return Err(TileError::Missing),
                Err(e) => return Err(TileError::Upstream(e)),
            },
        };
        if bytes.len() > MAX_TILE_BYTES {
            return Err(TileError::Upstream("a tile over 512 KB".into()));
        }
        let Some(t) = image_type(&bytes) else {
            return Err(TileError::Upstream("not a PNG or JPEG tile".into()));
        };
        if let Some(c) = &self.cache {
            c.put(z, x, y, &bytes);
        }
        Ok((bytes, t))
    }
}

impl Tiles {
    /// The renderer's basemap from these tiles, for a native render (an
    /// export, `dashboard --svg`): `admit` decides each upstream fetch (a
    /// server's limits; `|| true` for the CLI). A failed tile is left blank.
    pub fn basemap(
        self: &std::sync::Arc<Self>,
        admit: impl Fn() -> bool + Send + Sync + 'static,
    ) -> crate::Basemap {
        let t = self.clone();
        crate::Basemap {
            attribution: self.opts.attribution.clone(),
            max_zoom: self.opts.max_zoom,
            tiles: std::sync::Arc::new(move |z, x, y| {
                if t.is_upstream(z, x, y) && !admit() {
                    return None;
                }
                t.get(z, x, y).ok().map(|(b, _)| b)
            }),
        }
    }

    /// `options.basemap` given on the command line: the JSON object itself
    /// or a file holding it.
    pub fn from_arg(arg: &str) -> Result<Tiles, String> {
        let text = if arg.trim_start().starts_with('{') {
            arg.to_string()
        } else {
            std::fs::read_to_string(arg).map_err(|e| format!("--basemap {arg}: {e}"))?
        };
        let v: Value =
            serde_json::from_str(&text).map_err(|e| format!("--basemap: not JSON ({e})"))?;
        Tiles::open(BasemapOptions::parse(&v)?)
    }
}

/// A local tile file: a regular file, not a link, at most the tile cap.
fn read_tile_file(p: &Path) -> std::io::Result<Vec<u8>> {
    use std::os::unix::fs::OpenOptionsExt;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(p)?;
    if !f.metadata()?.is_file() {
        return Err(std::io::Error::other("not a file"));
    }
    let mut b = Vec::new();
    f.take(MAX_TILE_BYTES as u64 + 1).read_to_end(&mut b)?;
    Ok(b)
}

/// The on-disk tile cache: `z-x-y` files, trimmed to `max_bytes` by age.
struct Cache {
    dir: PathBuf,
    max_bytes: u64,
    /// name → (size, last written); the directory is this process's own.
    index: Mutex<BTreeMap<String, (u64, SystemTime)>>,
}

impl Cache {
    fn open(dir: &Path, max_bytes: u64) -> Result<Cache, String> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        let err = |e: std::io::Error| format!("options.basemap.cache_dir '{}': {e}", dir.display());
        match std::fs::symlink_metadata(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(dir)
                    .map_err(err)?;
            }
            Err(e) => return Err(err(e)),
            Ok(_) => {}
        }
        let meta = std::fs::symlink_metadata(dir).map_err(err)?;
        if !meta.file_type().is_dir() {
            return Err(format!(
                "options.basemap.cache_dir '{}' is not a directory (or is a link)",
                dir.display()
            ));
        }
        if meta.uid() != unsafe { libc::geteuid() } {
            return Err(format!(
                "options.basemap.cache_dir '{}' is not owned by this server",
                dir.display()
            ));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "options.basemap.cache_dir '{}' must be private to this server (chmod 700)",
                dir.display()
            ));
        }
        let mut index = BTreeMap::new();
        for e in std::fs::read_dir(dir).map_err(err)?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let (true, Ok(m)) = (Self::name_ok(&name), e.metadata()) {
                if m.is_file() {
                    index.insert(
                        name,
                        (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)),
                    );
                }
            }
        }
        let c = Cache {
            dir: dir.to_path_buf(),
            max_bytes,
            index: Mutex::new(index),
        };
        c.trim();
        Ok(c)
    }

    fn name(z: u32, x: u32, y: u32) -> String {
        format!("{z}-{x}-{y}")
    }

    fn name_ok(n: &str) -> bool {
        let parts: Vec<_> = n.split('-').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    }

    fn has(&self, z: u32, x: u32, y: u32) -> bool {
        self.index
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(&Self::name(z, x, y))
    }

    fn get(&self, z: u32, x: u32, y: u32) -> Option<Vec<u8>> {
        if !self.has(z, x, y) {
            return None;
        }
        read_tile_file(&self.dir.join(Self::name(z, x, y))).ok()
    }

    fn put(&self, z: u32, x: u32, y: u32, bytes: &[u8]) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if bytes.len() as u64 > self.max_bytes {
            return;
        }
        let name = Self::name(z, x, y);
        let tmp = self.dir.join(format!(".{name}.{}", std::process::id()));
        let ok = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)
            .and_then(|mut f| f.write_all(bytes))
            .and_then(|_| std::fs::rename(&tmp, self.dir.join(&name)));
        if ok.is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        self.index
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(name, (bytes.len() as u64, SystemTime::now()));
        self.trim();
    }

    /// Drop the oldest files until the cache is at most `max_bytes`.
    fn trim(&self) {
        let mut idx = self.index.lock().unwrap_or_else(|p| p.into_inner());
        let mut total: u64 = idx.values().map(|v| v.0).sum();
        if total <= self.max_bytes {
            return;
        }
        let mut by_age: Vec<_> = idx.iter().map(|(k, v)| (v.1, k.clone(), v.0)).collect();
        by_age.sort();
        for (_, name, size) in by_age {
            if total <= self.max_bytes {
                break;
            }
            let _ = std::fs::remove_file(self.dir.join(&name));
            idx.remove(&name);
            total -= size;
        }
    }

    #[cfg(test)]
    fn bytes(&self) -> u64 {
        self.index.lock().unwrap().values().map(|v| v.0).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];

    fn https(extra: Value) -> Value {
        let mut v = json!({"url": "https://tiles.example/{z}/{x}/{y}.png",
                           "attribution": "© Example", "cache_dir": "/nonexistent"});
        for (k, val) in extra.as_object().unwrap() {
            v[k] = val.clone();
        }
        v
    }

    #[test]
    fn options_parse_and_refuse() {
        let o = BasemapOptions::parse(&https(json!({}))).unwrap();
        assert_eq!(
            o.source,
            Source::Https("https://tiles.example/{z}/{x}/{y}.png".into())
        );
        assert_eq!(o.max_zoom, DEFAULT_MAX_ZOOM);
        let dir = BasemapOptions::parse(&json!({"url": "/srv/t/{z}/{x}/{y}.png",
                                                "attribution": "© me"}))
        .unwrap();
        assert_eq!(dir.cache_dir, None);
        for (bad, why) in [
            (json!({"url": "http://t/{z}/{x}/{y}.png"}), "https://"),
            (json!({"url": "https://t/{z}/{x}.png"}), "{y}"),
            (json!({"url": "https://t/{z}/{x}/{y}/{z}.png"}), "{z} once"),
            (
                json!({"url": "https://t/{z}/{x}/{y}.png?k={key}"}),
                "only {z}",
            ),
            (json!({"url": "https://u@t/{z}/{x}/{y}.png"}), "user info"),
            (json!({"url": "/srv/../etc/{z}/{x}/{y}"}), ".."),
            (json!({"url": "tiles/{z}/{x}/{y}.png"}), "absolute"),
            (json!({"attribution": ""}), "credit line"),
            (json!({"max_zoom": 22}), "max_zoom"),
            (json!({"cache_dir": "rel"}), "absolute directory"),
            (json!({"token": "x"}), "unknown key 'token'"),
        ] {
            let e = BasemapOptions::parse(&https(bad.clone())).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
        let e = BasemapOptions::parse(&json!({"url": "https://t/{z}/{x}/{y}"})).unwrap_err();
        assert!(e.contains("attribution"), "{e}");
        let e = BasemapOptions::parse(&json!({"url": "https://t/{z}/{x}/{y}",
                                              "attribution": "a"}))
        .unwrap_err();
        assert!(e.contains("cache_dir"), "{e}");
        let nocache = BasemapOptions::parse(&json!({"url": "https://t/{z}/{x}/{y}",
                                                    "attribution": "a", "cache_mb": 0}))
        .unwrap();
        assert_eq!(nocache.cache_dir, None);
    }

    #[test]
    fn only_plain_tile_paths_parse() {
        assert_eq!(parse_path("/api/tiles/3/4/5"), Some((3, 4, 5)));
        for bad in [
            "/api/tiles/3/4",
            "/api/tiles/3/4/5/6",
            "/api/tiles/3/4/-5",
            "/api/tiles/3/4/5.png",
            "/api/tiles/+3/4/5",
            "/api/tiles/3/4/99999999",
            "/api/tiles//4/5",
            "/api/tiles/3/4/5/",
        ] {
            assert_eq!(parse_path(bad), None, "{bad}");
        }
        assert!(valid_tile(0, 0, 0, 16));
        assert!(!valid_tile(2, 4, 0, 16));
        assert!(!valid_tile(2, 0, 4, 16));
        assert!(!valid_tile(17, 0, 0, 16));
    }

    /// Records the URLs it was asked for; answers with `body`.
    struct Fake {
        asked: Arc<Mutex<Vec<String>>>,
        body: Option<Vec<u8>>,
    }
    impl Fetch for Fake {
        fn fetch(&self, url: &str) -> Result<Option<Vec<u8>>, String> {
            self.asked.lock().unwrap().push(url.to_string());
            Ok(self.body.clone())
        }
    }

    fn private_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("anofox-tiles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn fetches_fill_only_the_three_numbers_and_are_cached() {
        let dir = private_dir("cache");
        let asked = Arc::new(Mutex::new(Vec::new()));
        let opts =
            BasemapOptions::parse(&https(json!({"cache_dir": dir.to_str().unwrap()}))).unwrap();
        let t = Tiles::with_fetch(
            opts,
            Box::new(Fake {
                asked: asked.clone(),
                body: Some(PNG.to_vec()),
            }),
        )
        .unwrap();
        assert!(t.is_upstream(3, 4, 5));
        assert_eq!(t.get(3, 4, 5).unwrap(), (PNG.to_vec(), "image/png"));
        assert!(!t.is_upstream(3, 4, 5));
        assert_eq!(t.get(3, 4, 5).unwrap().1, "image/png");
        assert_eq!(
            *asked.lock().unwrap(),
            vec!["https://tiles.example/3/4/5.png".to_string()]
        );
        assert_eq!(t.get(3, 8, 5), Err(TileError::Invalid));
        assert_eq!(t.get(17, 0, 0), Err(TileError::Invalid));
        assert_eq!(asked.lock().unwrap().len(), 1);
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("3-4-5")), 0o600);
        // a reopened cache still has it
        let t2 = Tiles::with_fetch(
            t.opts.clone(),
            Box::new(Fake {
                asked: asked.clone(),
                body: None,
            }),
        )
        .unwrap();
        assert!(!t2.is_upstream(3, 4, 5));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn non_images_and_oversized_bodies_are_refused_and_not_cached() {
        let dir = private_dir("bad");
        let opts =
            BasemapOptions::parse(&https(json!({"cache_dir": dir.to_str().unwrap()}))).unwrap();
        let asked = Arc::new(Mutex::new(Vec::new()));
        for body in [b"<html>".to_vec(), {
            let mut b = PNG.to_vec();
            b.resize(MAX_TILE_BYTES + 1, 0);
            b
        }] {
            let t = Tiles::with_fetch(
                opts.clone(),
                Box::new(Fake {
                    asked: asked.clone(),
                    body: Some(body),
                }),
            )
            .unwrap();
            assert!(matches!(t.get(1, 1, 1), Err(TileError::Upstream(_))));
            assert!(t.is_upstream(1, 1, 1));
        }
        let t = Tiles::with_fetch(opts, Box::new(Fake { asked, body: None })).unwrap();
        assert_eq!(t.get(1, 1, 1), Err(TileError::Missing));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cache_is_trimmed_oldest_first_and_refuses_shared_dirs() {
        let dir = private_dir("trim");
        let c = Cache::open(&dir, 25).unwrap();
        for i in 0..4 {
            c.put(1, 0, i % 2, &[0u8; 10]);
            c.put(2, 0, i, &[0u8; 10]);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(c.bytes() <= 25, "{}", c.bytes());
        assert!(c.has(2, 0, 3), "the newest stays");
        assert!(!c.has(2, 0, 0), "the oldest goes");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let e = Cache::open(&dir, 25).err().unwrap();
        assert!(e.contains("private"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
        let link = private_dir("link");
        std::os::unix::fs::symlink("/tmp", &link).unwrap();
        let e = Cache::open(&link, 25).err().unwrap();
        assert!(e.contains("link"), "{e}");
        std::fs::remove_file(&link).unwrap();
    }

    #[test]
    fn a_directory_source_reads_regular_files_only() {
        let dir = private_dir("src");
        std::fs::create_dir_all(dir.join("2/1")).unwrap();
        std::fs::write(dir.join("2/1/3.png"), PNG).unwrap();
        std::os::unix::fs::symlink(dir.join("2/1/3.png"), dir.join("2/1/2.png")).unwrap();
        let opts = BasemapOptions::parse(&json!({
            "url": format!("{}/{{z}}/{{x}}/{{y}}.png", dir.display()),
            "attribution": "© me"
        }))
        .unwrap();
        let t = Tiles::with_fetch(
            opts,
            Box::new(Fake {
                asked: Arc::new(Mutex::new(Vec::new())),
                body: None,
            }),
        )
        .unwrap();
        assert!(!t.is_upstream(2, 1, 3));
        assert_eq!(t.get(2, 1, 3).unwrap().1, "image/png");
        assert_eq!(t.get(2, 1, 0), Err(TileError::Missing));
        assert!(
            matches!(t.get(2, 1, 2), Err(TileError::Upstream(_))),
            "a link is refused"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
