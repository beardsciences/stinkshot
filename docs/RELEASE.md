# Publishing Stinkshot 0.1.0

Copy-paste steps for tonight, in order. Every command is Windows PowerShell 5.1 (no `&&` or
`||`; type `curl.exe`, not `curl`). Run them from `C:\dev\stinkshot` unless a step says
otherwise. Background, tools and the signing key: `docs/RELEASING.md`.

Nothing here runs by itself: every push and upload is a command you type.

## 0. Before you start

- **Repository owner.** The app is built to check
  `https://code.mccullough.dev/beard/stinkshot` (update check, Settings links, installer
  URLs, winget manifest). On Nov 8 there is no `beard` account or organization on
  code.mccullough.dev; your account is `christian`. Pick one before building:
  - **Keep the code (recommended tonight):** create an organization named `beard`
    (code.mccullough.dev, + > New Organization, name `beard`, visibility Public). Nothing
    else changes.
  - Or move everything to `christian/stinkshot`: replace `beard/stinkshot` in `Cargo.toml`,
    `crates/app/src/update.rs`, `crates/app/src/settings_ui.rs`, `packaging/`,
    `scripts/release.ps1`, `CHANGELOG.md`, `SECURITY.md`, the docs, and re-sign the test
    fixture in `crates/app/testdata/update` with a new throwaway key (the fixture's `page`
    must start with the project URL). Do this before step 2, not after the release.
- **Merge** every track you want in 0.1.0 into `main` and make sure the workspace is clean.
- **Memory.** Other builds and soak tests on the box can starve rustc (the Nov 8 dry run
  failed with "only metadata stub found for `rlib` dependency `core`" at 1.7 GB free). Close
  them, or pass `-Jobs 4` below.

## 1. Changelog

Edit `CHANGELOG.md`:

- Replace the two `TODO:` lines in `## [0.1.0]` with what actually shipped.
- Change `## [0.1.0] - Unreleased` to today's date, for example `## [0.1.0] - 2026-11-08`.

`release.ps1` refuses to build while either is left (unless `-AllowDirty`).

```
git add CHANGELOG.md
git commit -m "Release 0.1.0"
```

## 2. Build (one command)

```
powershell -ExecutionPolicy Bypass -File scripts\release.ps1 -TargetDir C:\dev\stinkshot-bench\target-dist-dryrun -OutDir C:\dev\stinkshot-bench\dist\0.1.0
```

Add `-Jobs 4` if memory is tight. It checks fmt, clippy and tests, builds, and writes to
`C:\dev\stinkshot-bench\dist\0.1.0`:

| File | Upload |
|---|---|
| `Stinkshot-0.1.0-x64-setup.exe` | yes |
| `Stinkshot-0.1.0-x64-portable.zip` | yes |
| `Stinkshot-0.1.0.cdx.json` | yes |
| `SHA256SUMS`, `SHA256SUMS.minisig` | yes |
| `stinkshot-release.json`, `stinkshot-release.json.minisig` | yes (the update check reads these) |
| `RELEASE_NOTES.md` | no: paste it as the release description |
| `test\` | no: smoke-test installer |
| `winget\` | no: for step 7 |

It prints a timing table at the end. The Nov 8 dry run (cold target dir, `-Jobs 4`, other
builds and soak tests running) took 30 minutes: fmt 16 s, clippy 34 s, tests 13.8 min, release
build 13.5 min, zip 6 s, installer 36 s, test installer 33 s, SBOM, sums, manifest and signing
under 10 s together. The commands here reuse that dry run's target dir, which skips most of
the compile time.

## 3. Smoke test

```
powershell -ExecutionPolicy Bypass -File scripts\smoke-test.ps1 -DistDir C:\dev\stinkshot-bench\dist\0.1.0 -TargetDir C:\dev\stinkshot-bench\target-dist-dryrun
```

Runs under the `smoke` profile with the test installer (own AppId, folder and shortcut), so
your running copy and a real install are never touched. A 600x200 color swatch shows at the
top left of the primary monitor for about 4 seconds during the recording check; the screen
must be on. Exit code is the number of failed checks. Add `-Mode portable` to test the zip.

On Nov 8 every check passed in both modes except `record`: with the dev and soak instances
running, Desktop Duplication on the HDR monitor was refused ("too many applications are
duplicating") and WGC timed out; when a session did start, the frames did not show the test
window. If it fails again, close the other Stinkshot instances and rerun; if it still fails,
record something with the installed app by hand before publishing. `-SkipRecord` skips it.

## 4. Create the repository and push (first release only)

On code.mccullough.dev: + > New Repository, owner `beard` (see step 0), name `stinkshot`,
visibility **Public** (the update check and winget download anonymously), no README, license
or .gitignore (the repo has them). Then:

```
git remote add origin https://code.mccullough.dev/beard/stinkshot.git
git push -u origin main
```

## 5. Tag

The tag must point at the commit `release.ps1` built (it prints it first, and
`stinkshot-release.json` has it as `commit`).

```
git tag -a v0.1.0 -m "Stinkshot 0.1.0"
git push origin v0.1.0
```

## 6. Create the release and upload

In the browser: repository > Releases > New Release, tag `v0.1.0`, title `Stinkshot 0.1.0`,
paste `RELEASE_NOTES.md` as the description, attach the seven "yes" files from step 2 with
their names unchanged, then Publish Release. Not draft, not pre-release: the update check asks
for the latest release, which skips both.

Or from PowerShell (token: Settings > Applications > Generate Token, scope
`write:repository`):

```
$tok = Read-Host "Forgejo token"
$dist = "C:\dev\stinkshot-bench\dist\0.1.0"
$api = "https://code.mccullough.dev/api/v1/repos/beard/stinkshot"
$body = @{ tag_name = "v0.1.0"; name = "Stinkshot 0.1.0"; body = (Get-Content -Raw "$dist\RELEASE_NOTES.md"); draft = $false; prerelease = $false } | ConvertTo-Json
$rel = Invoke-RestMethod -Method Post -Uri "$api/releases" -Headers @{ Authorization = "token $tok" } -ContentType "application/json; charset=utf-8" -Body ([Text.Encoding]::UTF8.GetBytes($body))
Get-ChildItem $dist -File | Where-Object { $_.Name -ne "RELEASE_NOTES.md" } | ForEach-Object { curl.exe -sSf -H "Authorization: token $tok" -F "attachment=@$($_.FullName)" "$api/releases/$($rel.id)/assets?name=$($_.Name)" ; "" }
```

### Where the update manifest lives

There is no separate hosting: the manifest is a release asset. An installed copy with update
checks on makes these GETs, in this order (User-Agent `Stinkshot/<version>`):

1. `https://code.mccullough.dev/api/v1/repos/beard/stinkshot/releases/latest` (reads `tag_name`)
2. `https://code.mccullough.dev/beard/stinkshot/releases/download/<tag>/stinkshot-release.json`
3. `https://code.mccullough.dev/beard/stinkshot/releases/download/<tag>/stinkshot-release.json.minisig`

