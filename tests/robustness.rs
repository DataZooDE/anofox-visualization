//! Panic-freedom on user input: a seeded fuzz loop over random JSON specs
//! (NaN/±inf, empty, one row, huge values, unicode, quotes, markup, odd sizes,
//! every role) plus targeted regressions. Every result must be `Ok(svg)` with
//! well-formed, script-free SVG or a non-panic `Err`, within a time bound.

mod common;

use anofox_visualization::{
    render_spec, render_spec_checked, render_with, roles, Column, Kind, RenderError, RenderOptions,
    Role, Value,
};
use common::{assert_safe_pre, assert_safe_svg};
use std::time::{Duration, Instant};

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
}

const STRINGS: &[&str] = &[
    "a",
    "b",
    "Ünïcødé ✓ 日本",
    "\"quoted\"",
    "it's",
    "<script>alert(1)</script>",
    "\"><svg onload=alert(1)>",
    "&amp; & <b>",
    "",
    " ",
    "W1",
    "W2",
    "W3",
    "#e03131",
    "#zzzzzz",
    "0,100",
    "0;100",
    "100",
    "50,50",
    "nan",
    "€",
    "$",
    " kg",
    "percent",
    "2024-01-01",
    "2024-02-31",
    "2024-03-01 12:00:00+02",
    "2024-03-01T12:00:00.123Z",
    "1970-01-01",
    "9999-12-31",
    "POINT(1 2)",
    "POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))",
    "POLYGON((0 0, 1 0",
    "LINESTRING(0 0, 5 5)",
    "low,ok,high",
    "\u{0}\u{7}ctrl",
    "[1,2,3]",
];

/// A random JSON cell (raw JSON text, so bare NaN/Infinity can be emitted).
fn cell(r: &mut Rng) -> String {
    match r.below(16) {
        0 => "NaN".into(),
        1 => "Infinity".into(),
        2 => "-Infinity".into(),
        3 => "null".into(),
        4 => "1e300".into(),
        5 => "-1e300".into(),
        6 => "1e13".into(),
        7 => "true".into(),
        8 => "[1, 2.5, NaN]".into(),
        9 | 10 => serde_json::to_string(r.pick(STRINGS)).unwrap(),
        11 => format!("\"{}\"", r.below(1000)),
        12 => "\"inf\"".into(),
        _ => format!("{}", (r.next() % 2001) as f64 / 10.0 - 100.0),
    }
}

fn random_spec(r: &mut Rng) -> String {
    let tokens: Vec<&str> = roles::REGISTRY.iter().map(|s| s.token).collect();
    let charts: Vec<&str> = roles::REGISTRY
        .iter()
        .filter(|s| s.category == "chart")
        .map(|s| s.token)
        .collect();
    let ncols = 1 + r.below(6);
    let mut roles_json = Vec::new();
    // Usually one chart measure + a few others.
    for i in 0..ncols {
        let tok = if i == 1 && r.below(4) != 0 {
            *r.pick(&charts)
        } else if i == 0 && r.below(3) != 0 {
            *r.pick(&["XAXIS", "CATEGORY", "LABEL"])
        } else {
            *r.pick(&tokens)
        };
        roles_json.push(format!("[{i}, \"{tok}\", \"n{i}\"]"));
    }
    let nrows = match r.below(10) {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        9 => 300 + r.below(1500),
        _ => r.below(60),
    };
    // Columns may be homogeneous (realistic) or mixed (adversarial).
    let homogeneous: Vec<Option<String>> = (0..ncols)
        .map(|_| (r.below(2) == 0).then(String::new))
        .collect();
    let mut rows = Vec::with_capacity(nrows);
    for k in 0..nrows {
        let mut fields = Vec::new();
        for (i, h) in homogeneous.iter().enumerate() {
            if r.below(25) == 0 {
                continue; // a missing key
            }
            let v = if h.is_some() {
                if i == 0 {
                    format!("\"{}\"", ["a", "b", "c", "d"][k % 4])
                } else {
                    format!("{}", (k * 7 % 23) as f64 + 0.5)
                }
            } else {
                cell(r)
            };
            fields.push(format!("\"c{i}\": {v}"));
        }
        rows.push(format!("{{{}}}", fields.join(", ")));
    }
    let dim = |r: &mut Rng| -> String {
        r.pick(&["0", "1", "-5", "1.5", "640", "300", "1e9", "\"x\"", "NaN"])
            .to_string()
    };
    let (w, h) = (dim(r), dim(r));
    format!(
        "{{\"rows\": [{}], \"roles\": [{}], \"width\": {w}, \"height\": {h}, \"primary\": \"{}\"}}",
        rows.join(", "),
        roles_json.join(", "),
        r.pick(&["", "e8335d", "#123456", "nothex", "\\\"><x"])
    )
}

