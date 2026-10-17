//! The built-in boundary pack: country, state and district geometries a map
//! looks up by code (`'DE-BY'::REGION`) or draws as a backdrop
//! (`'world'::BASEMAP`).
//!
//! Each layer is a CSV in `web/data/boundaries/` (`key,codes,name_en,
//! name_de,alt,wkt`, built by `scripts/build-boundaries.sh`; sources and
//! licences in `SOURCES.md`). Native builds embed the default layers; the
//! browser fetches a layer when a panel needs it and hands it over with
//! [`register`] (the wasm export `register_boundaries`), so the pack is never
//! part of the wasm itself.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use super::{shapes, Shape};
use ggplot_rs::spatial::parse_wkt;

/// The default layers (embedded in native builds).
pub(crate) const DEFAULT_LAYERS: &[&str] = &["world", "de-states", "de-districts"];
/// Every layer name a map accepts: the pack's (`world-50m` is optional: in
/// `web/data` for the browser, not embedded) and the EU NUTS levels a user
/// fetches for themselves (`scripts/fetch-nuts.sh`; never shipped).
pub(crate) const LAYERS: &[&str] = &[
    "world",
    "world-50m",
    "de-states",
    "de-districts",
    "nuts0",
    "nuts1",
    "nuts2",
    "nuts3",
];

/// The year of the last download from the BKG (its source note needs it).
pub(crate) const BKG_YEAR: u32 = 2026;

/// One region of a layer.
pub(crate) struct Region {
    pub key: String,
    /// The key and the other codes, upper-case.
    pub codes: Vec<String>,
    pub name_en: String,
    pub name_de: String,
    /// Other names (long and formal forms, "München (Landkreis)").
    pub alt: Vec<String>,
    /// The geometry in lon/lat.
    pub shapes: Vec<Shape>,
}

impl Region {
    /// The name in the dashboard locale (German for `de*`, else English).
    pub(crate) fn name(&self, german: bool) -> &str {
        if german && !self.name_de.is_empty() {
            &self.name_de
        } else {
            &self.name_en
        }
    }
}

/// A parsed layer with its code and name indexes.
pub(crate) struct Layer {
    pub name: String,
    pub regions: Vec<Region>,
    by_code: HashMap<String, usize>,
    by_name: HashMap<String, Vec<usize>>,
}

impl Layer {
    /// Parse a layer CSV.
    pub(crate) fn parse(name: &str, csv: &str) -> Result<Layer, String> {
        let mut rows = csv_rows(csv);
        let head = rows.next().ok_or("empty boundary file")?;
        let col = |n: &str| {
            head.iter()
                .position(|h| h == n)
                .ok_or_else(|| format!("boundary file without a `{n}` column"))
        };
        let (ik, ic, ien, ide, ia, iw) = (
            col("key")?,
            col("codes")?,
            col("name_en")?,
            col("name_de")?,
            col("alt")?,
            col("wkt")?,
        );
        let mut regions = Vec::new();
        for r in rows {
            let get = |i: usize| r.get(i).map(String::as_str).unwrap_or("");
            let key = get(ik).trim().to_string();
            if key.is_empty() {
                continue;
            }
            let mut out = Vec::new();
            if let Some(g) = parse_wkt(get(iw)) {
                shapes(&g, &mut out);
            }
            let mut codes = vec![key.to_uppercase()];
            for c in get(ic).split('|').map(|c| c.trim().to_uppercase()) {
                if !c.is_empty() && !codes.contains(&c) {
                    codes.push(c);
                }
            }
            regions.push(Region {
                key,
                codes,
                name_en: get(ien).trim().to_string(),
                name_de: get(ide).trim().to_string(),
                alt: get(ia)
                    .split('|')
                    .map(|a| a.trim().to_string())
                    .filter(|a| !a.is_empty())
                    .collect(),
                shapes: out,
            });
        }
        let mut by_code = HashMap::new();
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, r) in regions.iter().enumerate() {
            for c in &r.codes {
                by_code.entry(c.clone()).or_insert(i);
            }
            let mut names: Vec<String> = [&r.name_en, &r.name_de]
                .into_iter()
                .chain(&r.alt)
                .map(|n| fold(n))
                .filter(|n| !n.is_empty())
                .collect();
            names.sort();
            names.dedup();
            for n in names {
                by_name.entry(n).or_default().push(i);
            }
        }
        Ok(Layer {
            name: name.to_string(),
            regions,
            by_code,
            by_name,
        })
    }

    /// The regions a code names: one by its code, or — a German NUTS 2
    /// code (`DE21`) on the districts — every district inside it.
    pub(crate) fn by_code(&self, code: &str) -> Vec<usize> {
        let c = code.trim().to_uppercase();
        if c.is_empty() {
            return Vec::new();
        }
        if let Some(i) = self.by_code.get(&c) {
            return vec![*i];
        }
        // a code read as a number lost its leading zero (9162 → 09162)
        if matches!(c.len(), 1 | 4) && c.bytes().all(|b| b.is_ascii_digit()) {
            if let Some(i) = self.by_code.get(&format!("0{c}")) {
                return vec![*i];
            }
        }
        if self.name == "de-districts" && c.len() == 4 && c.starts_with("DE") {
            return (0..self.regions.len())
                .filter(|i| {
                    self.regions[*i]
                        .codes
                        .iter()
                        .any(|k| k.len() == 5 && k.starts_with(&c))
                })
                .collect();
        }
        Vec::new()
    }

    /// The regions a name names (case and accents folded; an ambiguous name
    /// — two districts called "München" — names the first).
    pub(crate) fn by_name(&self, name: &str) -> Vec<usize> {
        self.by_name
            .get(&fold(name))
            .and_then(|v| v.first())
            .map(|i| vec![*i])
            .unwrap_or_default()
    }
}

