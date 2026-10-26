//! `::IMAGE` / `::IMAGE_DARK` / markdown `![alt](src)` source rules — shared
//! logic for the browser's panel renderer and the headless/static renderer
//! (`dashboard.rs`).
//!
//! [`src_ok`] is the single source of truth for which image sources are
//! accepted; `web/app.js::imageSrc()` must give the **same** verdict for
//! every case in `tests/fixtures/image_src_vectors.json` (checked by
//! `tests/image.rs` here and `tests/web/image_src_test.mjs` there).
//!
//! No HTML sanitising of SVG is needed anywhere downstream: every accepted
//! source is drawn through an `<img>` (browser) or an SVG `<image href=…>`
//! (static output) — never inlined into the DOM, `<object>` or `<iframe>` —
//! so a browser runs no script and loads no external resources from inside
//! an SVG used this way.

/// What an image `src` resolves to. `Bad` carries a short machine reason
/// (not shown to a viewer; `lint.rs` turns it into a diagnostic code).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SrcKind {
    /// `data:image/<mime>…` — `mime` is one of [`DATA_IMAGE_TYPES`].
    Data(&'static str),
    /// An `https:` URL (never `http:` — mixed content on a served https page).
    Https,
    /// A relative path that stays inside the dashboard's own folder.
    Relative,
    /// Rejected. Reasons: `"empty"`, `"too-long"`, `"forbidden-character"`,
    /// `"scheme"` (anything but `https:`, including `http:`, `javascript:`,
    /// `data:text/html`, a bare `data:` with an unsupported image type, …),
    /// `"protocol-relative"` (`//host/x`), `"absolute-path"` (a leading `/`),
    /// or `"outside-folder"` (`..`, a `.` segment, a hidden segment, `\`).
    Bad(&'static str),
}

/// `data:image/<t>` MIME subtypes accepted for an image `src` (same list as
/// `host::app_brand::image_url_ok`'s `data:image/…` check and
/// `theme::safe_url`'s broader `data:image/`).
pub const DATA_IMAGE_TYPES: [&str; 6] = ["png", "jpeg", "jpg", "gif", "webp", "svg+xml"];

/// Hard length cap for an image `src` (a `data:` SVG infographic can be
/// large). `lint.rs` additionally warns above 512 000 chars but still
/// accepts the source.
pub const MAX_SRC_CHARS: usize = 2_000_000;

/// Characters never allowed in an image `src`, matching `theme::safe_url`.
fn has_forbidden_char(u: &str) -> bool {
    u.chars()
        .any(|c| "\"'<>()\\`".contains(c) || c.is_control())
}

/// Classify an `::IMAGE` / `::IMAGE_DARK` / markdown image `src`. See
/// [`SrcKind`] for the decision; `web/app.js::imageSrc()` mirrors this
/// exactly (parity guarded by `tests/fixtures/image_src_vectors.json`).
pub fn src_ok(u: &str) -> SrcKind {
    if u.is_empty() {
        return SrcKind::Bad("empty");
    }
    if u.chars().count() > MAX_SRC_CHARS {
        return SrcKind::Bad("too-long");
    }
    if has_forbidden_char(u) {
        return SrcKind::Bad("forbidden-character");
    }
    let low = u.to_ascii_lowercase();
    if low.starts_with("data:") {
        return DATA_IMAGE_TYPES
            .iter()
            .find(|t| {
                let p = format!("data:image/{t}");
                low.starts_with(&format!("{p};")) || low.starts_with(&format!("{p},"))
            })
            .map(|t| SrcKind::Data(t))
            .unwrap_or(SrcKind::Bad("scheme"));
    }
    if low.starts_with("//") {
        return SrcKind::Bad("protocol-relative");
    }
    if u.starts_with('/') {
        return SrcKind::Bad("absolute-path");
    }
    // A scheme is `letters:` where the part before the first `:` has no `/`
    // (so `c:\x` on Windows or a real path with a `:` later never counts).
    if let Some(i) = low.find(':') {
        if !low[..i].contains('/') {
            return if low.starts_with("https:") {
                SrcKind::Https
            } else {
                SrcKind::Bad("scheme")
            };
        }
    }
    // A `\` is already caught by `has_forbidden_char` above. A relative path
    // is never percent-encoded (review of #544): a browser's URL resolver
    // decodes `%2e%2e`/`%2f`/`%5c` before this ever reaches a server, so
    // `assets/%2e%2e/%2e%2e/x` would classify as `Relative` here — passing
    // the lint and the browser's own `imageSrc()` — while actually escaping
    // `assets/` once resolved. Simplest correct rule: reject any `%`.
    if u.contains('%') {
        return SrcKind::Bad("percent-encoded");
    }
    for seg in u.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.starts_with('.') {
            return SrcKind::Bad("outside-folder");
        }
    }
    SrcKind::Relative
}

