//! The shared table formatting vectors (tests/fixtures/table_vectors.json):
//! the Rust table module reads a column like the browser table does
//! (tests/web/table_test.mjs checks the browser side).
use anofox_visualization::format::Locale;
use anofox_visualization::table::{cell_text, profile_column, Cell, ColumnSpec, Negatives};

fn norm(s: &str) -> String {
    s.replace(['\u{202f}', '\u{a0}'], " ")
}

#[test]
fn table_vectors() {
    let v: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/table_vectors.json")).unwrap();
    for case in v["cases"].as_array().unwrap() {
        let l = Locale::from_tag(case["locale"].as_str().unwrap()).unwrap();
        for (k, c) in case["cols"].as_array().unwrap().iter().enumerate() {
            let cells: Vec<Cell> = c["values"]
                .as_array()
                .unwrap()
                .iter()
                .map(Cell::from_json)
                .collect();
            let spec = ColumnSpec {
                name: c["name"].as_str().unwrap(),
                fmt: c["fmt"].as_str(),
                first: k == 0,
                range: None,
                many: true,
            };
            let p = profile_column(&spec, &cells, &l);
            let Some(head) = c["head"].as_str() else {
                continue;
            };
            let at = format!("{} {}", case["locale"], spec.name);
            assert_eq!(norm(&p.head), norm(head), "head of {at}");
            let want: Vec<String> = c["cells"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| norm(x.as_str().unwrap()))
                .collect();
            let got: Vec<String> = cells
                .iter()
                .map(|x| norm(&cell_text(x, &p, spec.fmt, &l, Negatives::Minus)))
                .collect();
            assert_eq!(got, want, "cells of {at}");
        }
    }
}