fn check(spec: &str, ctx: &str) {
    let t = Instant::now();
    let res = render_spec_checked(spec);
    let dt = t.elapsed();
    assert!(
        dt < Duration::from_secs(20),
        "{ctx}: took {dt:?}\n{}",
        common::head(spec)
    );
    match res {
        Ok(svg) => assert_safe_svg(&svg, ctx),
        Err(RenderError::Panic(m)) => panic!("{ctx}: PANIC {m}\n{}", common::head(spec)),
        Err(e) => {
            assert!(!e.to_string().is_empty(), "{ctx}: empty error");
            assert_safe_pre(&render_spec(spec), ctx);
        }
    }
}

/// Run [`check`], returning the failure message instead of panicking, so a
/// test can report every failing case at once.
fn try_check(spec: &str, ctx: &str) -> Option<String> {
    std::panic::catch_unwind(|| check(spec, ctx))
        .err()
        .map(|p| {
            p.downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default()
        })
}

fn assert_all_ok(failures: Vec<String>) {
    assert!(
        failures.is_empty(),
        "{} failing case(s):\n{}",
        failures.len(),
        failures
            .iter()
            .take(25)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n---\n")
    );
}

#[test]
fn fuzz_random_specs_never_panic() {
    // Silence the default hook so a caught panic is reported by the assertion
    // above (with the spec) rather than as noise.
    // FUZZ_CASES / FUZZ_SEED widen the search locally (CI runs the default).
    let env = |k: &str, d: u64| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let mut r = Rng(env("FUZZ_SEED", 0x5eed_1234_abcd_0001) | 1);
    let mut failures = Vec::new();
    for i in 0..env("FUZZ_CASES", 400) {
        let spec = random_spec(&mut r);
        failures.extend(try_check(&spec, &format!("case {i}")));
    }
    assert_all_ok(failures);
}