/// Is `path` a relative image path under `assets/`? (lint `image/not-in-assets`;
/// decision 5 — only meaningful for [`SrcKind::Relative`] sources.)
pub fn is_under_assets(path: &str) -> bool {
    path == "assets" || path.starts_with("assets/")
}

/// Percent-decode (`%XX` → byte; `+` is left as-is — this is a URI data
/// payload, not a form body).
fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                b'A'..=b'F' => Some(c - b'A' + 10),
                _ => None,
            };
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Decode a standard-alphabet base64 string (padded or not); `None` on any
/// invalid character or truncated group.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    };
    let clean: Vec<u8> = s
        .bytes()
        .filter(|c| *c != b'=' && !c.is_ascii_whitespace())
        .collect();
    let mut out = Vec::with_capacity(clean.len() / 4 * 3 + 3);
    let mut chunk = [0u8; 4];
    let mut n = 0usize;
    for c in clean {
        chunk[n] = val(c)?;
        n += 1;
        if n == 4 {
            out.push(chunk[0] << 2 | chunk[1] >> 4);
            out.push(chunk[1] << 4 | chunk[2] >> 2);
            out.push(chunk[2] << 6 | chunk[3]);
            n = 0;
        }
    }
    match n {
        0 => {}
        2 => out.push(chunk[0] << 2 | chunk[1] >> 4),
        3 => {
            out.push(chunk[0] << 2 | chunk[1] >> 4);
            out.push(chunk[1] << 4 | chunk[2] >> 2);
        }
        _ => return None, // a lone leftover byte can't be a full base64 group
    }
    Some(out)
}

/// Decode a `data:` URI into `(mime, bytes)`, handling both `;base64,` and a
/// plain (percent-encoded) payload. `None` if `u` isn't `data:…,…`.
pub fn decode_data_uri(u: &str) -> Option<(String, Vec<u8>)> {
    let rest = u
        .strip_prefix("data:")
        .or_else(|| u.to_ascii_lowercase().starts_with("data:").then(|| &u[5..]))?;
    let comma = rest.find(',')?;
    let meta = &rest[..comma];
    let payload = &rest[comma + 1..];
    let is_b64 = meta.split(';').any(|p| p.eq_ignore_ascii_case("base64"));
    let mime = meta
        .split(';')
        .next()
        .filter(|m| !m.is_empty())
        .unwrap_or("text/plain")
        .to_string();
    let bytes = if is_b64 {
        base64_decode(payload)?
    } else {
        percent_decode(payload)
    };
    Some((mime, bytes))
}

/// `bytes`' own pixel size, by its format signature, or `None` when it can't
/// be read (an unrecognised format, a truncated file, or an SVG with
/// neither a `viewBox` nor `width`+`height` on its root element — lint
/// `image/svg-no-size`). Callers fall back to a 9:16 aspect.
pub fn intrinsic_size(bytes: &[u8], mime: &str) -> Option<(f64, f64)> {
    if mime.eq_ignore_ascii_case("image/svg+xml") || sniff_svg(bytes) {
        return svg_size(bytes);
    }
    png_size(bytes)
        .or_else(|| gif_size(bytes))
        .or_else(|| jpeg_size(bytes))
        .or_else(|| webp_size(bytes))
}

fn sniff_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_ascii_lowercase();
    let head = head.trim_start_matches('\u{feff}').trim_start();
    head.starts_with('<') && head.contains("<svg")
}