/// Lower-case, trimmed, with German umlauts spelled out (`Köln` = `koeln`).
fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.trim().chars().flat_map(char::to_lowercase) {
        match c {
            'ä' => out.push_str("ae"),
            'ö' => out.push_str("oe"),
            'ü' => out.push_str("ue"),
            'ß' => out.push_str("ss"),
            c => out.push(c),
        }
    }
    out
}

/// Records of a CSV (RFC 4180: `"` quotes, `""` escapes, newlines inside
/// quotes).
fn csv_rows(s: &str) -> impl Iterator<Item = Vec<String>> + '_ {
    let mut chars = s.chars().peekable();
    std::iter::from_fn(move || {
        chars.peek()?;
        let mut row = Vec::new();
        let mut field = String::new();
        let mut quoted = false;
        while let Some(c) = chars.next() {
            match (c, quoted) {
                ('"', true) if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                ('"', _) => quoted = !quoted,
                (',', false) => row.push(std::mem::take(&mut field)),
                ('\n', false) => break,
                ('\r', false) => {}
                (c, _) => field.push(c),
            }
        }
        row.push(field);
        Some(row)
    })
}

fn registry() -> &'static RwLock<HashMap<String, Arc<Layer>>> {
    static R: OnceLock<RwLock<HashMap<String, Arc<Layer>>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register a layer from its CSV text (the browser, after fetching it);
/// returns its region count. Replaces a layer of the same name.
#[cfg_attr(not(feature = "wasm"), allow(dead_code))]
pub(crate) fn register(name: &str, csv: &str) -> Result<usize, String> {
    if !LAYERS.contains(&name) {
        return Err(format!("unknown boundary layer `{name}`"));
    }
    let layer = Layer::parse(name, csv)?;
    let n = layer.regions.len();
    if let Ok(mut r) = registry().write() {
        r.insert(name.to_string(), Arc::new(layer));
    }
    Ok(n)
}

/// The embedded CSV of a default layer (native builds only).
#[cfg(not(target_arch = "wasm32"))]
fn embedded(name: &str) -> Option<&'static str> {
    Some(match name {
        "world" => include_str!("../../web/data/boundaries/world.csv"),
        "de-states" => include_str!("../../web/data/boundaries/de-states.csv"),
        "de-districts" => include_str!("../../web/data/boundaries/de-districts.csv"),
        _ => return None,
    })
}

#[cfg(target_arch = "wasm32")]
fn embedded(_: &str) -> Option<&'static str> {
    None
}

/// A layer: registered, else (native) parsed once from the embedded copy.
pub(crate) fn layer(name: &str) -> Option<Arc<Layer>> {
    if let Some(l) = registry().read().ok()?.get(name) {
        return Some(l.clone());
    }
    let csv = embedded(name)?;
    let l = Arc::new(Layer::parse(name, csv).ok()?);
    if let Ok(mut r) = registry().write() {
        r.entry(name.to_string()).or_insert_with(|| l.clone());
    }
    Some(l)
}

