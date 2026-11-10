//! #1076: a family-aware fuzz. `robustness.rs` throws random role soup at the
//! renderer, which rarely forms a real chart — a flow map with place names, a
//! box from statistics with a target — so its deep paths stayed untested (the
//! #1055 crash on "Lübeck" was found by a model, not by it). Here every case
//! is a valid template of one chart family, the way a model writes it, filled
//! with awkward data: NULLs, umlauts / emoji / CJK, empty strings, a single
//! row, duplicates, long labels, zero / negative / huge numbers, unknown
//! codes. Every result must be a well-formed, script-free SVG or a non-panic
//! error. `FUZZ_CASES` / `FUZZ_SEED` widen it locally.

mod common;

use anofox_visualization::{render_spec, render_spec_checked, RenderError};
use common::{assert_safe_pre, assert_safe_svg};
use serde_json::{json, Value};

/// xorshift64* — deterministic, dependency-free.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
}

/// What a template column holds.
#[derive(Clone, Copy)]
enum Ty {
    Text,
    Num,
    Date,
    Place,
    Code,
    List,
    Lat,
    Lon,
    /// A constant directive value (`'lm'::SMOOTH_METHOD`, `1::FLIP`).
    Lit(&'static str),
}
use Ty::*;

/// One chart family as a model writes it: `(role token, column type)`.
const FAMILIES: &[&[(&str, Ty)]] = &[
    &[("XAXIS", Text), ("BARCHART", Num)],
    &[
        ("XAXIS", Text),
        ("BARCHART_STACKED", Num),
        ("CATEGORY", Text),
    ],
    &[
        ("XAXIS", Text),
        ("BARCHART", Num),
        ("BARCHART", Num),
        ("FLIP", Lit("1")),
    ],
    &[("XAXIS", Num), ("BARCHART", Num), ("CATEGORY", Text)],
    &[("XAXIS", Date), ("LINECHART", Num), ("CATEGORY", Text)],
    &[
        ("XAXIS", Date),
        ("LINECHART", Num),
        ("SMOOTH", Num),
        ("SMOOTH_METHOD", Lit("lm")),
    ],
    &[
        ("XAXIS", Date),
        ("LINECHART", Num),
        ("BAND_LOWER", Num),
        ("BAND_UPPER", Num),
    ],
    &[
        ("XAXIS", Date),
        ("BARCHART", Num),
        ("LINECHART", Num),
        ("Y2", Lit("n2")),
        ("Y2FORMAT", Lit("percent")),
    ],
    &[
        ("XAXIS", Date),
        ("AREACHART_STACKED", Num),
        ("CATEGORY", Text),
    ],
    &[
        ("XAXIS", Num),
        ("SCATTER", Num),
        ("SIZE", Num),
        ("LABEL", Text),
        ("CATEGORY", Text),
    ],
    &[("XAXIS", Text), ("BOXPLOT", Num), ("FLIP", Lit("1"))],
    &[
        ("XAXIS", Text),
        ("Q1", Num),
        ("MEDIAN", Num),
        ("Q3", Num),
        ("WHISKER_LOW", Num),
        ("WHISKER_HIGH", Num),
        ("REFLINE", Num),
    ],
    &[("XAXIS", Text), ("VIOLIN", Num)],
    &[("HISTOGRAM", Num), ("CATEGORY", Text)],
    &[("DENSITY", Num)],
    &[("RIDGELINE", Num), ("CATEGORY", Text)],
    &[("QQ", Num)],
    &[
        ("XAXIS", Text),
        ("YAXIS", Text),
        ("HEATMAP", Num),
        ("DATALABELS", Lit("")),
    ],
    &[("XAXIS", Date), ("CALENDAR", Num)],
    &[("XAXIS", Text), ("PIE", Num)],
    &[("XAXIS", Text), ("DONUTCHART", Num)],
    &[("GAUGE", Num), ("RANGE", Lit("0,100"))],
    &[("XAXIS", Text), ("RADAR", Num), ("CATEGORY", Text)],
    &[("XAXIS", Text), ("WATERFALL", Num)],
    &[("XAXIS", Text), ("BULLET", Num), ("REFLINE", Num)],
    &[("XAXIS", Text), ("FUNNEL", Num)],
    &[("ORIGIN", Text), ("DEST", Text), ("SANKEY", Num)],
    &[
        ("XAXIS", Text),
        ("TREEMAP", Num),
        ("LABEL", Text),
        ("CATEGORY", Text),
    ],
    &[("XAXIS", Text), ("PARETO", Num)],
    &[
        ("XAXIS", Text),
        ("XMIN", Date),
        ("XMAX", Date),
        ("GANTT", Text),
    ],
    &[("XMIN", Num), ("XMAX", Num), ("DUMBBELL", Text)],
    &[("REGION", Code), ("BARCHART", Num), ("LABEL", Text)],
    &[("LAT", Lat), ("LON", Lon), ("LABEL", Text), ("SIZE", Num)],
    &[
        ("ORIGIN", Place),
        ("DEST", Place),
        ("SIZE", Num),
        ("CATEGORY", Text),
    ],
    &[
        ("METRIC", Num),
        ("LABEL", Text),
        ("DELTA", Num),
        ("SPARKLINE", List),
    ],
    &[("MONEY", Num), ("LABEL", Text), ("REFLINE", Num)],
    &[
        ("TABLE", Text),
        ("MONEY", Num),
        ("PERCENT", Num),
        ("TREND", Num),
        ("COLORSCALE", Num),
        ("SPARKLINE", List),
    ],
];

const TEXT: &[&str] = &[
    "North", "South", "West", "Lübeck", "Göttingen", "Fürth", "Ærø", "日本", "📦 Box",
    "a very long label that a model might write when it names a category after a whole sentence of context",
    "", " ", "O'Brien", "<b>bold</b>", "&amp;", "Nord-Süd", "東京", "x",
];
const PLACES: &[&str] = &[
    "Lübeck",
    "Göttingen",
    "Fürth",
    "München",
    "Köln",
    "Hamburg",
    "Düsseldorf",
    "Nowhere-🙃",
    "DE-BY",
    "09162",
    "POINT(10.7 53.9)",
    "",
];
const CODES: &[&str] = &[
    "DE-BY", "DE-BE", "DE", "DEU", "09162", "DE212", "AT-1", "CH-ZG", "de-by", "XX-99", "", "FR",
];

fn num(r: &mut Rng, k: usize) -> Value {
    match r.below(14) {
        0 => Value::Null,
        1 => json!(0),
        2 => json!(-((k * 13 % 97) as f64) - 0.5),
        3 => json!(1.0e12 + k as f64),
        4 => json!(1.0e-9 * k as f64),
        5 => json!(42), // duplicates
        _ => json!(((k * 37 + 11) % 503) as f64 / 3.0),
    }
}

fn cell(r: &mut Rng, ty: Ty, k: usize) -> Value {
    if r.chance(12) {
        return Value::Null;
    }
    match ty {
        Text => json!(r.pick(TEXT)),
        Num => num(r, k),
        Date => {
            if r.chance(20) {
                json!("not a date")
            } else {
                let d = (k * 3 + r.below(3)) % 28 + 1;
                json!(format!("2025-{:02}-{d:02}", (k / 28) % 12 + 1))
            }
        }
        Place => json!(r.pick(PLACES)),
        Code => json!(r.pick(CODES)),
        List => match r.below(4) {
            0 => json!("[]"),
            1 => json!([1.0, Value::Null, 3.5]),
            _ => json!((0..1 + r.below(12))
                .map(|i| (i * 7 % 11) as f64)
                .collect::<Vec<_>>()),
        },
        Lat => json!(47.0 + (k % 8) as f64 * 0.9),
        Lon => json!(6.0 + (k % 9) as f64 * 1.1),
        Lit(v) => json!(v),
    }
}

fn spec(r: &mut Rng) -> (String, String) {
    let fam = *r.pick(FAMILIES);
    let nrows = match r.below(8) {
        0 => 0,
        1 => 1,
        2 => 2,
        7 => 200 + r.below(800),
        _ => 3 + r.below(40),
    };
    let rows: Vec<Value> = (0..nrows)
        .map(|k| {
            let mut m = serde_json::Map::new();
            for (i, (_, ty)) in fam.iter().enumerate() {
                m.insert(format!("c{i}"), cell(r, *ty, k));
            }
            Value::Object(m)
        })
        .collect();
    let roles: Vec<Value> = fam
        .iter()
        .enumerate()
        .map(|(i, (tok, _))| json!([i, tok, format!("n{i}")]))
        .collect();
    let mut s = json!({ "rows": rows, "roles": roles, "width": 480, "height": 320 });
    // a locale or a dark theme now and then
    if r.chance(4) {
        s["locale"] = json!(r.pick(&["de-DE", "fr-FR", "en-GB"]));
    }
    if r.chance(6) {
        s["theme"] = json!("dark");
    }
    let name = fam.iter().map(|(t, _)| *t).collect::<Vec<_>>().join("+");
    (s.to_string(), name)
}

#[test]
fn every_chart_family_survives_awkward_data() {
    let env = |k: &str, d: u64| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let mut r = Rng(env("FUZZ_SEED", 0x1076_f00d_5eed_0001) | 1);
    let mut fails = Vec::new();
    for i in 0..env("FUZZ_CASES", 600) {
        let (spec, family) = spec(&mut r);
        let ctx = format!("case {i} ({family})");
        let res = std::panic::catch_unwind(|| match render_spec_checked(&spec) {
            Ok(svg) => assert_safe_svg(&svg, &ctx),
            Err(RenderError::Panic(m)) => panic!("PANIC {m}"),
            Err(e) => {
                assert!(!e.to_string().is_empty(), "empty error");
                assert_safe_pre(&render_spec(&spec), &ctx);
            }
        });
        if let Err(p) = res {
            let m = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            fails.push(format!("{ctx}: {m}\n{}", common::head(&spec)));
        }
    }
    assert!(
        fails.is_empty(),
        "{} failing case(s):\n{}",
        fails.len(),
        fails
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n---\n")
    );
}

#[test]
fn every_family_has_a_registered_role() {
    let known: std::collections::HashSet<&str> = anofox_visualization::roles::REGISTRY
        .iter()
        .map(|s| s.token)
        .collect();
    for fam in FAMILIES {
        for (tok, _) in *fam {
            assert!(known.contains(tok), "::{tok} is not a role");
        }
    }
}
