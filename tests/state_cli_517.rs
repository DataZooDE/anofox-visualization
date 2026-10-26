//! `dashboard state export|erase` (#517 S5, plan §1 "Operator access", §10
//! D7): offline, operator-only tooling against an `options.state` store
//! file. Exercised through the real binary (`CARGO_BIN_EXE_dashboard`), not
//! unit tests of `Store::erase_user` directly (those live in
//! `src/host/state.rs`) — this file is about the *CLI surface*: argument
//! parsing, the dry-run default, the audit line, and refusing a file a
//! `Store` elsewhere in this same process still holds `flock`'d (standing
//! in for "a server has it open").

use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d =
            std::env::temp_dir().join(format!("anofox_517_state_cli_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Scratch(d)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A hand-written store file — doesn't need a real server or submission to
/// exist, just something `Store::open` accepts (plan §1's line format).
fn write_store(path: &Path) {
    std::fs::write(
        path,
        "{\"anofox_state\":1,\"created\":\"2026-10-26T00:00:00.000Z\"}\n\
         {\"v\":1,\"seq\":1,\"ts\":\"2026-10-26T00:01:00.000Z\",\"user\":\"alice@example.com\",\"auth\":\"oidc\",\"dashboard\":\"tutorial\",\"submit\":\"quiz_1\",\"id\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"vars\":{\"q1\":\"B\"}}\n\
         {\"v\":1,\"seq\":2,\"ts\":\"2026-10-26T00:02:00.000Z\",\"user\":\"bob@example.com\",\"auth\":\"oidc\",\"dashboard\":\"tutorial\",\"submit\":\"quiz_1\",\"id\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",\"vars\":{\"q1\":\"A\",\"notes\":[\"x\",\"y\",null]}}\n\
         {\"v\":1,\"seq\":3,\"ts\":\"2026-10-26T00:03:00.000Z\",\"user\":\"alice@example.com\",\"auth\":\"oidc\",\"dashboard\":\"tutorial\",\"submit\":\"quiz_2\",\"id\":\"cccccccccccccccccccccccccccccccc\",\"vars\":{\"q2\":null}}\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn dashboard() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dashboard"))
}

#[test]
fn export_jsonl_defaults_to_every_record_in_seq_order() {
    let s = Scratch::new("export-all");
    let p = s.0.join("state.jsonl");
    write_store(&p);
    let out = dashboard()
        .arg("state")
        .arg("export")
        .arg(&p)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{stdout}");
    assert!(lines[0].contains("\"seq\":1") && lines[0].contains("alice"));
    assert!(lines[1].contains("\"seq\":2") && lines[1].contains("bob"));
    assert!(lines[2].contains("\"seq\":3") && lines[2].contains("alice"));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("exported 3 record(s)"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn export_user_scopes_to_only_that_viewers_rows() {
    let s = Scratch::new("export-user");
    let p = s.0.join("state.jsonl");
    write_store(&p);
    let out = dashboard()
        .arg("state")
        .arg("export")
        .arg(&p)
        .arg("--user")
        .arg("alice@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"seq\":1"));
    assert!(stdout.contains("\"seq\":3"));
    assert!(!stdout.contains("bob"), "{stdout}");
}

#[test]
fn export_csv_is_one_row_per_recorded_variable_with_a_real_null_list_item_blank() {
    let s = Scratch::new("export-csv");
    let p = s.0.join("state.jsonl");
    write_store(&p);
    let out = dashboard()
        .arg("state")
        .arg("export")
        .arg(&p)
        .arg("--format")
        .arg("csv")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next().unwrap(),
        "seq,ts,user,auth,dashboard,submit,id,name,value,value_list"
    );
    let rows: Vec<&str> = lines.collect();
    // bob's MULTISELECT ("notes") becomes its own row: both `value` and
    // `value_list` are the exact JSON array text (review of #555, nit —
    // a `;`-joined string couldn't tell an item containing `;` apart from
    // the separator; JSON array text always can, and a null item stays
    // JSON `null` rather than disappearing into an empty join). CSV's own
    // quoting (RFC 4180: a field with a `"` or `,` is wrapped and its
    // `"` doubled) then wraps that JSON text as `"[""x"",""y"",null]"`.
    let notes_row = rows
        .iter()
        .find(|l| l.contains(",notes,"))
        .unwrap_or_else(|| panic!("no notes row in: {stdout}"));
    assert!(
        notes_row.ends_with(r#",notes,"[""x"",""y"",null]","[""x"",""y"",null]""#),
        "{notes_row}"
    );
    // alice's "q2" was recorded as a scalar NULL: both cells blank, not "null".
    let q2_row = rows.iter().find(|l| l.contains(",q2,")).unwrap();
    assert!(q2_row.ends_with(",q2,,"), "{q2_row}");
}

#[test]
fn export_refuses_a_path_that_does_not_exist_rather_than_create_one() {
    let s = Scratch::new("export-missing");
    let p = s.0.join("nope.jsonl");
    let out = dashboard()
        .arg("state")
        .arg("export")
        .arg(&p)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!p.exists(), "export must never create a store file");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("No such file"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn erase_without_yes_is_a_dry_run_that_touches_nothing() {
    let s = Scratch::new("erase-dry");
    let p = s.0.join("state.jsonl");
    write_store(&p);
    let before = std::fs::read(&p).unwrap();
    let out = dashboard()
        .arg("state")
        .arg("erase")
        .arg("--user")
        .arg("alice@example.com")
        .arg(&p)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("dry run"), "{stdout}");
    assert!(stdout.contains("would remove 2 record(s)"), "{stdout}");
    assert!(stdout.contains("--yes"), "{stdout}");
    let after = std::fs::read(&p).unwrap();
    assert_eq!(before, after, "a dry run must not write anything");
}

#[test]
fn erase_yes_removes_only_the_named_users_rows_and_writes_an_audit_line() {
    let s = Scratch::new("erase-yes");
    let p = s.0.join("state.jsonl");
    write_store(&p);
    let out = dashboard()
        .arg("state")
        .arg("erase")
        .arg("--user")
        .arg("alice@example.com")
        .arg(&p)
        .arg("--yes")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("erased 2 record(s)"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    // Default audit target is stderr, one JSON line, never the values.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("\"audit\":\"state_erase\""), "{stderr}");
    assert!(stderr.contains("\"outcome\":\"ok\""), "{stderr}");
    assert!(stderr.contains("alice@example.com"), "{stderr}");
    assert!(
        !stderr.contains("\"B\""),
        "answer values must never reach the audit line: {stderr}"
    );

    let left = std::fs::read_to_string(&p).unwrap();
    assert!(left.contains("anofox_state"), "header line survives");
    assert!(left.contains("bob@example.com"), "bob's row survives");
    assert!(!left.contains("alice@example.com"), "{left}");
    assert_eq!(left.lines().count(), 2, "header + bob's one row: {left}");
}

#[test]
fn erase_audit_file_target_gets_the_same_line_the_stderr_default_would() {
    let s = Scratch::new("erase-audit-file");
    let p = s.0.join("state.jsonl");
    let audit_path = s.0.join("audit.jsonl");
    write_store(&p);
    let out = dashboard()
        .arg("state")
        .arg("erase")
        .arg("--user")
        .arg("bob@example.com")
        .arg(&p)
        .arg("--yes")
        .arg("--audit")
        .arg(&audit_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("state_erase"),
        "with --audit <file>, the line must not also go to stderr"
    );
    let audit = std::fs::read_to_string(&audit_path).unwrap();
    assert!(audit.contains("\"audit\":\"state_erase\""), "{audit}");
    assert!(audit.contains("bob@example.com"), "{audit}");
}

#[test]
fn erase_requires_a_user_and_a_yes_each_separately() {
    let s = Scratch::new("erase-args");
    let p = s.0.join("state.jsonl");
    write_store(&p);
    // No --user at all: a usage error, not "erase everyone".
    let out = dashboard()
        .arg("state")
        .arg("erase")
        .arg(&p)
        .arg("--yes")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 4);
}

#[test]
fn erase_refuses_while_the_store_is_still_open_elsewhere_in_process() {
    let s = Scratch::new("erase-locked");
    let p = s.0.join("state.jsonl");
    write_store(&p);
    // Stands in for "a server has this file open": the real lock
    // (`flock`) a live server's `Store::open` would hold, taken directly
    // rather than spawning a whole locked server for one assertion.
    let _held = anofox_visualization::host::state::Store::open(&p, 256).unwrap();
    let out = dashboard()
        .arg("state")
        .arg("erase")
        .arg("--user")
        .arg("alice@example.com")
        .arg(&p)
        .arg("--yes")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("in use by another server"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let export_out = dashboard()
        .arg("state")
        .arg("export")
        .arg(&p)
        .output()
        .unwrap();
    assert!(!export_out.status.success());
    assert!(
        String::from_utf8_lossy(&export_out.stderr).contains("in use by another server"),
        "{}",
        String::from_utf8_lossy(&export_out.stderr)
    );
}

#[test]
fn export_user_must_not_be_empty() {
    let s = Scratch::new("export-empty-user");
    let p = s.0.join("state.jsonl");
    write_store(&p);
    let out = dashboard()
        .arg("state")
        .arg("export")
        .arg(&p)
        .arg("--user")
        .arg("")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("must not be empty"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn erase_user_must_not_be_empty() {
    let s = Scratch::new("erase-empty-user");
    let p = s.0.join("state.jsonl");
    write_store(&p);
    let before = std::fs::read(&p).unwrap();
    let out = dashboard()
        .arg("state")
        .arg("erase")
        .arg("--user")
        .arg("")
        .arg(&p)
        .arg("--yes")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("must not be empty"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(&p).unwrap(),
        before,
        "must not touch the store"
    );
}

#[test]
fn erase_audit_line_carries_the_operators_own_uid_never_a_value() {
    let s = Scratch::new("erase-audit-uid");
    let p = s.0.join("state.jsonl");
    write_store(&p);
    let out = dashboard()
        .arg("state")
        .arg("erase")
        .arg("--user")
        .arg("alice@example.com")
        .arg(&p)
        .arg("--yes")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    // review of #555, nit: the audit line now names the operator (this
    // test process's own real uid — the CLI has no other identity to log)
    // alongside the viewer `user` whose rows were removed.
    let my_uid = unsafe { libc::getuid() };
    assert!(
        stderr.contains(&format!("\"operator_uid\":{my_uid}")),
        "{stderr}"
    );
}

#[test]
fn export_out_creates_a_0600_file_and_never_a_world_or_group_readable_one() {
    use std::os::unix::fs::PermissionsExt;
    let s = Scratch::new("export-out-mode");
    let p = s.0.join("state.jsonl");
    write_store(&p);
    let out_path = s.0.join("alice.jsonl");
    let out = dashboard()
        .arg("state")
        .arg("export")
        .arg(&p)
        .arg("--out")
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta = std::fs::metadata(&out_path).unwrap();
    assert_eq!(
        meta.permissions().mode() & 0o777,
        0o600,
        "export --out must never be created group/world readable, regardless of umask"
    );
    assert!(std::fs::read_to_string(&out_path)
        .unwrap()
        .contains("alice@example.com"));
}

#[test]
fn export_out_refuses_an_existing_file_without_force_and_overwrites_with_it() {
    let s = Scratch::new("export-out-exists");
    let p = s.0.join("state.jsonl");
    write_store(&p);
    let out_path = s.0.join("out.csv");
    std::fs::write(&out_path, "pre-existing content\n").unwrap();

    let refused = dashboard()
        .arg("state")
        .arg("export")
        .arg(&p)
        .arg("--out")
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("--force"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&out_path).unwrap(),
        "pre-existing content\n",
        "a refused export must not touch the existing file"
    );

    let forced = dashboard()
        .arg("state")
        .arg("export")
        .arg(&p)
        .arg("--out")
        .arg(&out_path)
        .arg("--force")
        .output()
        .unwrap();
    assert!(
        forced.status.success(),
        "{}",
        String::from_utf8_lossy(&forced.stderr)
    );
    let after = std::fs::read_to_string(&out_path).unwrap();
    assert!(after.contains("alice@example.com"), "{after}");
    assert!(!after.contains("pre-existing"), "{after}");
}

#[test]
fn export_out_never_follows_a_symlink_even_with_force() {
    let s = Scratch::new("export-out-symlink");
    let p = s.0.join("state.jsonl");
    write_store(&p);
    let victim = s.0.join("victim.csv");
    std::fs::write(&victim, "do not touch\n").unwrap();
    let link = s.0.join("link.csv");
    std::os::unix::fs::symlink(&victim, &link).unwrap();

    for force in [false, true] {
        let mut cmd = dashboard();
        cmd.arg("state")
            .arg("export")
            .arg(&p)
            .arg("--out")
            .arg(&link);
        if force {
            cmd.arg("--force");
        }
        let out = cmd.output().unwrap();
        assert!(!out.status.success(), "force={force}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("symlink"),
            "force={force}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "do not touch\n",
            "force={force}: must never write through the symlink to the victim"
        );
    }
}

/// A store file with a torn (no trailing `\n`) final line — stands in for
/// a crash or `kill -9` mid-`append`, the same fixture `Store::open`'s own
/// unit tests use, but exercised here through the CLI.
fn write_torn_store(path: &Path) {
    std::fs::write(
        path,
        "{\"anofox_state\":1,\"created\":\"2026-10-26T00:00:00.000Z\"}\n\
         {\"v\":1,\"seq\":1,\"ts\":\"2026-10-26T00:01:00.000Z\",\"user\":\"alice@example.com\",\"auth\":\"oidc\",\"dashboard\":\"tutorial\",\"submit\":\"quiz_1\",\"id\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"vars\":{\"q1\":\"B\"}}\n\
         {\"v\":1,\"seq\":2,\"ts\":\"2026-10-26T00:02:00.000Z\",\"user\":\"bob@example.com\",\"auth\":\"oidc\",\"dashboard\":\"tutorial\",\"submit\":\"quiz_1\",\"id\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",\"vars\":{\"q1\":\"A\"",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn export_on_a_torn_tail_store_leaves_the_file_byte_identical() {
    let s = Scratch::new("export-torn");
    let p = s.0.join("state.jsonl");
    write_torn_store(&p);
    let before = std::fs::read(&p).unwrap();
    let out = dashboard()
        .arg("state")
        .arg("export")
        .arg(&p)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Only alice's complete record is readable; bob's torn one is dropped
    // from the *output*, but — review of #555's should-fix — the file on
    // disk must stay exactly as found, torn tail and all: a read must
    // never "fix" a file nobody asked it to write.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("alice@example.com"), "{stdout}");
    assert!(!stdout.contains("bob@example.com"), "{stdout}");
    let after = std::fs::read(&p).unwrap();
    assert_eq!(before, after, "export must never truncate a torn tail");
}

#[test]
fn erase_dry_run_on_a_torn_tail_store_leaves_the_file_byte_identical() {
    let s = Scratch::new("erase-dry-torn");
    let p = s.0.join("state.jsonl");
    write_torn_store(&p);
    let before = std::fs::read(&p).unwrap();
    let out = dashboard()
        .arg("state")
        .arg("erase")
        .arg("--user")
        .arg("alice@example.com")
        .arg(&p)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("dry run"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let after = std::fs::read(&p).unwrap();
    assert_eq!(
        before, after,
        "a dry run must never truncate a torn tail, even on its read-only open"
    );
}
