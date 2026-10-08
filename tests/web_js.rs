//! Runs the browser-sink regression test (`tests/web/sanitize_test.mjs`:
//! escapeHtml / safeUrl / renderMarkdown from `web/app.js`) under Node.
//! Skipped (with a note) when `node` is not installed.

#[test]
fn web_html_sinks_are_inert() {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/web/sanitize_test.mjs");
    let out = match std::process::Command::new("node").arg(script).output() {
        Ok(o) => o,
        Err(_) => {
            eprintln!("node not found — skipping web sanitize test");
            return;
        }
    };
    assert!(
        out.status.success(),
        "web sanitize test failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
