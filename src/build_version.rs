//! The build version's display form (#216), from `git describe --tags
//! --match 'v*' --long --dirty --abbrev=7`. Included by build.rs, which stamps
//! it into `ANOFOX_VIZ_BUILD_VERSION`, and by the core's unit tests.

/// `v2026.10.18-0-g739aba4` → `v2026.10.18` (a release tag build);
/// `v2026.10.18-37-g739aba4` → `v2026.10.18+37 · 739aba4` (commits since the
/// tag · short sha); a `-dirty` describe keeps the long form and appends
/// `-dirty`. No describe (no git, no tag) or one that doesn't parse →
/// `v<pkg_version>`.
pub fn display_version(describe: Option<&str>, pkg_version: &str) -> String {
    describe
        .and_then(parse)
        .unwrap_or_else(|| format!("v{pkg_version}"))
}

fn parse(describe: &str) -> Option<String> {
    let d = describe.trim();
    let (d, dirty) = match d.strip_suffix("-dirty") {
        Some(rest) => (rest, true),
        None => (d, false),
    };
    // From the right: the tag itself may contain '-'.
    let mut parts = d.rsplitn(3, '-');
    let sha = parts.next()?.strip_prefix('g')?;
    let n = parts.next()?;
    let tag = parts.next()?;
    let hex = !sha.is_empty() && sha.bytes().all(|b| b.is_ascii_hexdigit());
    let num = !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
    if !hex || !num || tag.len() < 2 || !tag.starts_with('v') {
        return None;
    }
    let n: u32 = n.parse().ok()?;
    Some(if n == 0 && !dirty {
        tag.to_string()
    } else {
        let dirty = if dirty { "-dirty" } else { "" };
        format!("{tag}+{n} · {sha}{dirty}")
    })
}

#[cfg(test)]
mod tests {
    use super::display_version;

    const PKG: &str = "2026.10.18";

    #[test]
    fn a_release_tag_build_shows_the_tag() {
        assert_eq!(
            display_version(Some("v2026.10.18-0-g739aba4"), PKG),
            "v2026.10.18"
        );
        assert_eq!(
            display_version(Some("v2026.10.18-0-g739aba4\n"), PKG),
            "v2026.10.18"
        );
        // The tag wins over the Cargo.toml version (a dev build after the
        // release PR, before the tag, still counts from the last tag).
        assert_eq!(
            display_version(Some("v2026.10.17-0-g1234567"), PKG),
            "v2026.10.17"
        );
    }

    #[test]
    fn other_builds_show_commits_since_the_tag_and_the_sha() {
        assert_eq!(
            display_version(Some("v2026.10.18-37-g739aba4"), PKG),
            "v2026.10.18+37 · 739aba4"
        );
        assert_eq!(
            display_version(Some("v2026.10.18-rc1-2-gabc1234"), PKG),
            "v2026.10.18-rc1+2 · abc1234"
        );
    }

    #[test]
    fn a_dirty_tree_says_so() {
        assert_eq!(
            display_version(Some("v2026.10.18-37-g739aba4-dirty"), PKG),
            "v2026.10.18+37 · 739aba4-dirty"
        );
        assert_eq!(
            display_version(Some("v2026.10.18-0-g739aba4-dirty"), PKG),
            "v2026.10.18+0 · 739aba4-dirty"
        );
    }

    #[test]
    fn without_git_the_cargo_version() {
        for d in [
            None,
            Some(""),
            Some("garbage"),
            Some("739aba4"),
            Some("v2026.10.18-x-g739aba4"),
            Some("v2026.10.18-3-739aba4"),
            Some("v2026.10.18-3-gxyz"),
            Some("2026.10.18-3-g739aba4"),
            Some("v-3-g739aba4"),
        ] {
            assert_eq!(display_version(d, PKG), "v2026.10.18", "{d:?}");
        }
    }

    #[test]
    fn the_stamped_constant_has_that_shape() {
        let v = crate::BUILD_VERSION;
        assert!(v.starts_with('v'), "{v}");
        if let Some((head, tail)) = v.split_once(" · ") {
            let sha = tail.strip_suffix("-dirty").unwrap_or(tail);
            assert!(
                sha.len() >= 7 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
                "{v}"
            );
            let (_, n) = head.rsplit_once('+').expect(v);
            assert!(n.parse::<u32>().is_ok(), "{v}");
        }
    }
}
