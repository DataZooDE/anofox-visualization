//! #1076: the AI failure corpus — SQL that models actually wrote in the
//! #1044 benchmark runs, each with a small table of the same shape, and what
//! `dashboard --check` must say about it. A mistake keeps its actionable hint;
//! the version that worked keeps working; nothing ever crashes. No LLM: this
//! runs on every PR (DuckDB CLI only).
//!
//! A fixture (`tests/ai_corpus/*.sql`) starts with
//!
//! ```sql
//! -- @from <run> <task>: what the model did
//! -- @expect clean                 -- --check is clean and --svg renders
//! -- @expect no-crash              -- any diagnostics, but no internal error
//! -- @expect <code> <text>         -- a diagnostic <code> whose message has <text>
//! ```
//!
//! New fixtures come from a run's gaps.md (`bench/viz-tokens/gaps.py`): the
//! failed call, the call that worked after it, and a `CREATE TABLE` with the
//! columns they read.
mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn dashboard(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .env("DATAZOO_DISABLE_TELEMETRY", "1")
        .args(args)
        .output()
        .unwrap()
}

/// Text of a crash or an internal error, wherever it shows.
fn crashed(s: &str) -> bool {
    ["panicked", "internal render error", "RUST_BACKTRACE"]
        .iter()
        .any(|k| s.contains(k))
}

#[derive(Debug, PartialEq)]
enum Expect {
    Clean,
    NoCrash,
    Diag { code: String, text: String },
}

fn expect_of(sql: &str, name: &str) -> Expect {
    let line = sql
        .lines()
        .find_map(|l| l.strip_prefix("-- @expect "))
        .unwrap_or_else(|| panic!("{name}: no `-- @expect` line"))
        .trim();
    match line {
        "clean" => Expect::Clean,
        "no-crash" => Expect::NoCrash,
        _ => {
            let (code, text) = line.split_once(' ').unwrap_or((line, ""));
            Expect::Diag {
                code: code.into(),
                text: text.trim().into(),
            }
        }
    }
}

fn fixtures() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ai_corpus");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "sql"))
        .collect();
    v.sort();
    v
}

#[test]
fn every_fixture_says_what_it_expects() {
    let all = fixtures();
    assert!(all.len() >= 15, "the corpus shrank: {}", all.len());
    for f in &all {
        let sql = std::fs::read_to_string(f).unwrap();
        let name = f.file_name().unwrap().to_string_lossy();
        assert!(
            sql.starts_with("-- @from "),
            "{name}: first line is `-- @from <run> <task>: …`"
        );
        expect_of(&sql, &name);
    }
}

