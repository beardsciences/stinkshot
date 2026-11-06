//! The ONLY network code in StinkShot. Runs only when `config.privacy.check_for_updates`
//! is on, at most once per 24 h, on a worker thread. GETs the latest release tag from the
//! Forgejo API and compares with CARGO_PKG_VERSION. Sends no identifying data beyond what
//! any HTTPS GET carries (User-Agent "StinkShot/<version>"). Never downloads binaries:
//! the toast links to the release page.

pub const RELEASES_API: &str = "https://code.mccullough.dev/api/v1/repos/beard/stinkshot/releases/latest";

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    pub url: String,
}

/// Some(release) if newer than the running version.
pub fn check() -> anyhow::Result<Option<Release>> {
    todo!("update::check")
}

/// Semver-ish compare of "v0.2.0" vs "0.1.3"; true if `remote` is newer.
pub fn is_newer(remote: &str, current: &str) -> bool {
    let _ = (remote, current);
    todo!("update::is_newer")
}