fn png_size(b: &[u8]) -> Option<(f64, f64)> {
    if b.len() < 24 || !b.starts_with(b"\x89PNG\r\n\x1a\n") || &b[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes([b[16], b[17], b[18], b[19]]);
    let h = u32::from_be_bytes([b[20], b[21], b[22], b[23]]);
    (w > 0 && h > 0).then_some((w as f64, h as f64))
}

fn gif_size(b: &[u8]) -> Option<(f64, f64)> {
    if b.len() < 10 || !(b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a")) {
        return None;
    }
    let w = u16::from_le_bytes([b[6], b[7]]);
    let h = u16::from_le_bytes([b[8], b[9]]);
    (w > 0 && h > 0).then_some((w as f64, h as f64))
}

fn jpeg_size(b: &[u8]) -> Option<(f64, f64)> {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 3 < b.len() {
        if b[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = b[i + 1];
        // Markers with no payload (RST, SOI, EOI, TEM, 0x01).
        if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            i += 2;
            continue;
        }
        if i + 4 > b.len() {
            break;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC); // DHT / JPG / DAC
        if is_sof {
            if i + 9 > b.len() {
                return None;
            }
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]);
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]);
            return (w > 0 && h > 0).then_some((w as f64, h as f64));
        }
        if marker == 0xD9 || len < 2 {
            break;
        }
        i += 2 + len;
    }
    None
}

fn webp_size(b: &[u8]) -> Option<(f64, f64)> {
    if b.len() < 30 || &b[0..4] != b"RIFF" || &b[8..12] != b"WEBP" {
        return None;
    }
    match &b[12..16] {
        b"VP8X" => {
            let w = 1 + u32::from_le_bytes([b[24], b[25], b[26], 0]);
            let h = 1 + u32::from_le_bytes([b[27], b[28], b[29], 0]);
            Some((w as f64, h as f64))
        }
        b"VP8L" if b.len() >= 25 && b[20] == 0x2F => {
            let bits = u32::from_le_bytes([b[21], b[22], b[23], b[24]]);
            let w = 1 + (bits & 0x3FFF);
            let h = 1 + ((bits >> 14) & 0x3FFF);
            Some((w as f64, h as f64))
        }
        b"VP8 " if b.len() >= 30 && b[23..26] == [0x9d, 0x01, 0x2a] => {
            let w = u16::from_le_bytes([b[26], b[27]]) & 0x3FFF;
            let h = u16::from_le_bytes([b[28], b[29]]) & 0x3FFF;
            (w > 0 && h > 0).then_some((w as f64, h as f64))
        }
        _ => None,
    }
}

/// An SVG root element's size, from `viewBox="minx miny w h"` (preferred) or
/// `width="…" height="…"` (bare numbers / `px`; `%` and other units don't
/// give a pixel aspect, so they're skipped) on the first `<svg …>` tag.
fn svg_size(bytes: &[u8]) -> Option<(f64, f64)> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let tag_start = head.find("<svg")?;
    let tag_end = head[tag_start..].find('>').map(|e| tag_start + e)?;
    let tag = &head[tag_start..tag_end];
    let attr = |name: &str| -> Option<String> {
        let needle = format!("{name}=\"");
        let i = tag.find(&needle)? + needle.len();
        let j = tag[i..].find('"')? + i;
        Some(tag[i..j].to_string())
    };
    if let Some(vb) = attr("viewBox") {
        let nums: Vec<f64> = vb
            .split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect();
        if let [_, _, w, h] = nums[..] {
            if w > 0.0 && h > 0.0 {
                return Some((w, h));
            }
        }
    }
    let num = |s: String| -> Option<f64> { s.trim_end_matches("px").trim().parse().ok() };
    match (attr("width").and_then(num), attr("height").and_then(num)) {
        (Some(w), Some(h)) if w > 0.0 && h > 0.0 => Some((w, h)),
        _ => None,
    }
}

/// Fallback aspect (width, height) when [`intrinsic_size`] can't read one
/// (decision 8/9 of plan-514): a tall placeholder, not a wide one, so a
/// missing size doesn't collapse the panel to a sliver.
pub const FALLBACK_ASPECT: (f64, f64) = (9.0, 16.0);

/// Relative image file extensions the locked server, `--site` and the CLI
/// will ever read from an `assets/` folder (case-insensitive; matches
/// [`DATA_IMAGE_TYPES`] minus the `data:` spelling quirks — `jpg`/`jpeg` are
/// both real extensions, `svg+xml` on disk is just `svg`).
pub const ASSET_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "svg"];

/// Default cap on one `assets/` file (`options.max_asset_bytes`,
/// `duckext/src/serve.rs::LockedOptions`; the CLI/`--site` paths that have
/// no such option use this directly).
pub const DEFAULT_MAX_ASSET_BYTES: u64 = 5 << 20;

