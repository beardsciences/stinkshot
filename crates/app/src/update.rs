//! The ONLY network code in Stinkshot. Runs only when `config.privacy.check_for_updates`
//! is on, at most once per 24 h, on a worker thread. Never sends captures or anything about
//! the user: every request is a plain HTTPS GET (User-Agent "Stinkshot/<version>", no
//! cookies, no query strings).
//!
//! Update Now (`download_installer`, only when the user clicks it): downloads the setup.exe
//! the signed manifest lists from stinkshot.com, checks its size and SHA-256 against that
//! manifest, and runs it silently (`run_installer`). Anything that does not match is deleted
//! and the release page is opened instead. Copies the setup did not install (portable, dev
//! builds) always get the release page.
//!
//! Flow (docs/notes/release.md):
//! 1. GET `stinkshot-release.json` and its minisign signature `stinkshot-release.json.minisig`
//!    from the fixed base on stinkshot.com (a 404 means nothing is published: up to date).
//!    stinkshot.com is used so the code host can move without breaking installed copies.
//! 2. Verify the signature with the public key embedded at build time
//!    (`assets/release-key.pub`, prehashed Ed25519 only). Only the verified manifest's version,
//!    tag, page and file hashes are trusted; a missing or bad signature is an error and
//!    nothing is shown.

use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;

/// `<base>/<file>` is the latest release's signed info.
pub const RELEASE_BASE: &str = "https://stinkshot.com/release";
/// A manifest's `page` must start with this, or `DEFAULT_PAGE` is used instead.
pub const PROJECT_URL: &str = "https://stinkshot.com/";
pub const DEFAULT_PAGE: &str = "https://stinkshot.com/download";
/// `<base>/<setup file name>` is where the installer the manifest lists is downloaded from.
pub const DOWNLOAD_BASE: &str = "https://stinkshot.com/download";
pub const MANIFEST_NAME: &str = "stinkshot-release.json";
pub const SIGNATURE_NAME: &str = "stinkshot-release.json.minisig";
/// The release signing public key (minisign format). The secret key never enters the repo.
pub const RELEASE_PUBLIC_KEY: &str = include_str!("../assets/release-key.pub");
/// Error text prefix for signature failures, so the shell can say so instead of "offline".
pub const UNTRUSTED: &str = "release manifest failed verification";
/// Error text prefix for a newer release without its signed manifest or signature attached
/// (not uploaded yet): not a verification failure.
pub const UNSIGNED: &str = "release has no signed release info";

/// Debug builds only (compiled out of release): point the update check and download at a
/// local test server and trust a test key, for the end-to-end update test.
/// `STINKSHOT_UPDATE_BASE` replaces both `RELEASE_BASE` and `DOWNLOAD_BASE`;
/// `STINKSHOT_UPDATE_KEY` is minisign public key text.
fn dev_override(var: &str) -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var(var).ok().filter(|v| !v.trim().is_empty())
    } else {
        None
    }
}

fn release_base() -> String {
    dev_override("STINKSHOT_UPDATE_BASE").unwrap_or_else(|| RELEASE_BASE.to_string())
}

fn download_base() -> String {
    dev_override("STINKSHOT_UPDATE_BASE").unwrap_or_else(|| DOWNLOAD_BASE.to_string())
}

fn release_key() -> String {
    dev_override("STINKSHOT_UPDATE_KEY").unwrap_or_else(|| RELEASE_PUBLIC_KEY.to_string())
}

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 4 * 1024;
const MAX_INSTALLER_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    pub url: String,
    /// This architecture's setup.exe from the signed manifest, if it lists one.
    pub installer: Option<Installer>,
}

/// A setup.exe the signed manifest vouches for.
#[derive(Clone, Debug, PartialEq)]
pub struct Installer {
    pub name: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

/// The setup file name suffix for the architecture this build runs on.
fn setup_suffix() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "-arm64-setup.exe"
    } else {
        "-x64-setup.exe"
    }
}

/// This architecture's installer from `files`, if one is listed with a usable name and hash.
fn pick_installer(files: &[ManifestFile]) -> Option<Installer> {
    files
        .iter()
        .find(|f| {
            f.name.starts_with("Stinkshot-")
                && f.name.ends_with(setup_suffix())
                && f.name.len() <= 96
                && f.name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
                && f.size > 0
                && f.size <= MAX_INSTALLER_BYTES
                && f.sha256.len() == 64
                && f.sha256.chars().all(|c| c.is_ascii_hexdigit())
        })
        .map(|f| Installer {
            name: f.name.clone(),
            size: f.size,
            sha256: f.sha256.to_ascii_lowercase(),
        })
}

