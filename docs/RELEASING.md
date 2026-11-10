# Releasing Stinkshot

Maintainer procedure. Nothing here runs automatically: every publish step is a command you
type. Background and design decisions: `docs/notes/release.md`.

## One-time setup

### Tools

`scripts/release.ps1` looks for each tool as a parameter, an environment variable, on PATH, then
under `C:\dev\stinkshot-bench\tools` (where the dev box has them, all portable, nothing
installed system-wide):

| Tool | Version | Where it came from | Override |
|---|---|---|---|
| Inno Setup | 7.1.1 | https://jrsoftware.org/isdl.php (installed with `/CURRENTUSER /PORTABLE=1 /DIR=...`) | `-Iscc`, `STINKSHOT_ISCC` |
| minisign | 0.12 | https://github.com/jedisct1/minisign/releases (signature checked against the author's key) | `-Minisign`, `STINKSHOT_MINISIGN` |
| cargo-cyclonedx | 0.5.10 | `cargo install cargo-cyclonedx --locked --root C:\dev\stinkshot-bench\tools\cargo` | `-Cyclonedx`, `STINKSHOT_CYCLONEDX` |
| signtool | Windows SDK 10.0.26100 | already installed with the Build Tools | `-SignTool` |

### Release signing key (update manifest)

The keypair lives outside the repo:

- `C:\Users\beard\.stinkshot-release\stinkshot-release.key`: secret key. Never commit it.
- `C:\Users\beard\.stinkshot-release\stinkshot-release.pub`: public key, also committed as
  `crates/app/assets/release-key.pub` and compiled into the app.

Key id `A2920B49A99E16A0`, public key
`RWSgFp6pSQuSoo99V89nCiRKQ+SToPCxbnLeyViYxFzzfnbNKV77DFH3`.

The secret key was generated without a password so the release script can run unattended.
Recommended once: add one (minisign then asks for it on every release):

```
C:\dev\stinkshot-bench\tools\minisign\minisign-win64\x86_64\minisign.exe -C -s C:\Users\beard\.stinkshot-release\stinkshot-release.key
```

**Back it up now.** Copy the whole `.stinkshot-release` folder to two places that are not this
PC, for example an encrypted USB stick kept offline and a password manager attachment
(the `.key` file is about 400 bytes of text). Not to the NAS share that syncs captures, not to
any cloud folder in plain form.

Planned rotation (old key still held): ship one release signed with the old key that embeds
the new public key, then sign with the new key from the next release on.

If the secret key is lost, nothing can be signed with it again, so installed copies can never
trust a new release: users have to download the next version by hand. This is why the backup
matters.

If it leaks, anyone can make installed copies show a fake "update available". Update Now only
downloads from stinkshot.com/download and checks the file against the signed hash, so a leaked key
alone cannot push an installer; a leaked key plus write access to the site could.
Rotate as above at once (the leaked key still signs that one release) and say so in its notes.

### Code signing (optional, Authenticode)

Unsigned builds work; Windows SmartScreen shows "unrecognized app" on first run of an unsigned
installer until it builds reputation. Two ways to sign, both already wired into the script:

**SignPath Foundation (free for open source).** You have to apply yourself:

1. Make the repository public (it must be reachable from the internet).
2. Apply at https://signpath.org (Open Source "Apply" form). They review the project and its
   license (GPL-3.0 qualifies) and ask for a code signing policy page in the repo or on the
   site; `SECURITY.md` covers the policy basics.
3. Check their current build requirement before relying on local builds: the Foundation
   certificate is normally issued only for artifacts built on a CI system they can verify
   (GitHub Actions, AppVeyor, Azure DevOps and similar). Forgejo Actions may not be on their
   list; if not, the usual route is a GitHub mirror with a GitHub Actions workflow that runs
   `scripts/release.ps1`. Not verified here.
4. Once approved you get an organization id, a project slug, a signing policy slug
   (`release-signing`), and an API token for a CI user. Then:

```
Install-Module SignPath -Scope CurrentUser
$env:SIGNPATH_API_TOKEN = "<token>"
.\scripts\release.ps1 -Sign signpath -SignPathOrganizationId <org id> -SignPathProjectSlug stinkshot -SignPathPolicySlug release-signing
```

The script submits `stinkshot.exe` before it is zipped and packed, then the finished
`Stinkshot-<version>-x64-setup.exe`, waits for each signing request, and replaces the file in
place. The signed files are the ones in `dist\<version>\` (the zip contains the signed exe).
The installer's uninstaller stays unsigned in this mode (Inno builds it at install time from the
setup exe; SignPath can't re-sign it). Hashes and the update manifest are computed after
signing, so they always match what you upload.

**Your own certificate.** Import the certificate into `Cert:\CurrentUser\My` and pass its
thumbprint; signtool signs the exe, the installer and the uninstaller, with an RFC 3161
timestamp:

```
.\scripts\release.ps1 -Sign signtool -CertThumbprint <sha1 thumbprint>
```

## Each release

1. Update `CHANGELOG.md`: move the `[Unreleased]` entries into `## [x.y.z] - yyyy-mm-dd` and fix
   the compare links at the bottom.
2. Bump `version` under `[workspace.package]` in `Cargo.toml`, run `cargo check` so
   `Cargo.lock` follows, and in `packaging/winget/...` only if the draft format changed (the
   script fills the version and hash itself).
3. Commit: `git commit -am "Release x.y.z"`.
4. Build:

