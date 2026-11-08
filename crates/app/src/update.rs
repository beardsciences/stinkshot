//! The ONLY network code in Stinkshot. Runs only when `config.privacy.check_for_updates`
//! is on, at most once per 24 h, on a worker thread. Never sends captures or anything about
//! the user: every request is a plain HTTPS GET (User-Agent "Stinkshot/<version>", no
//! cookies, no query strings). Never downloads binaries: the toast and tray item link to the
//! release page.
//!
//! Flow (docs/notes/release.md):
//! 1. GET the Forgejo "latest release" API for its `tag_name`. Untrusted: it only decides
//!    whether to look further. A tag that is not newer ends the check (saves two requests).
//! 2. GET `stinkshot-release.json` and its minisign signature `stinkshot-release.json.minisig`
//!    from that release's downloads, at URLs built from the fixed base below (never from URLs
//!    in the API answer).
//! 3. Verify the signature with the public key embedded at build time
//!    (`assets/release-key.pub`, prehashed Ed25519 only). Only the verified manifest's version,
//!    tag and page are trusted; a missing or bad signature is an error and nothing is shown.

use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;

pub const RELEASES_API: &str =
    "https://code.mccullough.dev/api/v1/repos/beard/stinkshot/releases/latest";
/// `<base>/<tag>/<file>` is a release asset.
pub const DOWNLOAD_BASE: &str = "https://code.mccullough.dev/beard/stinkshot/releases/download";
/// A manifest's `page` must start with this, or the default release page is used instead.
pub const PROJECT_URL: &str = "https://code.mccullough.dev/beard/stinkshot/";
pub const MANIFEST_NAME: &str = "stinkshot-release.json";
pub const SIGNATURE_NAME: &str = "stinkshot-release.json.minisig";
/// The release signing public key (minisign format). The secret key never enters the repo.
pub const RELEASE_PUBLIC_KEY: &str = include_str!("../assets/release-key.pub");
/// Error text prefix for signature failures, so the shell can say so instead of "offline".
pub const UNTRUSTED: &str = "release manifest failed verification";

const MAX_API_BYTES: u64 = 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 4 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    pub url: String,
}

/// `stinkshot-release.json`, written and signed by `scripts/release.ps1`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Manifest {
    pub schema: u32,
    pub product: String,
    pub version: String,
    pub tag: String,
    #[serde(default)]
    pub released: String,
    #[serde(default)]
    pub page: String,
    #[serde(default)]
    pub files: Vec<ManifestFile>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ManifestFile {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    pub sha256: String,
}

