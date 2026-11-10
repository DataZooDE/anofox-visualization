//! #1084: a static table (`dashboard --svg`, the server's PNG / PDF exports,
//! chat delivery) draws what the browser draws around a cell — the
//! `::COLORSCALE` tints, `::BADGE` pills and the slim bar of a plain numeric
//! column — in the theme's colours. They used to come out as plain text, so
//! a model's correct colour-scaled table read as uncoloured (the #1044 run's
//! tbl-colorscale, 0/3).
mod common;

use std::process::Command;

fn render(sql: &str, tag: &str) -> Option<String> {
    if Command::new("duckdb").arg("-version").output().is_err() {
        common::skip_without("duckdb", tag);
        return None;
    }
    let dir = std::env::temp_dir().join(format!("anofox-1084-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (f, out) = (dir.join("t.sql"), dir.join("t.svg"));
    std::fs::write(&f, sql).unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .env("DATAZOO_DISABLE_TELEMETRY", "1")
        .arg("--svg")
        .arg(&f)
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
    common::assert_safe_svg(&svg, tag);
    Some(svg)
}

const DATA: &str = "CREATE TABLE r AS SELECT * FROM (VALUES
  ('North', 1200, 18.2, 0.91, 2.5, 'on track'), ('South', 1100, 9.5, 0.84, -4.1, 'critical'),
  ('West', 800, 14.0, 0.88, 0.0, 'ok'), ('East', 950, 11.3, 0.79, -1.2, 'at risk')) t(region, plan, margin, accuracy, bias, status);\n";

/// The fills of `<rect … height="H" …>` elements.
fn rect_fills(svg: &str, height: &str) -> Vec<String> {
    svg.split("<rect ")
        .skip(1)
        .map(|r| &r[..r.find('>').unwrap()])
        .filter(|r| r.contains(&format!("height=\"{height}\"")))
        .filter_map(|r| {
            let k = r.find("fill=\"")? + 6;
            Some(r[k..k + r[k..].find('"')?].to_string())
        })
        .collect()
}

#[test]
fn colour_scales_badges_and_bars_are_drawn() {
    let Some(svg) = render(
        &format!(
            "{DATA}SELECT region::TABLE, plan::MONEY, margin, accuracy::COLORSCALE_HIGH_GOOD, \
             bias::COLORSCALE_DIVERGING, status::BADGE, 'Plan'::TITLE FROM r;"
        ),
        "all",
    ) else {
        return;
    };
    // 18 px cells: 4 accuracy tints + their 4 edges, 4 bias tints + edges, 4 badges
    let cells = rect_fills(&svg, "18");
    assert_eq!(cells.len(), 20, "{cells:?}");
    let distinct: std::collections::HashSet<_> = cells.iter().collect();
    assert!(
        distinct.len() >= 8,
        "the tints follow the values: {cells:?}"
    );
    // the slim bars of the plain `margin` column
    assert_eq!(rect_fills(&svg, "3").len(), 4, "one bar per row");
    // the status words keep their text
    for w in ["on track", "critical", "ok", "at risk"] {
        assert!(svg.contains(&format!(">{w}<")), "{w}");
    }
}

#[test]
fn a_plain_table_keeps_plain_cells() {
    let Some(svg) = render(
        &format!("{DATA}SELECT region::TABLE, plan::MONEY, 'Plan'::TITLE FROM r;"),
        "plain",
    ) else {
        return;
    };
    assert!(rect_fills(&svg, "18").is_empty());
    assert!(rect_fills(&svg, "3").is_empty());
}