/// Whether a layer can be drawn without a fetch.
#[cfg_attr(not(feature = "wasm"), allow(dead_code))]
pub(crate) fn available(name: &str) -> bool {
    registry()
        .read()
        .map(|r| r.contains_key(name))
        .unwrap_or(false)
        || embedded(name).is_some()
}

/// How a region column was matched.
pub(crate) struct Matched {
    pub layer: Arc<Layer>,
    /// The regions of each row (empty: no match).
    pub rows: Vec<Vec<usize>>,
    /// Up to five codes that matched nothing.
    pub unmatched: Vec<String>,
    pub unmatched_count: usize,
    /// Matched by name, not by code.
    pub by_name: bool,
}

/// Match region codes against the pack: the `forced` layer (`::BOUNDARIES`),
/// else the default layer with the most code matches; by name only when no
/// code matches anywhere.
pub(crate) fn match_codes(codes: &[Option<String>], forced: Option<&str>) -> Option<Matched> {
    let names: Vec<&str> = match forced {
        Some(f) => vec![f],
        None => DEFAULT_LAYERS.to_vec(),
    };
    let layers: Vec<Arc<Layer>> = names.iter().filter_map(|n| layer(n)).collect();
    let score = |l: &Layer, by_name: bool| -> (usize, Vec<Vec<usize>>) {
        let rows: Vec<Vec<usize>> = codes
            .iter()
            .map(|c| match c {
                Some(c) if by_name => l.by_name(c),
                Some(c) => l.by_code(c),
                None => Vec::new(),
            })
            .collect();
        (rows.iter().filter(|r| !r.is_empty()).count(), rows)
    };
    // (matches, layer, regions per row, by name)
    type Best = (usize, Arc<Layer>, Vec<Vec<usize>>, bool);
    let mut best: Option<Best> = None;
    for by_name in [false, true] {
        for l in &layers {
            let (n, rows) = score(l, by_name);
            if n > 0 && best.as_ref().is_none_or(|b| n > b.0) {
                best = Some((n, l.clone(), rows, by_name));
            }
        }
        if best.is_some() {
            break;
        }
    }
    let (_, layer, rows, by_name) = match best {
        Some(b) => b,
        None => {
            let l = layers.first()?.clone();
            (0, l, vec![Vec::new(); codes.len()], false)
        }
    };
    let mut unmatched = Vec::new();
    let mut unmatched_count = 0;
    for (c, r) in codes.iter().zip(&rows) {
        if let (Some(c), true) = (c, r.is_empty()) {
            unmatched_count += 1;
            if unmatched.len() < 5 && !unmatched.contains(c) {
                unmatched.push(c.clone());
            }
        }
    }
    Some(Matched {
        layer,
        rows,
        unmatched,
        unmatched_count,
        by_name,
    })
}