#[test]
fn every_chart_kind_survives_degenerate_inputs() {
    let degenerate: &[(&str, &str)] = &[
        ("empty", "[]"),
        ("one row", r#"[{"c0":"a","c1":5,"c2":"x"}]"#),
        (
            "all null",
            r#"[{"c0":null,"c1":null},{"c0":null,"c1":null}]"#,
        ),
        ("NaN", r#"[{"c0":"a","c1":NaN},{"c0":"b","c1":Infinity}]"#),
        (
            "NaN strings",
            r#"[{"c0":"a","c1":"NaN"},{"c0":"b","c1":"-inf"}]"#,
        ),
        (
            "huge",
            r#"[{"c0":1e300,"c1":-1e300},{"c0":-1e300,"c1":1e300}]"#,
        ),
        (
            "constant",
            r#"[{"c0":1,"c1":3},{"c0":1,"c1":3},{"c0":1,"c1":3}]"#,
        ),
        (
            "markup",
            r#"[{"c0":"<b>\"x\"</b>","c1":1,"c2":"'<i>"},{"c0":"é","c1":2,"c2":"&"}]"#,
        ),
    ];
    let mut failures = Vec::new();
    for spec in roles::REGISTRY.iter().filter(|s| s.category == "chart") {
        for (name, rows) in degenerate {
            for x in ["XAXIS", "CATEGORY"] {
                let json = format!(
                    r#"{{"rows":{rows},"roles":[[0,"{x}"],[1,"{}"],[2,"YAXIS"]],"width":300,"height":200}}"#,
                    spec.token
                );
                failures.extend(try_check(
                    &json,
                    &format!("::{} / {x} / {name}", spec.token),
                ));
            }
        }
    }
    assert_all_ok(failures);
}

#[test]
fn gauge_range_regressions() {
    // `(p.len()==2).then_some((p[0],p[1]))` indexed eagerly → OOB panic.
    for range in [
        "100",
        "null",
        "\"0;100\"",
        "\"\"",
        "\"5\"",
        "\"a,b\"",
        "\"1,2,3\"",
        "\"100,0\"",
    ] {
        let spec =
            format!(r#"{{"rows":[{{"c0":42,"c1":{range}}}],"roles":[[0,"GAUGE"],[1,"RANGE"]]}}"#);
        let svg = render_spec_checked(&spec).unwrap_or_else(|e| panic!("{range}: {e}"));
        assert_safe_svg(&svg, range);
    }
    // '0;100' is accepted as a range; a reversed range is swapped.
    let svg = render_spec_checked(
        r#"{"rows":[{"c0":42,"c1":"200;0"}],"roles":[[0,"GAUGE"],[1,"RANGE"]]}"#,
    )
    .unwrap();
    assert!(svg.contains("of 200"), "{svg}");
}

#[test]
fn gauge_zone_labels_render() {
    let svg = render_spec_checked(
        r##"{"rows":[{"c0":70,"c1":"#e03131,#efc94c,#0ca678","c2":"low,ok,high"}],
            "roles":[[0,"GAUGE"],[1,"COLORS"],[2,"LABELS"]]}"##,
    )
    .unwrap();
    for l in ["low", "ok", "high"] {
        assert!(svg.contains(&format!(">{l}</text>")), "zone label {l}");
    }
}

#[test]
fn calendar_span_is_capped_and_fast() {
    for (secs, ok) in [
        ("1e13", false),
        ("1e300", false),
        ("3.1536e9", false),
        ("8.64e6", true),
    ] {
        let spec = format!(
            r#"{{"rows":[{{"c0":0,"c1":1}},{{"c0":{secs},"c1":2}}],"roles":[[0,"XAXIS"],[1,"CALENDAR"]]}}"#
        );
        let t = Instant::now();
        let res = render_spec_checked(&spec);
        assert!(t.elapsed() < Duration::from_secs(2), "{secs}: too slow");
        match (res, ok) {
            (Ok(svg), true) => {
                assert!(svg.len() < 200_000, "{secs}: {} bytes", svg.len());
                assert_safe_svg(&svg, secs);
            }
            (Err(RenderError::Render(m)), false) => assert!(m.contains("years"), "{m}"),
            (r, _) => panic!("{secs}: unexpected {r:?}"),
        }
    }
}

#[test]
fn sizes_are_clamped() {
    let svg = render_spec_checked(
        r#"{"rows":[{"c0":"a","c1":1}],"roles":[[0,"XAXIS"],[1,"BARCHART"]],"width":4294967297,"height":1}"#,
    )
    .unwrap();
    assert!(svg.contains("width=\"8192\""), "{}", common::head(&svg));
    assert!(render_spec_checked(r#"{"rows":[],"roles":[],"width":"wide"}"#).is_err());
}

#[test]
fn empty_and_tiny_inputs_say_so() {
    let svg = render_spec_checked(r#"{"rows":[],"roles":[[0,"XAXIS"],[1,"BARCHART"]]}"#).unwrap();
    assert!(svg.contains("No data"), "{svg}");
    let svg = render_spec_checked(r#"{"rows":[{"c0":1}],"roles":[[0,"DENSITY"]]}"#).unwrap();
    assert!(svg.contains("needs at least 2 values"), "{svg}");
}

#[test]
fn bad_specs_are_errors_not_panics() {
    for s in [
        "",
        "{",
        "[]",
        "null",
        r#"{"rows":5}"#,
        r#"{"rows":[1,2]}"#,
        r#"{"roles":"x"}"#,
        r#"{"roles":[["a","XAXIS"]]}"#,
        r#"{"rows":[],"roles":[[0,"NOPE<script>"]]}"#,
    ] {
        match render_spec_checked(s) {
            Err(RenderError::BadSpec(_)) => {}
            other => panic!("{s}: expected BadSpec, got {other:?}"),
        }
        assert_safe_pre(&render_spec(s), s);
    }
}

#[test]
fn custom_renderers_draw_their_title() {
    let s = |v: &[&str]| {
        v.iter()
            .map(|x| Value::Str(x.to_string()))
            .collect::<Vec<_>>()
    };
    let f = |v: &[f64]| v.iter().map(|x| Value::Float(*x)).collect::<Vec<_>>();
    let title = || Column::new("t", Role::Label, s(&["My <title>"]));
    let cases: Vec<(&str, Vec<Column>)> = vec![
        (
            "radar",
            vec![
                Column::new("x", Role::X, s(&["a", "b", "c"])),
                Column::new("v", Role::Value(Kind::Radar), f(&[1.0, 2.0, 3.0])),
                title(),
            ],
        ),
        (
            "candlestick",
            vec![
                Column::new("x", Role::X, s(&["d1", "d2"])),
                Column::new("o", Role::Open, f(&[1.0, 2.0])),
                Column::new("h", Role::High, f(&[3.0, 4.0])),
                Column::new("l", Role::Low, f(&[0.5, 1.0])),
                Column::new("c", Role::Value(Kind::Candlestick), f(&[2.0, 1.5])),
                title(),
            ],
        ),
        (
            "calendar",
            vec![
                Column::new(
                    "x",
                    Role::X,
                    vec![Value::DateTime(0), Value::DateTime(86_400 * 40)],
                ),
                Column::new("v", Role::Value(Kind::Calendar), f(&[1.0, 2.0])),
                title(),
            ],
        ),
    ];
    for (name, cols) in cases {
        let svg = render_with(&cols, 400, 300, &RenderOptions::default()).unwrap();
        assert!(svg.contains("My &lt;title&gt;"), "{name}: title missing");
        assert_safe_svg(&svg, name);
    }
}

#[test]
fn many_categories_are_capped_and_fast() {
    // 50k distinct bars: quadratic dedup used to take minutes; now capped.
    let n = 50_000;
    let rows: Vec<String> = (0..n)
        .map(|i| format!(r#"{{"c0":"cat{i}","c1":{}}}"#, i % 97))
        .collect();
    let spec = format!(
        r#"{{"rows":[{}],"roles":[[0,"XAXIS"],[1,"BARCHART"]],"max_categories":12}}"#,
        rows.join(",")
    );
    let t = Instant::now();
    let svg = render_spec_checked(&spec).unwrap();
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    assert!(svg.contains("Other"), "fold bucket missing");
    assert_safe_svg(&svg, "many categories");
}

#[test]
fn long_lines_are_downsampled() {
    let n = 100_000;
    let x: Vec<Value> = (0..n).map(|i| Value::Float(i as f64)).collect();
    let y: Vec<Value> = (0..n)
        .map(|i| Value::Float((i as f64 / 300.0).sin()))
        .collect();
    let cols = vec![
        Column::new("x", Role::X, x),
        Column::new("y", Role::Value(Kind::Line), y),
    ];
    let t = Instant::now();
    let svg = render_with(&cols, 600, 300, &RenderOptions::default()).unwrap();
    assert!(t.elapsed() < Duration::from_secs(15), "{:?}", t.elapsed());
    // ≤ max_line_points markers (+ a little chrome), not 100k.
    assert!(svg.matches("<circle").count() <= 5_100);
}
