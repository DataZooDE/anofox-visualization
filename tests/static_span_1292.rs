//! #1292: the dashboard CLI's HTML page sizes a panel from the `::SPAN` /
//! `::HEIGHT` statements before it, as `--svg` and the browser do.

mod common;

use std::process::Command;

#[test]
fn span_and_height_size_a_panel_on_the_html_page() {
    if Command::new("duckdb").arg("-version").output().is_err() {
        common::skip_without("duckdb", "span_and_height_size_a_panel_on_the_html_page");
        return;
    }
    let dir = std::env::temp_dir().join(format!("anofox-span-1292-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sql = dir.join("wide.sql");
    std::fs::write(
        &sql,
        "SELECT 8 ::SPAN;\nSELECT 0.5 ::HEIGHT;\nSELECT 'a' AS x ::XAXIS, 1 AS y ::BARCHART;\n",
    )
    .unwrap();
    let html = dir.join("wide.html");
    let out = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .arg(&sql)
        .arg(&html)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = std::fs::read_to_string(&html).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(body.contains("class=\"grid dp-spans\""));
    // 8 of 12 columns at their width on the page grid (#1345): 869 wide,
    // half as high
    let fig = body.split("<figure ").nth(1).expect("a panel");
    assert!(fig.starts_with("class=\"panel dp-span-8\""), "{fig}");
    assert!(fig.contains("viewBox=\"0 0 869 435\""), "{}", &fig[..300]);
}