/// The layers a panel may need before it can draw, guessed from the shape
/// of its region codes (the browser fetches them first): 2/3 letters →
/// `world`; `DE-XX`, a 2-digit AGS or `DE` + 1 → `de-states`; a 5-digit AGS
/// or `DE` + 2–3 → `de-districts`; names → every default layer.
#[cfg_attr(not(feature = "wasm"), allow(dead_code))]
pub(crate) fn guess_layers(codes: &[Option<String>]) -> Vec<&'static str> {
    let mut want: Vec<&'static str> = Vec::new();
    fn add(want: &mut Vec<&'static str>, l: &'static str) {
        if !want.contains(&l) {
            want.push(l);
        }
    }
    for c in codes.iter().flatten() {
        let c = c.trim().to_uppercase();
        let alpha = c.chars().all(|ch| ch.is_ascii_alphabetic());
        let digits = !c.is_empty() && c.chars().all(|ch| ch.is_ascii_digit());
        if (c.starts_with("DE-") && c.len() == 5) || (digits && c.len() <= 2) {
            add(&mut want, "de-states");
        } else if digits && (c.len() == 4 || c.len() == 5) {
            add(&mut want, "de-districts");
        } else if c.len() == 3
            && c.starts_with("DE")
            && matches!(c.as_bytes()[2], b'1'..=b'9' | b'A'..=b'G')
        {
            add(&mut want, "de-states");
        } else if c.starts_with("DE") && (c.len() == 4 || c.len() == 5) && c.is_ascii() {
            add(&mut want, "de-districts");
        } else if alpha && (c.len() == 2 || c.len() == 3) {
            add(&mut want, "world");
        } else {
            for l in DEFAULT_LAYERS {
                add(&mut want, l);
            }
        }
        if want.len() == DEFAULT_LAYERS.len() {
            break;
        }
    }
    want
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_and_escapes() {
        let rows: Vec<Vec<String>> = csv_rows("a,b\n\"x, y\",\"say \"\"hi\"\"\"\n1,\n").collect();
        assert_eq!(rows[1], vec!["x, y".to_string(), "say \"hi\"".to_string()]);
        assert_eq!(rows[2], vec!["1".to_string(), String::new()]);
    }

    /// The pack's integrity: every layer parses, keys are unique, the
    /// expected counts, every region has geometry, and the codes the docs
    /// promise resolve.
    #[test]
    fn the_pack_is_complete() {
        let w = layer("world").unwrap();
        assert_eq!(w.regions.len(), 177);
        let s = layer("de-states").unwrap();
        assert_eq!(s.regions.len(), 16);
        let d = layer("de-districts").unwrap();
        assert_eq!(d.regions.len(), 400);
        for l in [&w, &s, &d] {
            let mut keys: Vec<&str> = l.regions.iter().map(|r| r.key.as_str()).collect();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), l.regions.len(), "{}: unique keys", l.name);
            assert!(l.regions.iter().all(|r| !r.shapes.is_empty()), "{}", l.name);
        }
        let name = |l: &Layer, c: &str| l.by_code(c).first().map(|i| l.regions[*i].name_en.clone());
        assert_eq!(name(&w, "DEU").as_deref(), Some("Germany"));
        assert_eq!(name(&w, "de").as_deref(), Some("Germany"));
        assert_eq!(name(&w, "FR").as_deref(), Some("France"));
        assert_eq!(name(&w, "NO").as_deref(), Some("Norway"));
        assert_eq!(name(&s, "DE-BY").as_deref(), Some("Bavaria"));
        assert_eq!(name(&s, "09").as_deref(), Some("Bavaria"));
        assert_eq!(name(&s, "DE2").as_deref(), Some("Bavaria"));
        assert_eq!(name(&d, "09162").as_deref(), Some("München"));
        assert_eq!(name(&d, "DE212").as_deref(), Some("München"));
        // a NUTS 2 code is the union of its districts (Oberbayern: 23)
        assert_eq!(d.by_code("DE21").len(), 23);
        // names, folded
        assert_eq!(s.by_name("bayern"), s.by_code("DE-BY"));
        assert_eq!(d.by_name("Koeln").len(), 1);
        assert_eq!(w.by_name("Deutschland"), w.by_code("DEU"));
    }

    #[test]
    fn codes_pick_their_layer() {
        let c = |v: &[&str]| v.iter().map(|s| Some(s.to_string())).collect::<Vec<_>>();
        let m = match_codes(&c(&["DE-BY", "DE-BE", "XX"]), None).unwrap();
        assert_eq!(m.layer.name, "de-states");
        assert_eq!(m.unmatched, vec!["XX".to_string()]);
        assert_eq!(
            match_codes(&c(&["09162", "11000"]), None)
                .unwrap()
                .layer
                .name,
            "de-districts"
        );
        assert_eq!(
            match_codes(&c(&["DEU", "FRA", "ITA"]), None)
                .unwrap()
                .layer
                .name,
            "world"
        );
        let m = match_codes(&c(&["Bayern", "Hessen"]), None).unwrap();
        assert!(m.by_name && m.layer.name == "de-states");
        // ::BOUNDARIES forces the layer
        let m = match_codes(&c(&["Berlin"]), Some("de-districts")).unwrap();
        assert_eq!(m.layer.name, "de-districts");
        assert_eq!(guess_layers(&c(&["DE-BY"])), vec!["de-states"]);
        assert_eq!(
            guess_layers(&c(&["09162", "DE"])),
            vec!["de-districts", "world"]
        );
        assert_eq!(guess_layers(&c(&["Bayern"])), DEFAULT_LAYERS.to_vec());
    }
}