/// Whether the setup installed this copy (its uninstaller sits next to the exe), so running
/// a newer setup upgrades this very copy. Portable and dev builds return false.
pub fn can_self_update() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("unins000.exe").is_file()))
        .unwrap_or(false)
}

/// Download `inst` into `%TEMP%\Stinkshot-update`, checking its size and SHA-256 against the
/// signed manifest as it streams. `progress(done, total)` is called as bytes arrive. Returns
/// the verified file; on any mismatch the partial file is deleted and this is an error.
pub fn download_installer(
    inst: &Installer,
    mut progress: impl FnMut(u64, u64),
) -> anyhow::Result<std::path::PathBuf> {
    use std::io::{Read, Write};

    let dir = std::env::temp_dir().join("Stinkshot-update");
    // Never reuse an old download: every update starts from an empty folder.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).context("update folder could not be created")?;
    let path = dir.join(&inst.name);
    let part = dir.join(format!("{}.part", inst.name));

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(15 * 60)))
        .user_agent(format!("Stinkshot/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let url = format!("{}/{}", download_base(), inst.name);
    let mut resp = agent
        .get(&url)
        .header("Accept", "application/octet-stream")
        .call()
        .map_err(|e| anyhow::Error::new(e).context("update download request failed"))?;
    let mut reader = resp.body_mut().with_config().limit(inst.size + 1).reader();
    let mut file = std::fs::File::create(&part).context("update file could not be created")?;
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    let mut result = || -> anyhow::Result<()> {
        loop {
            let n = reader
                .read(&mut buf)
                .context("update download was cut off")?;
            if n == 0 {
                break;
            }
            done += n as u64;
            if done > inst.size {
                bail!("{UNTRUSTED}: the download is larger than the release info says");
            }
            hash.update(&buf[..n]);
            file.write_all(&buf[..n])
                .context("update file could not be written")?;
            progress(done, inst.size);
        }
        file.flush().context("update file could not be written")?;
        if done != inst.size {
            bail!(
                "update download was cut off ({done} of {} bytes)",
                inst.size
            );
        }
        Ok(())
    };
    let outcome = result();
    drop(file);
    let outcome = outcome.and_then(|()| {
        if hex(hash.finish().as_ref()) != inst.sha256 {
            bail!("{UNTRUSTED}: the downloaded installer does not match the signed hash");
        }
        Ok(())
    });
    if let Err(e) = outcome {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, &path).context("update file could not be renamed")?;
    Ok(path)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Start the verified installer silently. It asks this copy to quit, replaces it, and starts
/// the new version in the tray (packaging/inno/stinkshot.iss, RelaunchAfterSilentUpgrade).
pub fn run_installer(path: &std::path::Path) -> anyhow::Result<()> {
    std::process::Command::new(path)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
        .spawn()
        .context("the installer could not be started")?;
    Ok(())
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
    // 404 before the first release is published: nothing newer exists, so this is a
    // successful "up to date", not a failure to retry.
    let Some(manifest) = get(
        &agent,
        &asset_url(MANIFEST_NAME),
        "application/json",
        MAX_MANIFEST_BYTES,
    )?
    else {
        return Ok(None);
    };
    let signature = get(
        &agent,
        &asset_url(SIGNATURE_NAME),
        "text/plain",
        MAX_SIGNATURE_BYTES,
    )?
    .ok_or_else(|| anyhow!("{UNSIGNED}: no {SIGNATURE_NAME} published"))?;
    let signature =
        String::from_utf8(signature).map_err(|_| anyhow!("{UNTRUSTED}: signature is not text"))?;
    let m = verify_manifest(&manifest, &signature, &release_key())?;
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

fn asset_url(name: &str) -> String {
    format!("{}/{name}", release_base())
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
        DEFAULT_PAGE.to_string()
    };
    Some(Release {
        tag: m.tag.clone(),
        url,
        installer: pick_installer(&m.files),
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
        // The fixture's page is on the old code host, outside PROJECT_URL.
        assert_eq!(r.url, DEFAULT_PAGE);
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

    fn file(name: &str, size: u64, sha: &str) -> ManifestFile {
        ManifestFile {
            name: name.into(),
            size,
            sha256: sha.into(),
        }
    }

    #[test]
    fn installer_is_picked_for_this_architecture() {
        let sha = "AB".repeat(32);
        let (arch, other) = if cfg!(target_arch = "aarch64") {
            ("arm64", "x64")
        } else {
            ("x64", "arm64")
        };
        let files = vec![
            file(&format!("Stinkshot-1.2.3-{arch}-portable.zip"), 5, &sha),
            file(&format!("Stinkshot-1.2.3-{other}-setup.exe"), 5, &sha),
            file(&format!("Stinkshot-1.2.3-{arch}-setup.exe"), 7, &sha),
        ];
        let i = pick_installer(&files).unwrap();
        assert_eq!(i.name, format!("Stinkshot-1.2.3-{arch}-setup.exe"));
        assert_eq!(i.size, 7);
        assert_eq!(i.sha256, "ab".repeat(32));
    }

    #[test]
    fn unusable_installer_entries_are_ignored() {
        let sha = "ab".repeat(32);
        let suffix = setup_suffix();
        for f in [
            file(&format!("../Stinkshot-1{suffix}"), 5, &sha),
            file(&format!("Stinkshot-1/x{suffix}"), 5, &sha),
            file(&format!("Other-1{suffix}"), 5, &sha),
            file(&format!("Stinkshot-1{suffix}"), 0, &sha),
            file(&format!("Stinkshot-1{suffix}"), 5, "abc"),
            file(&format!("Stinkshot-1{suffix}"), 5, &"zz".repeat(32)),
            file(
                &format!("Stinkshot-1{suffix}"),
                MAX_INSTALLER_BYTES + 1,
                &sha,
            ),
        ] {
            assert!(pick_installer(std::slice::from_ref(&f)).is_none(), "{f:?}");
        }
    }

    #[test]
    fn hex_is_lowercase() {
        assert_eq!(hex(&[0x00, 0xab, 0xff]), "00abff");
    }

    /// Against a local test server (`STINKSHOT_UPDATE_BASE`, debug builds) serving
    /// `STINKSHOT_UPDATE_FILE`: the right hash is accepted, a wrong hash or size is rejected
    /// and leaves no file behind.
    #[test]
    #[ignore = "needs a local update test server"]
    fn local_download_checks_hash_and_size() {
        let name = std::env::var("STINKSHOT_UPDATE_FILE").expect("set STINKSHOT_UPDATE_FILE");
        let size: u64 = std::env::var("STINKSHOT_UPDATE_SIZE")
            .unwrap()
            .parse()
            .unwrap();
        let sha = std::env::var("STINKSHOT_UPDATE_SHA")
            .unwrap()
            .to_ascii_lowercase();
        let dir = std::env::temp_dir().join("Stinkshot-update");
        let good = Installer {
            name: name.clone(),
            size,
            sha256: sha.clone(),
        };
        let path = download_installer(&good, |_, _| {}).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), size);

        let mut bad_hash = good.clone();
        bad_hash.sha256 = "0".repeat(64);
        let e = download_installer(&bad_hash, |_, _| {}).unwrap_err();
        assert!(format!("{e:#}").starts_with(UNTRUSTED), "{e:#}");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "nothing left behind"
        );

        let mut short = good.clone();
        short.size = size - 1;
        let e = download_installer(&short, |_, _| {}).unwrap_err();
        assert!(format!("{e:#}").starts_with(UNTRUSTED), "{e:#}");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "nothing left behind"
        );

        let mut long = good;
        long.size = size + 1;
        assert!(download_installer(&long, |_, _| {}).is_err());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "nothing left behind"
        );
    }

    /// Real network: downloads the live release's installer and checks it against the signed
    /// manifest, like Update Now does.
    #[test]
    #[ignore = "network"]
    fn live_installer_downloads_and_verifies() {
        let agent: ureq::Agent = ureq::Agent::config_builder().build().into();
        let manifest = get(
            &agent,
            &asset_url(MANIFEST_NAME),
            "application/json",
            MAX_MANIFEST_BYTES,
        )
        .unwrap()
        .unwrap();
        let sig = get(
            &agent,
            &asset_url(SIGNATURE_NAME),
            "text/plain",
            MAX_SIGNATURE_BYTES,
        )
        .unwrap()
        .unwrap();
        let m = verify_manifest(
            &manifest,
            &String::from_utf8(sig).unwrap(),
            RELEASE_PUBLIC_KEY,
        )
        .unwrap();
        let inst = pick_installer(&m.files).expect("manifest lists a setup");
        let mut last = 0;
        let path = download_installer(&inst, |d, _| last = d).unwrap();
        assert_eq!(last, inst.size);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), inst.size);
        let _ = std::fs::remove_file(path);
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
        assert_eq!(r.url, DEFAULT_PAGE);
    }
}
