//! The ONLY network code in Stinkshot. Runs only when `config.privacy.check_for_updates`
//! is on, at most once per 24 h, on a worker thread. GETs the latest release tag from the
//! Forgejo API and compares with CARGO_PKG_VERSION. Sends no identifying data beyond what
//! any HTTPS GET carries (User-Agent "Stinkshot/<version>"). Never downloads binaries:
//! the toast links to the release page.

use std::time::Duration;

use anyhow::{Context, anyhow};

pub const RELEASES_API: &str =
    "https://code.mccullough.dev/api/v1/repos/beard/stinkshot/releases/latest";

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    pub url: String,
}

/// Some(release) if newer than the running version.
pub fn check() -> anyhow::Result<Option<Release>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(format!("Stinkshot/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut resp = agent
        .get(RELEASES_API)
        .header("Accept", "application/json")
        .call()
        .context("update check request failed")?;
    let body: serde_json::Value = resp
        .body_mut()
        .read_json()
        .context("update check response was not JSON")?;
    let release = parse_release(&body)?;
    Ok(if is_newer(&release.tag, env!("CARGO_PKG_VERSION")) {
        Some(release)
    } else {
        None
    })
}

/// Pull `tag_name` and `html_url` out of a Forgejo/Gitea release object.
pub fn parse_release(v: &serde_json::Value) -> anyhow::Result<Release> {
    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| anyhow!("release has no tag_name"))?
        .to_string();
    let url = v
        .get("html_url")
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_string();
    Ok(Release { tag, url })
}

/// Semver-ish compare of "v0.2.0" vs "0.1.3"; true if `remote` is newer.
pub fn is_newer(remote: &str, current: &str) -> bool {
    let (r, r_pre) = parse_version(remote);
    let (c, c_pre) = parse_version(current);
    match r.cmp(&c) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        // Same numbers: a release beats a pre-release of itself, never the other way round.
        std::cmp::Ordering::Equal => c_pre && !r_pre,
    }
}

fn parse_version(s: &str) -> (Vec<u64>, bool) {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let end = s.find(['-', '+']).unwrap_or(s.len());
    let core = &s[..end];
    let pre = s[end..].starts_with('-');
    let mut nums: Vec<u64> = core
        .split('.')
        .map(|p| p.trim().parse::<u64>().unwrap_or(0))
        .collect();
    while nums.len() < 3 {
        nums.push(0);
    }
    // Trailing zeros must not matter ("1.0" == "1.0.0").
    while nums.len() > 3 && nums.last() == Some(&0) {
        nums.pop();
    }
    (nums, pre)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_versions() {
        assert!(is_newer("v0.2.0", "0.1.3"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("1.0", "1.0.0"));
    }

    #[test]
    fn prerelease_ordering() {
        assert!(is_newer("0.2.0", "0.2.0-beta.1"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0"));
        assert!(!is_newer("0.2.0-beta.2", "0.2.0-beta.1"));
        assert!(is_newer("0.3.0-rc1", "0.2.0"));
    }

    #[test]
    fn garbage_is_never_newer() {
        assert!(!is_newer("", "0.1.0"));
        assert!(!is_newer("latest", "0.1.0"));
    }

    #[test]
    fn parses_release_json() {
        let v = serde_json::json!({"tag_name": "v0.2.0", "html_url": "https://example.test/r/v0.2.0", "id": 4});
        let r = parse_release(&v).unwrap();
        assert_eq!(
            r,
            Release {
                tag: "v0.2.0".into(),
                url: "https://example.test/r/v0.2.0".into()
            }
        );
        assert!(parse_release(&serde_json::json!({"name": "x"})).is_err());
        assert!(parse_release(&serde_json::json!({"tag_name": ""})).is_err());
    }
}