/// Is `dir`'s own `assets` entry a symlink (not followed)? Review of #554
/// nit 8: `canonicalize` on a symlinked `assets -> /` (or anywhere else)
/// would otherwise resolve it to that target and serve every image-typed
/// file under it — `assets/` is meant to be a plain subfolder of the
/// dashboards folder, never a pointer elsewhere. [`canonical_assets_dir`]
/// treats this the same as a missing folder (`None`); the locked server
/// and `--site` both call this first to refuse loudly instead (a startup
/// error / a failed publish), since a silent `None` there would read as
/// "no assets configured" rather than "a symlink was refused".
pub fn assets_dir_is_symlink(dir: &std::path::Path) -> bool {
    std::fs::symlink_metadata(dir.join("assets"))
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// `dir`'s own `assets/` subfolder, canonicalized — `None` when there is no
/// such folder, or it is a symlink ([`assets_dir_is_symlink`]: every
/// relative `::IMAGE` source then draws the alt-text placeholder, as before
/// PR B, rather than this resolving a symlinked root). Computed once at
/// startup/call time by every reader of `assets/` — the locked server
/// (`duckext`'s `start_locked`), the CLI (`dashboard`'s `--svg`/HTML
/// output, `serve --dashboards`) and `--site` — never per request/render.
pub fn canonical_assets_dir(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    if assets_dir_is_symlink(dir) {
        return None;
    }
    dir.join("assets")
        .canonicalize()
        .ok()
        .filter(|p| p.is_dir())
}

/// Validate `rest` (a path under an `assets/` folder, already stripped of
/// that prefix) against `root` — the dashboards folder's `assets/`
/// directory, **already canonicalized by the caller** — and open the file
/// it names, up to (and including) the point both [`read_asset`] (the
/// whole file, `GET`) and [`stat_asset`] (its length and sniffed type
/// only, `HEAD`) need. Shared by the locked server's `GET
/// /d/<id>/assets/<rest>` route ([`duckext`]'s `serve.rs` — which, review
/// of #554 S2, calls this directly itself so it can hold its own
/// in-flight memory budget against the file's real `fstat`-reported
/// length rather than a pessimistic `max_asset_bytes` regardless of size)
/// and the CLI/static renderers ([`crate::dashboard`],
/// [`crate::image_panel_svg`]'s caller), so every reader of `assets/`
/// enforces exactly the same rule.
///
/// `rest` is percent-decoded once (a no-op for a CLI caller: its `rest`
/// was never percent-encoded in the first place, because [`src_ok`]
/// already rejects any `%` in a relative `src` — this matters for a
/// locked server, whose `rest` comes straight off the URL path). Refused
/// (`Err`, a short machine reason — never shown to a viewer): empty;
/// longer than 512 bytes; a `\`, NUL or other control character; an
/// empty, `.` or `..` segment; a segment starting with `.`; an extension
/// outside [`ASSET_EXTENSIONS`] (case-insensitive) — checked *before*
/// touching the filesystem, so a request can't even probe for a
/// non-image file's existence through this route. Then: `root.join(rest)`
/// must canonicalize to a path inside `root` (blocks `..`/absolute
/// escapes missed above and a symlink pointing outside), and the final
/// path component must open as a regular file, not a symlink or a FIFO
/// ([`open_asset_file`]).
///
/// Returns the open file and its `fstat`-reported length — **not yet
/// capped against any `max`**, since [`stat_asset`] needs the real length
/// even when it's over the cap, to refuse honestly rather than by
/// coincidence of a short read; each caller applies `max` itself
/// ([`read_opened_asset`], [`stat_opened_asset`]).
///
/// Review of #554 N1: `O_NOFOLLOW` on [`open_asset_file`] covers only this
/// *final* path component, after `canonicalize` has already resolved
/// every symlink in `rest`'s own path into `real`. A **parent** directory
/// on that already-canonical path that gets swapped for a symlink between
/// the `canonicalize` call above and this `open` — only possible for
/// someone who can already write inside `assets/` — is still followed;
/// closing that fully needs `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)`
/// (Linux-only, no portable `std` wrapper) or re-deriving the opened path
/// from `/proc/self/fd/<fd>` after the fact and re-checking containment.
/// Left as this comment, not code, until #514/#554's own low-privilege
/// threat model (a dashboards folder's own author, not an outside
/// attacker) calls for it.
pub fn open_validated_asset(
    root: &std::path::Path,
    rest: &str,
) -> Result<(std::fs::File, u64), &'static str> {
    if rest.is_empty() || rest.len() > 512 {
        return Err("bad-path");
    }
    let decoded = percent_decode(rest);
    let decoded = String::from_utf8(decoded).map_err(|_| "bad-path")?;
    if decoded.is_empty() || decoded.len() > 512 {
        return Err("bad-path");
    }
    if decoded.chars().any(|c| c == '\\' || c.is_control()) {
        return Err("bad-path");
    }
    for seg in decoded.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.starts_with('.') {
            return Err("outside-folder");
        }
    }
    let ext = decoded
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if !ASSET_EXTENSIONS.contains(&ext.as_str()) {
        return Err("bad-extension");
    }
    let real = std::fs::canonicalize(root.join(&decoded)).map_err(|_| "not-found")?;
    if !real.starts_with(root) {
        return Err("outside-folder");
    }
    // Review of #554, nit 5: one handle from here on, not a `stat` then a
    // separate `read` — a symlink or a FIFO put in `real`'s place between
    // the two would otherwise be followed/opened. `O_NOFOLLOW` refuses a
    // symlink at this final, already-canonical path (closing that race);
    // `O_NONBLOCK` means opening a FIFO never blocks waiting for a writer
    // (it also stops a `stat`-reported size of 0 from skipping the real
    // cap below — the read itself, not `stat`, is what's bounded). The
    // size cap on the actual bytes read is each caller's own job
    // (`read_opened_asset`'s `take(max + 1)`), so an attacker-controlled
    // FIFO can't lie about its length via `stat` either.
    let file = open_asset_file(&real).map_err(|_| "not-found")?;
    let meta = file.metadata().map_err(|_| "not-found")?;
    if !meta.is_file() {
        return Err("not-a-file");
    }
    Ok((file, meta.len()))
}

