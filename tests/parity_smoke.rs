use duckplot::{render, Column, Kind, Role, TextSize};
use ggplot_rs::prelude::Value;

fn f(v: &[f64]) -> Vec<Value> { v.iter().map(|x| Value::Float(*x)).collect() }
fn s(v: &[&str]) -> Vec<Value> { v.iter().map(|x| Value::Str((*x).into())).collect() }

fn case(name: &str, cols: Vec<Column>) {
    let r = std::panic::catch_unwind(|| render(&cols, 460, 300));
    match r {
        Ok(Ok(svg)) => assert!(svg.contains("<svg"), "{name}: not an svg"),
        Ok(Err(e)) => panic!("{name} render error: {e}"),
        Err(_) => panic!("{name} PANICKED"),
    }
}

#[test]
fn smoke() {
    case("gauge", vec![
        Column::new("v", Role::Value(Kind::Gauge), f(&[96.0])),
        Column::new("rng", Role::Range, s(&["0,120"])),
        Column::new("col", Role::GaugeColors, s(&["#e03131,#efc94c,#0ca678"])),
        Column::new("t", Role::Title, s(&["Sessions"])),
    ]);
    case("donut", vec![
        Column::new("cat", Role::Category, s(&["app","web","api"])),
        Column::new("v", Role::Value(Kind::Donut), f(&[10.0,20.0,30.0])),
    ]);
    case("stacked_pct", vec![
        Column::new("x", Role::X, s(&["W1","W1","W2","W2"])),
        Column::new("cat", Role::Category, s(&["a","b","a","b"])),
        Column::new("v", Role::Value(Kind::BarStackedPercent), f(&[1.0,2.0,3.0,4.0])),
    ]);
    case("band_line", vec![
        Column::new("x", Role::X, s(&["W1","W2","W3"])),
        Column::new("v", Role::Value(Kind::Line), f(&[10.0,20.0,15.0])),
        Column::new("lo", Role::BandLower, f(&[8.0,17.0,12.0])),
        Column::new("hi", Role::BandUpper, f(&[12.0,23.0,18.0])),
    ]);
    case("text", vec![
        Column::new("v", Role::Text(TextSize::Medium), s(&["hi"])),
    ]);
}