```
powershell -ExecutionPolicy Bypass -File scripts\release.ps1
```

   It refuses a dirty tree, checks fmt, clippy and tests, builds with `--locked` into
   `target-dist\` (never `target\`), checks the exe's version resource, and writes to
   `dist\<version>\`:

   - `Stinkshot-<version>-x64-setup.exe`: per-user installer
   - `Stinkshot-<version>-x64-portable.zip`: `stinkshot.exe`, `LICENSE.txt`, `README.md`
   - `Stinkshot-<version>.cdx.json`: CycloneDX 1.5 SBOM of the app and every dependency
   - `SHA256SUMS`, `SHA256SUMS.minisig`
   - `stinkshot-release.json`, `stinkshot-release.json.minisig`: what the update check reads
   - `RELEASE_NOTES.md`: the release description (from the changelog and the template)
   - `winget\manifests\s\Stinkshot\Stinkshot\<version>\`: filled winget manifest

   About 4 minutes with warm caches on the dev box (`-SkipTests` skips fmt, clippy and tests).

5. Smoke test with the test installer the script also builds,
   `dist\<version>\test\Stinkshot-Test-<version>-x64-setup.exe` (never upload it). It has its
   own AppId, Apps entry, folder and Start menu shortcut, so it cannot take over or remove a
   real install's. Run it under a test profile so the Run value, saved PrintScreen value and
   cache are separate too. The PrintScreen setting itself is shared: a test profile never hands
   it back to Snipping Tool while your own copy holds the takeover, but never plant
   `PrevPrintScreenSnipping-*` values by hand while your copy runs. Only Windows Sandbox or a VM
   isolates everything (needed for an upgrade over a running copy).

```
$env:STINKSHOT_PROFILE = "release-test"
.\dist\<version>\test\Stinkshot-Test-<version>-x64-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
& "$env:LOCALAPPDATA\Programs\Stinkshot Test\unins000.exe" /VERYSILENT
Remove-Item Env:STINKSHOT_PROFILE
```

## Publishing (manual, in this order)

First release only: create the repository. On https://code.mccullough.dev, New Repository,
owner `christian`, name `stinkshot`, no template files (the repo already has README, LICENSE and
.gitignore). Then:

```
git remote add origin https://code.mccullough.dev/christian/stinkshot.git
git push -u origin main
```

Every release:

1. Tag and push:

```
git tag -a vX.Y.Z -m "Stinkshot X.Y.Z"
git push origin main
git push origin vX.Y.Z
```

2. On the repository, Releases > New Release, pick tag `vX.Y.Z`, title `Stinkshot X.Y.Z`,
   paste `dist\X.Y.Z\RELEASE_NOTES.md` as the description, and attach every file in
   `dist\X.Y.Z\` except `RELEASE_NOTES.md` and the `winget` folder. The update check needs
   `stinkshot-release.json` and `stinkshot-release.json.minisig` attached with exactly those
   names. Publish (not draft, not pre-release: the API's "latest" skips both).

   Or with the API (token from Settings > Applications, scope `write:repository`):

```
$h = @{ Authorization = "token <forgejo token>" }
$r = Invoke-RestMethod -Method Post -Headers $h -ContentType "application/json" -Uri https://code.mccullough.dev/api/v1/repos/christian/stinkshot/releases -Body (@{ tag_name = "vX.Y.Z"; name = "Stinkshot X.Y.Z"; body = (Get-Content -Raw dist\X.Y.Z\RELEASE_NOTES.md) } | ConvertTo-Json)
Get-ChildItem dist\X.Y.Z -File | Where-Object Name -ne RELEASE_NOTES.md | ForEach-Object { Invoke-RestMethod -Method Post -Headers $h -Uri "https://code.mccullough.dev/api/v1/repos/christian/stinkshot/releases/$($r.id)/assets?name=$($_.Name)" -Form @{ attachment = $_ } }
```

   (`-Form` needs PowerShell 7.)

3. Check the update path end to end from a copy with update checks on (Settings > Privacy >
   Check Now on an older build), or:

```
curl.exe -sSfLO https://code.mccullough.dev/christian/stinkshot/releases/download/vX.Y.Z/stinkshot-release.json
curl.exe -sSfLO https://code.mccullough.dev/christian/stinkshot/releases/download/vX.Y.Z/stinkshot-release.json.minisig
minisign -Vm stinkshot-release.json -p crates\app\assets\release-key.pub
```

   code.mccullough.dev is behind Anubis: confirm the downloads above return the files, not a
   challenge page, for non-browser clients (winget and the app send non-browser User-Agents).

4. winget (after the release is public and the download URL works):

```
winget validate --manifest dist\X.Y.Z\winget\manifests\s\Stinkshot\Stinkshot\X.Y.Z
```

   Fork https://github.com/microsoft/winget-pkgs, copy that folder to the same path in the
   fork, commit `New package: Stinkshot.Stinkshot version X.Y.Z`, open a pull request. Or with
   wingetcreate (`winget install Microsoft.WingetCreate`):

```
wingetcreate submit --token <github token> dist\X.Y.Z\winget\manifests\s\Stinkshot\Stinkshot\X.Y.Z
```

   Later versions: `wingetcreate update Stinkshot.Stinkshot --version X.Y.Z --urls <setup url> --submit`.
   The package identifier `Stinkshot.Stinkshot` is a choice, not a requirement; change it in
   `packaging/winget` before the first submission if you prefer another publisher name.