Check them right after publishing:

```
curl.exe -sS -A "Stinkshot/0.1.0" https://code.mccullough.dev/api/v1/repos/beard/stinkshot/releases/latest | Select-String '"tag_name":"v0.1.0"'
curl.exe -sSfLO -A "Stinkshot/0.1.0" https://code.mccullough.dev/beard/stinkshot/releases/download/v0.1.0/stinkshot-release.json
curl.exe -sSfLO -A "Stinkshot/0.1.0" https://code.mccullough.dev/beard/stinkshot/releases/download/v0.1.0/stinkshot-release.json.minisig
C:\dev\stinkshot-bench\tools\minisign\minisign-win64\x86_64\minisign.exe -Vm stinkshot-release.json -p crates\app\assets\release-key.pub
Remove-Item stinkshot-release.json, stinkshot-release.json.minisig
```

Anubis: on Nov 8, requests with the `Stinkshot/0.1.0` and winget User-Agents reached Forgejo
(they got its own 404s, not a challenge page). If the first command above returns HTML,
Anubis is challenging it and the update check and winget will fail; allow `/api/` and
`/releases/download/` for non-browser clients in its policy.

0.1.0 itself never shows "update available" (nothing is newer); the first real test of the
update path is 0.1.1.

## 7. winget (after step 6, once the setup URL downloads)

Needs a GitHub account. The filled manifest is in
`C:\dev\stinkshot-bench\dist\0.1.0\winget\manifests\s\Stinkshot\Stinkshot\0.1.0`.

```
winget validate --manifest C:\dev\stinkshot-bench\dist\0.1.0\winget\manifests\s\Stinkshot\Stinkshot\0.1.0
winget install Microsoft.WingetCreate
wingetcreate submit --token <github token with public_repo> C:\dev\stinkshot-bench\dist\0.1.0\winget\manifests\s\Stinkshot\Stinkshot\0.1.0
```

`wingetcreate submit` forks microsoft/winget-pkgs, commits the folder and opens the pull
request. Without it: fork https://github.com/microsoft/winget-pkgs, copy the folder to
`manifests/s/Stinkshot/Stinkshot/0.1.0/`, commit `New package: Stinkshot.Stinkshot version
0.1.0`, open a pull request and answer the bot. Their pipeline downloads and scans the
installer; unsigned is allowed. `Stinkshot.Stinkshot` and the publisher name are set in
`packaging/winget`; change them before this first submission if you want another name.

Later versions: `wingetcreate update Stinkshot.Stinkshot --version X.Y.Z --urls <setup url> --submit --token <github token>`.

## 8. SmartScreen

0.1.0 is not Authenticode-signed. On first run of the installer (and of the portable exe)
Windows SmartScreen says "Windows protected your PC"; users click More info > Run anyway.
It fades as the file builds reputation, and resets with every new unsigned build. Say so in
the release notes or the README if you want fewer questions.

## 9. Code signing later (SignPath Foundation, free for open source)

1. Repository public with the GPL-3.0 license (done in step 4).
2. Apply at https://signpath.org (open source application form). They ask for the repository
   URL, the license and a code signing policy; `SECURITY.md` covers the basics.
3. Check their build requirement: Foundation certificates are normally only for artifacts
   built on a CI system they can verify (GitHub Actions, AppVeyor, Azure DevOps). Forgejo
   Actions may not qualify; then the route is a GitHub mirror with a workflow that runs
   `scripts/release.ps1`. Not verified.
4. Once approved, with the organization id, project slug, policy slug and CI token they give
   you:

```
Install-Module SignPath -Scope CurrentUser
$env:SIGNPATH_API_TOKEN = Read-Host "SignPath token"
powershell -ExecutionPolicy Bypass -File scripts\release.ps1 -Sign signpath -SignPathOrganizationId <org id> -SignPathProjectSlug stinkshot -SignPathPolicySlug release-signing -TargetDir C:\dev\stinkshot-bench\target-dist -OutDir C:\dev\stinkshot-bench\dist\X.Y.Z
```

With your own certificate instead: `-Sign signtool -CertThumbprint <sha1>`
(`docs/RELEASING.md`).