/// Read the whole of an already-[`open_validated_asset`]-opened file
/// (`file`, `len` its `fstat`-reported size) and sniff its real MIME type
/// — the shared tail of [`read_asset`] and the locked server's own
/// `locked_asset` (review of #554 S2: it calls [`open_validated_asset`]
/// directly first, to hold `min(len, max)` of its in-flight memory budget
/// *before* this read, rather than a pessimistic `max` regardless of the
/// file's real size).
pub fn read_opened_asset(
    file: std::fs::File,
    len: u64,
    max: u64,
) -> Result<(String, Vec<u8>), &'static str> {
    if len > max {
        return Err("too-large");
    }
    // Review of #554 N4: `take(len + 1)`, not `max + 1` — the caller's own
    // in-flight memory hold is sized to `min(len, max)` (`locked_asset`),
    // so reading up to `max` here (when `max` is larger than `len`) could
    // read more than was ever held. Refusing anything longer than the
    // `fstat`-reported `len` also catches a file that grew after that
    // `fstat` (only possible for someone who can already write inside
    // `assets/`) rather than silently serving the extra bytes.
    let mut bytes = Vec::new();
    {
        use std::io::Read;
        file.take(len + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "io-error")?;
    }
    if bytes.len() as u64 > len {
        return Err("too-large");
    }
    let mime = crate::host::app_brand::sniff_image(&bytes).ok_or("not-an-image")?;
    Ok((mime.to_string(), bytes))
}

/// `rest` (a path under `root`'s `assets/`), fully read and sniffed. See
/// [`open_validated_asset`] for the path/containment/open rules and
/// [`read_opened_asset`] for the read/size-cap/sniff rules this composes.
pub fn read_asset(
    root: &std::path::Path,
    rest: &str,
    max: u64,
) -> Result<(String, Vec<u8>), &'static str> {
    let (file, len) = open_validated_asset(root, rest)?;
    read_opened_asset(file, len, max)
}

/// Like [`read_opened_asset`], but for a `HEAD` request: the sniffed MIME
/// type and the file's real length (`fstat`, already known as `len`),
/// without reading the whole file just to answer "what would `GET` send"
/// (review of #554 nit 6). Reads only a `min(len, 4096)`-byte prefix —
/// enough for every format [`crate::host::app_brand::sniff_image`]
/// recognises (an SVG's own sniff already only looks at the first 4 KiB;
/// every other signature is well under that) — so the locked server's own
/// in-flight hold for a `HEAD` (review of #554 S2) only needs to cover
/// that prefix, not the whole file.
pub fn stat_opened_asset(
    file: std::fs::File,
    len: u64,
    max: u64,
) -> Result<(String, u64), &'static str> {
    if len > max {
        return Err("too-large");
    }
    let mut prefix = vec![0u8; len.min(4096) as usize];
    {
        use std::io::Read;
        file.take(prefix.len() as u64)
            .read_exact(&mut prefix)
            .map_err(|_| "io-error")?;
    }
    let mime = crate::host::app_brand::sniff_image(&prefix).ok_or("not-an-image")?;
    Ok((mime.to_string(), len))
}