#[test]
fn the_corpus_keeps_its_hints_and_never_crashes() {
    if Command::new("duckdb").arg("-version").output().is_err() {
        common::skip_without("duckdb", "the_corpus_keeps_its_hints_and_never_crashes");
        return;
    }
    let out_dir = std::env::temp_dir().join(format!("anofox-1076-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut fails = Vec::new();
    for f in fixtures() {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let sql = std::fs::read_to_string(&f).unwrap();
        let expect = expect_of(&sql, &name);
        let check = dashboard(&["--check".as_ref(), f.as_os_str(), "--json".as_ref()]);
        let text = String::from_utf8_lossy(&check.stdout).to_string()
            + &String::from_utf8_lossy(&check.stderr);
        if crashed(&text) {
            fails.push(format!("{name}: --check crashed:\n{text}"));
            continue;
        }
        let json: serde_json::Value = match serde_json::from_slice(&check.stdout) {
            Ok(j) => j,
            Err(e) => {
                fails.push(format!("{name}: --check --json is not JSON ({e}):\n{text}"));
                continue;
            }
        };
        let diags: Vec<(String, String)> = json["diagnostics"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|d| {
                        (
                            d["code"].as_str().unwrap_or("").to_string(),
                            d["message"].as_str().unwrap_or("").to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        match &expect {
            Expect::Clean if !diags.is_empty() => {
                fails.push(format!("{name}: expected clean, got {diags:?}"))
            }
            Expect::Diag { code, text }
                if !diags
                    .iter()
                    .any(|(c, m)| c == code && m.contains(text.as_str())) =>
            {
                fails.push(format!(
                    "{name}: expected `{code}` with \"{text}\", got {diags:?}"
                ))
            }
            _ => {}
        }
        // every fixture renders without crashing; a clean one renders at all
        let svg = out_dir.join(format!("{name}.svg"));
        let run = dashboard(&["--svg".as_ref(), f.as_os_str(), svg.as_os_str()]);
        let err = String::from_utf8_lossy(&run.stderr);
        if crashed(&err) {
            fails.push(format!("{name}: --svg crashed:\n{err}"));
        } else if expect == Expect::Clean && !(run.status.success() && svg.exists()) {
            fails.push(format!("{name}: --svg failed:\n{err}"));
        }
    }
    let _ = std::fs::remove_dir_all(&out_dir);
    assert!(fails.is_empty(), "{}", fails.join("\n\n"));
}

/// The mistakes models make, applied to a working script's last statement.
fn mutations(sql: &str) -> Vec<(&'static str, String)> {
    let (head, last) = match sql.trim_end().trim_end_matches(';').rsplit_once(";\n") {
        Some((h, l)) => (format!("{h};\n"), l.to_string()),
        None => (
            String::new(),
            sql.trim_end().trim_end_matches(';').to_string(),
        ),
    };
    let kinds = [
        "::BARCHART",
        "::LINECHART",
        "::FUNNEL",
        "::TABLE",
        "::ORIGIN",
        "::HEATMAP",
    ];
    let kind = kinds.iter().find(|k| last.contains(*k)).copied();
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let with = |l: String| format!("{head}{l};\n");
    if let Some(k) = kind {
        out.push((
            "chained role",
            with(last.replacen(k, &format!("::MONEY{k}"), 1)),
        ));
        out.push((
            "role after a second alias",
            with(last.replacen(k, &format!(" AS \"v\" {k}"), 1)),
        ));
    }
    out.push(("roles in lower case", with(last.to_lowercase())));
    out.push((
        "roles inside a subquery",
        with(format!("SELECT * FROM ({last})")),
    ));
    out.push((
        "a stray role at the end",
        with(format!("{last}\n::SCATTER")),
    ));
    out.push((
        "no ';' before it",
        format!("{}\n{last};\n", head.trim_end().trim_end_matches(';')),
    ));
    out.push(("cut off", with(last[..last.len() * 2 / 3].to_string())));
    out.push(("a space before ::", with(last.replace("::", " ::"))));
    out.push(("roles swapped", {
        let toks: Vec<&str> = last.match_indices("::").map(|(i, _)| &last[i..]).collect();
        let tok = |t: &str| {
            t[2..]
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .next()
                .unwrap_or("")
                .to_string()
        };
        match (toks.first().map(|t| tok(t)), toks.get(1).map(|t| tok(t))) {
            (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() && a != b => with(
                last.replacen(&format!("::{a}"), "::\u{1}", 1)
                    .replacen(&format!("::{b}"), &format!("::{a}"), 1)
                    .replacen("::\u{1}", &format!("::{b}"), 1),
            ),
            _ => with(last.clone()),
        }
    }));
    out
}

#[test]
fn mistakes_on_working_scripts_get_a_message_not_a_crash() {
    if Command::new("duckdb").arg("-version").output().is_err() {
        common::skip_without(
            "duckdb",
            "mistakes_on_working_scripts_get_a_message_not_a_crash",
        );
        return;
    }
    let roles: Vec<&str> = anofox_visualization::roles::REGISTRY
        .iter()
        .map(|s| s.token)
        .collect();
    let dir = std::env::temp_dir().join(format!("anofox-1076-mut-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut fails = Vec::new();
    let mut n = 0;
    for f in fixtures() {
        let sql = std::fs::read_to_string(&f).unwrap();
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        if expect_of(&sql, &name) != Expect::Clean {
            continue;
        }
        for (what, m) in mutations(&sql) {
            n += 1;
            let p = dir.join(format!("m{n}.sql"));
            std::fs::write(&p, &m).unwrap();
            let check = dashboard(&["--check".as_ref(), p.as_os_str(), "--json".as_ref()]);
            let text = String::from_utf8_lossy(&check.stdout).to_string()
                + &String::from_utf8_lossy(&check.stderr);
            let ctx = format!("{name} / {what}:\n{m}");
            if crashed(&text) {
                fails.push(format!("{ctx}\n→ crashed: {text}"));
                continue;
            }
            let Ok(json) = serde_json::from_slice::<serde_json::Value>(&check.stdout) else {
                fails.push(format!("{ctx}\n→ not JSON: {text}"));
                continue;
            };
            for d in json["diagnostics"].as_array().into_iter().flatten() {
                let msg = d["message"].as_str().unwrap_or("");
                if msg.trim().is_empty() {
                    fails.push(format!("{ctx}\n→ an empty message: {d}"));
                }
                // #1056: a role DuckDB took for a type always says it's a role
                for r in &roles {
                    if msg.contains(&format!("Type with name {r} does not exist"))
                        && !msg.contains("is a role")
                    {
                        fails.push(format!(
                            "{ctx}\n→ ::{r} reached DuckDB as a type, no hint: {msg}"
                        ));
                    }
                }
            }
            let svg = dir.join(format!("m{n}.svg"));
            let run = dashboard(&["--svg".as_ref(), p.as_os_str(), svg.as_os_str()]);
            let err = String::from_utf8_lossy(&run.stderr);
            if crashed(&err) {
                fails.push(format!("{ctx}\n→ --svg crashed: {err}"));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(n >= 40, "only {n} mutations ran");
    assert!(
        fails.is_empty(),
        "{} of {n}:\n\n{}",
        fails.len(),
        fails.join("\n\n")
    );
}
