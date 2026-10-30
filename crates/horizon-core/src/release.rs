//! Which Horizon release this build is (docs/versioning.md). Not Minecraft's version: that is
//! [`crate::version`], generated from the game's own files.

/// The release's version (`0.2.3`, no `v`): release.yml builds every release with `HORIZON_VERSION`
/// set from its tag, so there are no version-bump commits. None for a build from source.
pub const RELEASE: Option<&str> = match option_env!("HORIZON_VERSION") {
    Some(version) if !version.is_empty() => Some(version),
    _ => None,
};

/// [`RELEASE`], or this crate's Cargo.toml version for a build from source: what `--version`, user
/// agents and reports say.
pub const VERSION: &str = match RELEASE {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// A nightly's date and commit (`2026-10-28 0c02067`): nightly-build.yml builds the opt-in Nightly
/// channel with `HORIZON_NIGHTLY` set (docs/release-flow.md). None for a release or a build from source.
pub const NIGHTLY: Option<&str> = match option_env!("HORIZON_NIGHTLY") {
    Some(nightly) if !nightly.is_empty() => Some(nightly),
    _ => None,
};

/// How a nightly names itself where players read it: `Horizon 2026.10.28`. None for any other build.
pub fn nightly_label() -> Option<String> {
    NIGHTLY.map(label)
}

/// [`nightly_label`] with its commit, for crash reports, which must say the exact build:
/// `Horizon 2026.10.28 (0c02067)`.
pub fn nightly_report_label() -> Option<String> {
    NIGHTLY.map(report_label)
}

/// A nightly's version as the launcher installs it (`nightly-2026-10-28-0c02067`). Updates rank and
/// verify this one, so it stays as it is however the build is shown.
pub fn nightly_version() -> Option<String> {
    NIGHTLY.map(|n| format!("nightly-{}", n.replace(' ', "-")))
}

fn label(nightly: &str) -> String {
    let date = nightly.split(' ').next().unwrap_or(nightly);
    format!("Horizon {}", date.replace('-', "."))
}

fn report_label(nightly: &str) -> String {
    match nightly.split_once(' ') {
        Some((_, commit)) => format!("{} ({commit})", label(nightly)),
        None => label(nightly),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_nightly_reads_as_its_date() {
        assert_eq!(super::label("2026-10-28 0c02067"), "Horizon 2026.10.28");
        assert_eq!(super::label("2026-10-28"), "Horizon 2026.10.28");
        assert_eq!(super::nightly_label().is_some(), super::NIGHTLY.is_some());
    }

    #[test]
    fn a_crash_report_also_names_the_commit() {
        assert_eq!(
            super::report_label("2026-10-28 0c02067"),
            "Horizon 2026.10.28 (0c02067)"
        );
        assert_eq!(super::report_label("2026-10-28"), "Horizon 2026.10.28");
    }

    #[test]
    fn the_version_is_the_release_or_the_crate() {
        assert_eq!(
            super::RELEASE,
            option_env!("HORIZON_VERSION").filter(|v| !v.is_empty())
        );
        assert_eq!(
            super::VERSION,
            super::RELEASE.unwrap_or(env!("CARGO_PKG_VERSION"))
        );
        assert!(!super::VERSION.starts_with('v'));
    }
}
