//! #1345: `dashboard file.sql out.html` keeps the dashboard's tabs and names
//! the page by its `-- @title` / `-- @description`. Each `::TAB` opens a
//! `<section class="dp-tab">` with the tab's name (the page's script turns
//! them into a tab bar); a dashboard without tabs keeps its flat grid and
//! carries no tab script. Needs the `duckdb` CLI (skips without it locally,
//! fails on CI — see `common::skip_without`).
mod common;
use std::path::PathBuf;
use std::process::Command;

fn render(tag: &str, script: &str) -> Option<String> {
    if Command::new("duckdb").arg("-version").output().is_err() {
        common::skip_without("duckdb", tag);
        return None;
    }
    let dir: PathBuf =
        std::env::temp_dir().join(format!("anofox_1345_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (sql, out) = (dir.join("d.sql"), dir.join("d.html"));
    std::fs::write(&sql, script).unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .arg(&sql)
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let html = std::fs::read_to_string(&out).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    Some(html)
}

const TABBED: &str = "-- @title Where do sales come from?\n\
-- @description Two tabs & a <note>.\n\
SELECT 'Overview'::TAB;\n\
SELECT 42::METRIC, 'orders'::LABEL;\n\
SELECT 'By region'::TAB;\n\
SELECT 'North'::SUBTAB;\n\
SELECT 7::METRIC, 'north'::LABEL;\n";

#[test]
fn tabs_become_sections_under_their_names() {
    let Some(html) = render("tabs", TABBED) else {
        return;
    };
    assert!(
        html.contains("<section class=\"dp-tab\" data-tab=\"Overview\">"),
        "{html}"
    );
    assert!(
        html.contains("<section class=\"dp-tab\" data-tab=\"By region\">"),
        "{html}"
    );
    // one section closes before the next opens, the last one at the end
    assert_eq!(html.matches("<section class=\"dp-tab\"").count(), 2);
    assert_eq!(html.matches("</section>").count(), 2);
    let (a, b) = (
        html.find("data-tab=\"Overview\"").unwrap(),
        html.find("data-tab=\"By region\"").unwrap(),
    );
    let between = &html[a..b];
    assert!(
        between.contains("orders"),
        "the first tab's panel sits in the first section"
    );
    assert!(
        html.contains("<h3 class=\"section dp-subtab\">North</h3>"),
        "a sub-tab is a heading"
    );
    // the tab bar script and its CSS come with the sections
    assert!(html.contains("querySelectorAll('section.dp-tab')"));
    assert!(html.contains("section.dp-tab{display:contents}"));
    // markers don't count as panels: two KPIs
    assert!(html.contains("· 2 panels"), "{html}");
}

#[test]
fn the_page_is_named_by_its_title_and_description() {
    let Some(html) = render("title", TABBED) else {
        return;
    };
    assert!(
        html.contains("<title>Where do sales come from?</title>"),
        "{html}"
    );
    assert!(html.contains(">Where do sales come from?</h1>"), "{html}");
    assert!(
        html.contains("<p class=\"dp-desc\">Two tabs &amp; a &lt;note&gt;.</p>"),
        "escaped: {html}"
    );
    assert!(!html.contains("anofox-visualization dashboard"));
}

#[test]
fn without_tabs_the_page_stays_flat() {
    let Some(html) = render("flat", "SELECT 1::METRIC, 'one'::LABEL;\n") else {
        return;
    };
    assert!(!html.contains("<section class=\"dp-tab\""));
    assert!(!html.contains("querySelectorAll('section.dp-tab')"));
    assert!(
        html.contains("<title>anofox-visualization dashboard</title>"),
        "no @title: the fallback"
    );
}