/// `rest` (a path under `root`'s `assets/`), `fstat`-sized and sniffed
/// without a full read. See [`open_validated_asset`] for the path/
/// containment/open rules and [`stat_opened_asset`] for the sniff rule
/// this composes.
pub fn stat_asset(
    root: &std::path::Path,
    rest: &str,
    max: u64,
) -> Result<(String, u64), &'static str> {
    let (file, len) = open_validated_asset(root, rest)?;
    stat_opened_asset(file, len, max)
}

/// Open `path` (already canonical — no symlink components) for reading,
/// refusing a symlink or anything that would block waiting for a writer
/// (a FIFO) at that final path component. Unix only: `O_NOFOLLOW` has no
/// portable `std` equivalent, so other platforms fall back to a plain
/// `File::open` (the containment check above still holds; this is the
/// narrower TOCTOU-hardening belt, not the braces).
#[cfg(unix)]
fn open_asset_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}
#[cfg(not(unix))]
fn open_asset_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

/// `src` (whose [`src_ok`] verdict is [`SrcKind::Relative`]) read from
/// `asset_root`'s `assets/` folder — `None` when there is no root, `src`
/// isn't under `assets/` (decision 5 of plan-514: the locked server,
/// `--site` and the CLI only ever resolve that one subfolder), or
/// [`read_asset`] refuses it for any reason. Shared by every static/headless
/// render path (`dashboard.rs`'s `image_href`/`image_aspect`, `lib.rs`'s
/// `image_panel_svg`) so a relative image inlines the same way everywhere.
pub fn resolve_relative(
    src: &str,
    asset_root: Option<&std::path::Path>,
) -> Option<(String, Vec<u8>)> {
    let root = asset_root?;
    if !is_under_assets(src) {
        return None;
    }
    let rest = src.strip_prefix("assets/")?;
    read_asset(root, rest, DEFAULT_MAX_ASSET_BYTES).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn vectors_match_rust_and_js_contract() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/image_src_vectors.json"
        );
        let text = fs::read_to_string(path).expect("read image_src_vectors.json");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let vectors = v["vectors"].as_array().expect("vectors array");
        assert!(!vectors.is_empty());
        for entry in vectors {
            let src = entry["src"].as_str().expect("src");
            let kind = entry["kind"].as_str().expect("kind");
            let got = match src_ok(src) {
                SrcKind::Data(_) => "data",
                SrcKind::Https => "https",
                SrcKind::Relative => "relative",
                SrcKind::Bad(_) => "bad",
            };
            assert_eq!(got, kind, "src_ok({src:?})");
        }
    }

    #[test]
    fn data_uri_roundtrip() {
        let (mime, bytes) = decode_data_uri("data:image/png;base64,iVBORw0KGgo=").unwrap();
        assert_eq!(mime, "image/png");
        assert!(!bytes.is_empty());
        let (mime, bytes) = decode_data_uri("data:image/svg+xml,%3Csvg%3E").unwrap();
        assert_eq!(mime, "image/svg+xml");
        assert_eq!(bytes, b"<svg>");
    }

    #[test]
    fn https_only_not_http() {
        assert_eq!(src_ok("https://a.b/c.png"), SrcKind::Https);
        assert!(matches!(src_ok("http://a.b/c.png"), SrcKind::Bad(_)));
    }

    #[test]
    fn relative_paths() {
        assert_eq!(src_ok("assets/x.svg"), SrcKind::Relative);
        assert_eq!(src_ok("img/ok.png"), SrcKind::Relative);
        assert!(matches!(
            src_ok("assets/../x"),
            SrcKind::Bad("outside-folder")
        ));
        assert!(matches!(
            src_ok("assets/.hidden"),
            SrcKind::Bad("outside-folder")
        ));
        assert!(matches!(src_ok("/abs"), SrcKind::Bad("absolute-path")));
        assert!(matches!(src_ok("//x"), SrcKind::Bad("protocol-relative")));
        assert!(matches!(
            src_ok("a\\b"),
            SrcKind::Bad("forbidden-character")
        ));
    }

    /// #544 review: a browser's URL resolver decodes `%2e`/`%2f`/`%5c`
    /// before a fetch, so these must never classify as plain `Relative` —
    /// `assets/%2e%2e/x` resolves outside `assets/` once fetched.
    #[test]
    fn percent_encoding_is_rejected_in_a_relative_path() {
        for bad in [
            "assets/%2e%2e/x",
            "assets/%2E%2E%2Fx",
            "assets/..%2fx",
            "assets/..%2Fx",
            "assets%2f..%2fx",
            "assets/%5c..%5cx",
            "assets/x%2esvg",
        ] {
            assert!(
                matches!(src_ok(bad), SrcKind::Bad("percent-encoded")),
                "{bad}"
            );
        }
        // Still fine without any `%`.
        assert_eq!(src_ok("assets/ok.svg"), SrcKind::Relative);
    }

    #[test]
    fn rejects_scripts_and_markup() {
        assert!(matches!(src_ok("javascript:alert(1)"), SrcKind::Bad(_)));
        assert!(matches!(
            src_ok("data:text/html,<h1>x</h1>"),
            SrcKind::Bad(_)
        ));
        assert!(matches!(
            src_ok("x\"y"),
            SrcKind::Bad("forbidden-character")
        ));
        assert!(matches!(
            src_ok("x(y)"),
            SrcKind::Bad("forbidden-character")
        ));
        assert!(matches!(src_ok("x<y"), SrcKind::Bad("forbidden-character")));
    }

    #[test]
    fn length_cap() {
        let long = format!("data:image/png;base64,{}", "A".repeat(MAX_SRC_CHARS));
        assert!(matches!(src_ok(&long), SrcKind::Bad("too-long")));
    }

    #[test]
    fn png_header() {
        // 4x2 1-bit PNG IHDR (signature + length/type + width/height + rest irrelevant here).
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        b.extend([0, 0, 0, 13]); // chunk length (unused by our parser)
        b.extend(b"IHDR");
        b.extend(4u32.to_be_bytes()); // width
        b.extend(2u32.to_be_bytes()); // height
        assert_eq!(intrinsic_size(&b, "image/png"), Some((4.0, 2.0)));
    }

    #[test]
    fn svg_viewbox() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 300 150"><rect/></svg>"#;
        assert_eq!(intrinsic_size(svg, "image/svg+xml"), Some((300.0, 150.0)));
    }

    #[test]
    fn svg_no_size_is_none() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>"#;
        assert_eq!(intrinsic_size(svg, "image/svg+xml"), None);
    }

    /// A tiny `assets/` tree shared by [`read_asset`]'s own tests, dropped
    /// (and its temp dir cleaned up) when the test ends.
    struct AssetsDir(std::path::PathBuf);

    impl AssetsDir {
        fn new() -> AssetsDir {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir()
                .join(format!("anofox-image-test-{}-{nanos}", std::process::id(),));
            let assets = dir.join("assets");
            fs::create_dir_all(assets.join("sub")).unwrap();
            // A real (tiny) PNG: signature + IHDR, 1x1.
            let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
            png.extend([0, 0, 0, 13]);
            png.extend(b"IHDR");
            png.extend(1u32.to_be_bytes());
            png.extend(1u32.to_be_bytes());
            fs::write(assets.join("sub").join("b.png"), &png).unwrap();
            fs::write(
                assets.join("a.svg"),
                br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"/>"#,
            )
            .unwrap();
            fs::write(assets.join("x.sql"), b"SELECT 1;").unwrap();
            // A PNG-named file that is actually just text — the magic-byte
            // check must refuse it even though the extension is fine.
            fs::write(assets.join("fake.png"), b"not a png").unwrap();
            fs::write(dir.join("policy.json"), b"{}").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                let _ = symlink(dir.join("policy.json"), assets.join("link.png"));
            }
            AssetsDir(dir)
        }
        fn assets(&self) -> std::path::PathBuf {
            self.0.join("assets").canonicalize().unwrap()
        }
    }
    impl Drop for AssetsDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn read_asset_serves_real_images_under_the_root() {
        let d = AssetsDir::new();
        let root = d.assets();
        let (mime, bytes) = read_asset(&root, "a.svg", DEFAULT_MAX_ASSET_BYTES).unwrap();
        assert_eq!(mime, "image/svg+xml");
        assert!(!bytes.is_empty());
        let (mime, _) = read_asset(&root, "sub/b.png", DEFAULT_MAX_ASSET_BYTES).unwrap();
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn read_asset_refuses_traversal_and_bad_files() {
        let d = AssetsDir::new();
        let root = d.assets();
        for (rest, why) in [
            ("../policy.json", "outside-folder"),
            ("%2e%2e/policy.json", "outside-folder"),
            ("..%2fpolicy.json", "outside-folder"),
            (".secret.png", "outside-folder"),
            ("x.sql", "bad-extension"),
            ("a.SVG.sql", "bad-extension"),
            ("", "bad-path"),
            ("nope.png", "not-found"),
        ] {
            assert_eq!(
                read_asset(&root, rest, DEFAULT_MAX_ASSET_BYTES),
                Err(why),
                "{rest}"
            );
        }
        // A renamed non-image: right extension, wrong real bytes.
        assert_eq!(
            read_asset(&root, "fake.png", DEFAULT_MAX_ASSET_BYTES),
            Err("not-an-image")
        );
        // Oversize.
        assert_eq!(read_asset(&root, "a.svg", 4), Err("too-large"));
        // A directory, not a file.
        assert_eq!(
            read_asset(&root, "sub", DEFAULT_MAX_ASSET_BYTES),
            Err("bad-extension")
        );
    }

    #[test]
    #[cfg(unix)]
    fn read_asset_refuses_a_symlink_pointing_outside() {
        let d = AssetsDir::new();
        let root = d.assets();
        assert_eq!(
            read_asset(&root, "link.png", DEFAULT_MAX_ASSET_BYTES),
            Err("outside-folder")
        );
    }

    #[test]
    fn resolve_relative_only_reads_under_assets() {
        let d = AssetsDir::new();
        let root = d.assets();
        assert!(resolve_relative("assets/a.svg", Some(&root)).is_some());
        assert!(resolve_relative("assets/sub/b.png", Some(&root)).is_some());
        // Not under assets/: never resolved, even if the file exists.
        assert!(resolve_relative("other/a.svg", Some(&root)).is_none());
        // No root at all (no `assets/` folder next to the dashboard).
        assert!(resolve_relative("assets/a.svg", None).is_none());
    }

    /// Review of #554 nit 6: `stat_asset` (HEAD) must agree with
    /// `read_asset` (GET) — same MIME, and the real file length, without
    /// reading the whole file.
    #[test]
    fn stat_asset_matches_read_asset() {
        let d = AssetsDir::new();
        let root = d.assets();
        let (mime, bytes) = read_asset(&root, "a.svg", DEFAULT_MAX_ASSET_BYTES).unwrap();
        let (stat_mime, len) = stat_asset(&root, "a.svg", DEFAULT_MAX_ASSET_BYTES).unwrap();
        assert_eq!(stat_mime, mime);
        assert_eq!(len, bytes.len() as u64);

        let (mime, bytes) = read_asset(&root, "sub/b.png", DEFAULT_MAX_ASSET_BYTES).unwrap();
        let (stat_mime, len) = stat_asset(&root, "sub/b.png", DEFAULT_MAX_ASSET_BYTES).unwrap();
        assert_eq!(stat_mime, mime);
        assert_eq!(len, bytes.len() as u64);

        // Same refusals either way.
        assert_eq!(
            stat_asset(&root, "fake.png", DEFAULT_MAX_ASSET_BYTES),
            Err("not-an-image")
        );
        assert_eq!(stat_asset(&root, "a.svg", 4), Err("too-large"));
        assert_eq!(
            stat_asset(&root, "../policy.json", DEFAULT_MAX_ASSET_BYTES),
            Err("outside-folder")
        );
    }

    /// Review of #554 nit 8: a symlinked `assets/` folder itself (not a
    /// symlink *inside* an otherwise plain one, already covered above)
    /// must never resolve — `canonicalize` would otherwise follow it to
    /// its target and serve whatever is there.
    #[test]
    #[cfg(unix)]
    fn canonical_assets_dir_refuses_a_symlinked_assets_folder() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "anofox-image-symlink-test-{}-{nanos}",
            std::process::id()
        ));
        let elsewhere = std::env::temp_dir().join(format!(
            "anofox-image-symlink-target-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("leak.png"), b"\x89PNG\r\n\x1a\n").unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.join("assets")).unwrap();
        assert!(assets_dir_is_symlink(&dir));
        assert!(canonical_assets_dir(&dir).is_none());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&elsewhere);
    }
}
