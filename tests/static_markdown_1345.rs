//! #1345 slice 3: `dashboard file.sql out.html` draws a `::MARKDOWN` text as
//! HTML by the browser's rules (src/markdown_html.rs, parity with
//! `renderMarkdown` in web/app.js) — code blocks, lists, headings — instead
//! of plain SVG text lines. Values fill in as escaped text; markup in the
//! source stays text. Needs the `duckdb` CLI (skips without it locally,
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
        std::env::temp_dir().join(format!("anofox_1345md_{tag}_{}", std::process::id()));
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

#[test]
fn markdown_is_html_with_code_blocks_lists_and_values() {
    let script = "SELECT '## How\n\n- one **two**\n- <script>x</script>\n\n```\nSELECT 1 < 2;\n```\n\nTotal: {{n}}'::MARKDOWN, \
                  'Notes'::TITLE, 1234 AS n;\n";
    let Some(html) = render("md", script) else {
        return;
    };
    assert!(
        html.contains(
            "<figcaption class=\"panel-title\">Notes</figcaption><div class=\"md-body\"><h2>How</h2>\
             <ul><li>one <strong>two</strong></li><li>&lt;script&gt;x&lt;/script&gt;</li></ul>\
             <pre><code>SELECT 1 &lt; 2;\n</code></pre><p>Total: 1,234</p></div>"
        ),
        "{html}"
    );
    assert!(!html.contains("<script>x"), "markup in the text stays text");
}

#[test]
fn a_details_tone_folds_open_under_its_label() {
    let script = "SELECT 'Hidden *text*'::MARKDOWN, 'details: Method'::MARKDOWN_TONE;\n";
    let Some(html) = render("details", script) else {
        return;
    };
    assert!(
        html.contains(
            "<details class=\"md-details\" open><summary>Method</summary><div class=\"md-body\"><p>Hidden <em>text</em></p></div></details>"
        ),
        "{html}"
    );
    // the label is the <summary>, not a tone label on the card
    assert!(
        html.contains("<figure class=\"panel md-tone md-tone-details\"><details"),
        "{html}"
    );
}