/// Some(release) if a verified release is newer than the running version.
pub fn check() -> anyhow::Result<Option<Release>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(format!("Stinkshot/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    // Forgejo answers 404 when the repository has no published release yet: nothing newer
    // exists, so this is a successful "up to date", not a failure to retry.
    let Some(api) = get(&agent, RELEASES_API, "application/json", MAX_API_BYTES)? else {
        return Ok(None);
    };
    let body: serde_json::Value =
        serde_json::from_slice(&api).context("update check response was not JSON")?;
    let tag = latest_tag(&body)?;
    if !is_newer(&tag, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let manifest = get(
        &agent,
        &asset_url(&tag, MANIFEST_NAME),
        "application/json",
        MAX_MANIFEST_BYTES,
    )?
    .ok_or_else(|| anyhow!("{UNTRUSTED}: release {tag} has no {MANIFEST_NAME}"))?;
    let signature = get(
        &agent,
        &asset_url(&tag, SIGNATURE_NAME),
        "text/plain",
        MAX_SIGNATURE_BYTES,
    )?
    .ok_or_else(|| anyhow!("{UNTRUSTED}: release {tag} has no {SIGNATURE_NAME}"))?;
    let signature =
        String::from_utf8(signature).map_err(|_| anyhow!("{UNTRUSTED}: signature is not text"))?;
    let m = verify_manifest(&manifest, &signature, RELEASE_PUBLIC_KEY)?;
    if m.tag != tag {
        bail!("{UNTRUSTED}: signed tag {} does not match {tag}", m.tag);
    }
    Ok(release_if_newer(&m, env!("CARGO_PKG_VERSION")))
}

/// GET with a size cap. Ok(None) on 404.
fn get(
    agent: &ureq::Agent,
    url: &str,
    accept: &str,
    limit: u64,
) -> anyhow::Result<Option<Vec<u8>>> {
    let mut resp = match agent.get(url).header("Accept", accept).call() {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(anyhow::Error::new(e).context("update check request failed")),
    };
    let bytes = resp
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .context("update check response could not be read")?;
    Ok(Some(bytes))
}

fn asset_url(tag: &str, name: &str) -> String {
    format!("{DOWNLOAD_BASE}/{tag}/{name}")
}

/// `tag_name` of a Forgejo/Gitea release object, restricted to characters that are safe in a
/// URL path segment (the tag is put into the asset URLs).
pub fn latest_tag(v: &serde_json::Value) -> anyhow::Result<String> {
    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| anyhow!("release has no tag_name"))?;
    if !is_safe_tag(tag) {
        bail!("release tag has unexpected characters");
    }
    Ok(tag.to_string())
}

fn is_safe_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 64
        && !tag.starts_with('.')
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

/// Check `signature` (minisign, prehashed Ed25519) over the exact manifest bytes with
/// `public_key` (minisign `.pub` file text), then parse and sanity-check the manifest.
pub fn verify_manifest(
    manifest: &[u8],
    signature: &str,
    public_key: &str,
) -> anyhow::Result<Manifest> {
    let pk = minisign_verify::PublicKey::decode(public_key.trim())
        .map_err(|e| anyhow!("embedded release key is invalid: {e}"))?;
    let sig = minisign_verify::Signature::decode(signature.trim())
        .map_err(|e| anyhow!("{UNTRUSTED}: bad signature file: {e}"))?;
    pk.verify(manifest, &sig, false)
        .map_err(|e| anyhow!("{UNTRUSTED}: {e}"))?;
    let m: Manifest = serde_json::from_slice(manifest)
        .map_err(|e| anyhow!("{UNTRUSTED}: signed manifest is not valid: {e}"))?;
    if m.schema != 1 {
        bail!("{UNTRUSTED}: unknown manifest schema {}", m.schema);
    }
    if m.product != "stinkshot" {
        bail!("{UNTRUSTED}: manifest is for {:?}", m.product);
    }
    if !is_safe_tag(&m.tag) || m.version.trim().is_empty() {
        bail!("{UNTRUSTED}: manifest has no usable version");
    }
    if parse_version(&m.version) != parse_version(&m.tag) {
        bail!(
            "{UNTRUSTED}: manifest version {} does not match tag {}",
            m.version,
            m.tag
        );
    }
    Ok(m)
}

/// The verified manifest as a `Release` if it is newer than `current`.
pub fn release_if_newer(m: &Manifest, current: &str) -> Option<Release> {
    if !is_newer(&m.version, current) {
        return None;
    }
    let url = if m.page.starts_with(PROJECT_URL) && !m.page.contains(char::is_whitespace) {
        m.page.clone()
    } else {
        format!("{PROJECT_URL}releases/tag/{}", m.tag)
    };
    Some(Release {
        tag: m.tag.clone(),
        url,
    })
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

    // Signed with a throwaway test key (testdata/update/test-key.pub; its secret key was
    // discarded). The files are marked binary in .gitattributes so line endings never change.
    const FIXTURE: &[u8] = include_bytes!("../testdata/update/stinkshot-release.json");
    const FIXTURE_SIG: &str = include_str!("../testdata/update/stinkshot-release.json.minisig");
    const TEST_KEY: &str = include_str!("../testdata/update/test-key.pub");

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
    fn parses_latest_tag() {
        let v = serde_json::json!({"tag_name": "v0.2.0", "html_url": "https://example.test/r/v0.2.0", "id": 4});
        assert_eq!(latest_tag(&v).unwrap(), "v0.2.0");
        assert!(latest_tag(&serde_json::json!({"name": "x"})).is_err());
        assert!(latest_tag(&serde_json::json!({"tag_name": ""})).is_err());
        // The tag goes into a URL path: no separators, queries or traversal.
        for bad in ["v1/../../x", "v1?x=1", "..", "v1 2", "v1#a", "v1%2f"] {
            assert!(
                latest_tag(&serde_json::json!({ "tag_name": bad })).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn embedded_release_key_parses() {
        assert!(minisign_verify::PublicKey::decode(RELEASE_PUBLIC_KEY.trim()).is_ok());
        // The test key must never be the release key.
        assert_ne!(RELEASE_PUBLIC_KEY.trim(), TEST_KEY.trim());
    }

    #[test]
    fn signed_fixture_verifies() {
        let m = verify_manifest(FIXTURE, FIXTURE_SIG, TEST_KEY).unwrap();
        assert_eq!(m.version, "9.9.9");
        assert_eq!(m.tag, "v9.9.9");
        assert_eq!(m.files.len(), 1);
        let r = release_if_newer(&m, "0.1.0").unwrap();
        assert_eq!(r.tag, "v9.9.9");
        assert_eq!(
            r.url,
            "https://code.mccullough.dev/beard/stinkshot/releases/tag/v9.9.9"
        );
        assert!(release_if_newer(&m, "9.9.9").is_none());
        assert!(release_if_newer(&m, "10.0.0").is_none());
    }

    #[test]
    fn tampered_manifest_is_rejected() {
        let mut bytes = FIXTURE.to_vec();
        let at = bytes
            .windows(5)
            .position(|w| w == b"9.9.9")
            .expect("fixture has a version");
        bytes[at] = b'8';
        let e = verify_manifest(&bytes, FIXTURE_SIG, TEST_KEY).unwrap_err();
        assert!(format!("{e:#}").starts_with(UNTRUSTED), "{e:#}");
        // Line-ending conversion alone must also fail: bytes are signed as published.
        let crlf = String::from_utf8(FIXTURE.to_vec())
            .unwrap()
            .replace('\n', "\r\n");
        assert!(verify_manifest(crlf.as_bytes(), FIXTURE_SIG, TEST_KEY).is_err());
    }

    #[test]
    fn wrong_key_is_rejected() {
        let e = verify_manifest(FIXTURE, FIXTURE_SIG, RELEASE_PUBLIC_KEY).unwrap_err();
        assert!(format!("{e:#}").starts_with(UNTRUSTED), "{e:#}");
    }

    #[test]
    fn tampered_trusted_comment_is_rejected() {
        let sig = FIXTURE_SIG.replace("test fixture", "test fixturE");
        assert_ne!(sig, FIXTURE_SIG);
        assert!(verify_manifest(FIXTURE, &sig, TEST_KEY).is_err());
    }

    #[test]
    fn garbage_signature_is_rejected() {
        assert!(verify_manifest(FIXTURE, "", TEST_KEY).is_err());
        assert!(verify_manifest(FIXTURE, "<html>404</html>", TEST_KEY).is_err());
    }

    /// `STINKSHOT_RELEASE_DIR=dist\0.1.0 cargo test -p stinkshot -- --ignored release_dir`:
    /// the manifest `scripts/release.ps1` signed passes the app's own check with the embedded
    /// key, and every file it lists matches on disk.
    #[test]
    #[ignore = "needs a release folder from scripts/release.ps1"]
    fn release_dir_verifies_with_embedded_key() {
        let dir = std::path::PathBuf::from(
            std::env::var("STINKSHOT_RELEASE_DIR").expect("set STINKSHOT_RELEASE_DIR"),
        );
        let manifest = std::fs::read(dir.join(MANIFEST_NAME)).unwrap();
        let sig = std::fs::read_to_string(dir.join(SIGNATURE_NAME)).unwrap();
        let m = verify_manifest(&manifest, &sig, RELEASE_PUBLIC_KEY).unwrap();
        assert!(!m.files.is_empty());
        for f in &m.files {
            let meta = std::fs::metadata(dir.join(&f.name)).unwrap();
            assert_eq!(meta.len(), f.size, "{}", f.name);
            assert_eq!(f.sha256.len(), 64, "{}", f.name);
        }
    }

    /// Real network: the live server answers without error (404 before the first release).
    #[test]
    #[ignore = "network"]
    fn live_check_answers() {
        let r = check();
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn page_outside_the_project_is_replaced() {
        let m = Manifest {
            schema: 1,
            product: "stinkshot".into(),
            version: "1.2.3".into(),
            tag: "v1.2.3".into(),
            released: String::new(),
            page: "https://evil.example/stinkshot".into(),
            files: vec![],
        };
        let r = release_if_newer(&m, "1.0.0").unwrap();
        assert_eq!(
            r.url,
            "https://code.mccullough.dev/beard/stinkshot/releases/tag/v1.2.3"
        );
    }
}
