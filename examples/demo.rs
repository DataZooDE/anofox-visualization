use anofox_visualization::{render, Column, Kind, Role};
use ggplot_rs::prelude::Value;
fn s(v: &[&str]) -> Vec<Value> {
    v.iter().map(|x| Value::Str(x.to_string())).collect()
}
fn n(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}
fn main() {
    // SELECT week::XAXIS, category::CATEGORY, count()::BARCHART_STACKED  (Shaper's example)
    let cols = vec![
        Column::new("t", Role::Label, s(&["Sessions per week"])),
        Column::new(
            "week",
            Role::X,
            s(&["W1", "W1", "W1", "W2", "W2", "W2", "W3", "W3", "W3"]),
        ),
        Column::new(
            "cat",
            Role::Category,
            s(&[
                "app", "web", "api", "app", "web", "api", "app", "web", "api",
            ]),
        ),
        Column::new(
            "n",
            Role::Value(Kind::BarStacked),
            n(&[30., 22., 12., 41., 28., 15., 26., 33., 9.]),
        ),
    ];
    std::fs::write(
        std::env::args().nth(1).unwrap(),
        render(&cols, 520, 320).unwrap(),
    )
    .unwrap();
}
