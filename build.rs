// Stamps the build version into ANOFOX_VIZ_BUILD_VERSION (#216), from
// `git describe`: `v2026.10.18` for a release tag build, else
// `v2026.10.18+37 · 739aba4` (`-dirty` with uncommitted changes). A build
// outside this repository's own checkout (a crates.io tarball, a docker build
// without .git) gets `v<Cargo.toml version>`. The parsing is in
// src/build_version.rs, unit-tested with the core crate.
#[path = "src/build_version.rs"]
mod build_version;

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Re-run when HEAD moves, the branch gets a commit or a tag appears. Only
/// paths that exist: cargo re-runs on every build for a missing one.
fn watch_git(dir: &Path) {
    let mut paths = Vec::new();
    if let Some(g) = git(dir, &["rev-parse", "--absolute-git-dir"]) {
        paths.push(PathBuf::from(g).join("HEAD"));
    }
    if let Some(c) = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) {
        let c = PathBuf::from(c);
        if let Some(r) = git(dir, &["symbolic-ref", "-q", "HEAD"]) {
            paths.push(c.join(r));
        }
        paths.push(c.join("refs").join("tags"));
        paths.push(c.join("packed-refs"));
    }
    for p in paths.into_iter().filter(|p| p.exists()) {
        println!("cargo:rerun-if-changed={}", p.display());
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let pkg = std::env::var("CARGO_PKG_VERSION").unwrap();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    // Only this checkout's own git: a tarball unpacked inside some other
    // repository must not pick up that repository's tags.
    let own = git(&manifest, &["rev-parse", "--show-toplevel"])
        .and_then(|t| std::fs::canonicalize(t).ok())
        .is_some_and(|t| std::fs::canonicalize(&manifest).is_ok_and(|m| m == t));
    let describe = if own {
        watch_git(&manifest);
        git(
            &manifest,
            &[
                "describe",
                "--tags",
                "--match",
                "v*",
                "--long",
                "--dirty",
                "--abbrev=7",
            ],
        )
    } else {
        None
    };
    let v = build_version::display_version(describe.as_deref(), &pkg);
    println!("cargo:rustc-env=ANOFOX_VIZ_BUILD_VERSION={v}");
}
