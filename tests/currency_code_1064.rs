//! #1064: `'EUR'::CURRENCY_CODE` beside a value sets that panel's currency —
//! a `::MONEY` KPI, a table's `::MONEY` columns and a chart's y axis — while
//! the locale keeps the marks and the currency's place. Other panels keep the
//! locale's own currency.
mod common;

use anofox_visualization::{render_with, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;
use std::process::Command;

fn strs(v: &[&str]) -> Vec<Value> {
    v.iter().map(|s| Value::Str(s.to_string())).collect()
}
fn nums(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}

/// The text nodes of an SVG.
fn texts(svg: &str) -> Vec<String> {
    svg.split('>')
        .filter_map(|p| p.split('<').next())
        .map(|t| t.replace("&#39;", "'"))
        .filter(|t| !t.trim().is_empty())
        .collect()
}

#[test]
fn a_kpi_takes_its_currency_code() {
    let kpi = |code: Option<&str>, o: &RenderOptions| {
        let mut cols = vec![
            Column::new(
                "v",
                Role::Metric(anofox_visualization::MetricFmt::Money),
                nums(&[889_560.0]),
            ),
            Column::new("l", Role::Label, strs(&["Revenue"])),
        ];
        if let Some(c) = code {
            cols.push(Column::new("c", Role::CurrencyCode, strs(&[c])));
        }
        texts(&render_with(&cols, 320, 120, o).expect("renders"))
    };
    let en = RenderOptions::default();
    assert!(kpi(Some("EUR"), &en).contains(&"€889,560".to_string()));
    assert!(kpi(None, &en).contains(&"$889,560".to_string()));
    // German marks, the euro's German place; the code beats the locale's
    let de = RenderOptions {
        locale: anofox_visualization::format::Locale::DE_DE,
        ..RenderOptions::default()
    };
    assert!(
        kpi(Some("USD"), &de).contains(&"889.560 $".to_string()),
        "{:?}",
        kpi(Some("USD"), &de)
    );
    // an unknown code keeps the locale's currency (lint flags it)
    assert!(kpi(Some("Euro"), &en).contains(&"$889,560".to_string()));
}

#[test]
fn a_chart_axis_takes_its_currency_code_unless_it_has_a_yformat() {
    let chart = |extra: Vec<Column>| {
        let mut cols = vec![
            Column::new("m", Role::X, strs(&["Jan", "Feb"])),
            Column::new("v", Role::Value(Kind::Bar), nums(&[12_000.0, 30_000.0])),
        ];
        cols.extend(extra);
        texts(&render_with(&cols, 480, 320, &RenderOptions::default()).expect("renders"))
    };
    let t = chart(vec![Column::new(
        "c",
        Role::CurrencyCode,
        strs(&["EUR", "EUR"]),
    )]);
    assert!(t.iter().any(|s| s == "€30,000"), "{t:?}");
    let t = chart(vec![
        Column::new("c", Role::CurrencyCode, strs(&["EUR", "EUR"])),
        Column::new("f", Role::YFormat, strs(&["percent", "percent"])),
    ]);
    assert!(!t.iter().any(|s| s.contains('€')), "{t:?}");
    let t = chart(vec![Column::new(
        "c",
        Role::CurrencyCode,
        strs(&["CHF", "CHF"]),
    )]);
    assert!(t.iter().any(|s| s == "CHF 30,000"), "{t:?}");
}

/// A dashboard through the CLI on a real DuckDB: each panel its own currency,
/// the code never shown as a table column.
#[test]
fn a_dashboard_mixes_currencies_per_panel() {
    if Command::new("duckdb").arg("-version").output().is_err() {
        common::skip_without("duckdb", "a_dashboard_mixes_currencies_per_panel");
        return;
    }
    let dir = std::env::temp_dir().join(format!("anofox-1064-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (sql, out) = (dir.join("c.sql"), dir.join("c.svg"));
    std::fs::write(
        &sql,
        "SELECT 889560.0::MONEY, 'Revenue'::LABEL, 'EUR'::CURRENCY_CODE;\n\
         SELECT 5000::MONEY, 'Default'::LABEL;\n\
         SELECT r::TABLE, v::MONEY, 'Amounts'::TITLE, 'GBP'::CURRENCY_CODE \
         FROM (VALUES ('a', 1234.5), ('b', 98000.0)) t(r, v);\n",
    )
    .unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .env("DATAZOO_DISABLE_TELEMETRY", "1")
        .arg("--svg")
        .arg(&sql)
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let svg = std::fs::read_to_string(&out).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let t = texts(&svg);
    assert!(t.contains(&"€889,560".to_string()), "{t:?}");
    assert!(t.contains(&"$5,000".to_string()), "{t:?}");
    assert!(t.iter().any(|s| s.contains("(£)")), "{t:?}");
    assert!(!svg.contains("GBP"), "the code is not a table column");
}
