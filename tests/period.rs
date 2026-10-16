//! `::PERIOD` date arithmetic against the shared vectors
//! (tests/fixtures/period_vectors.json — the browser checks the same file
//! through the wasm export in tests/web/period_test.mjs).
use anofox_visualization::period::resolve_json;
use serde_json::Value;

#[test]
fn period_vectors_match_the_shared_fixture() {
    let fx: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/fixtures/period_vectors.json").unwrap(),
    )
    .unwrap();
    let mut bad = Vec::new();
    for c in fx["cases"].as_array().unwrap() {
        let got = resolve_json(c["spec"].as_str().unwrap(), &c["state"].to_string());
        let want = |k: &str| c.get(k).cloned();
        let ok = want("vars").is_some_and(|v| v == got["vars"])
            && want("ok").is_some_and(|v| v == got["ok"])
            && want("preset").is_some_and(|v| v == got["preset"]);
        if !ok {
            bad.push(format!(
                "{}: got {{\"vars\": {}, \"ok\": {}, \"preset\": {}}}",
                c["name"], got["vars"], got["ok"], got["preset"]
            ));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}
