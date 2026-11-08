# release notes (area)

Release engineering, Nov 8 2026. Procedure for the maintainer: `docs/RELEASING.md`.

Files: `scripts/release.ps1`, `packaging/inno/stinkshot.iss`, `packaging/winget/...`,
`packaging/release-notes-template.md`, `crates/app/src/update.rs`,
`crates/app/assets/release-key.pub`, `crates/app/testdata/update/*`, `crates/app/build.rs`
(VERSIONINFO), `CHANGELOG.md`, `SECURITY.md`, `CONTRIBUTING.md`, `.gitattributes`.
App changes outside update.rs: `stinkshot quit` (cli.rs, `shell::quit_running`,
`sysutil::running_instance`), one Settings description, the signature-failure toast body.

## Installer: Inno Setup 7, not WiX

Both meet the bar (per-user, silent, winget types `inno` and `msi`, Start menu shortcut,
uninstall entry). Inno won on the specifics of a tray app installed per user:

- Per-user is first class (`PrivilegesRequired=lowest`, `{autopf}` = `%LOCALAPPDATA%\Programs`).
  Per-user MSI needs ICE38/ICE64/ICE91 workarounds (HKCU key paths for every component,
  RemoveFolder rows) and `MSIINSTALLPERUSER` quirks.
- Optional autostart is a one-line unchecked `[Tasks]` checkbox; in WiX it is a custom dialog or
  a property only scripts can set.
- Closing the running tray app before an upgrade or uninstall: `PrepareToInstall` and
  `[UninstallRun]` call `stinkshot quit` (graceful, restores PrintScreen); MSI would need a
  custom action DLL or `util:CloseApplication`, which only sends WM_CLOSE to windows the tray
  app does not have.
- Uninstall cleanup that must be conditional (remove the Run value only if it points at this
  copy, restore the Snipping Tool value only if Stinkshot saved one) is a few lines of Pascal.
- winget knows Inno's switches (`/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`, `/SILENT`), so the
  manifest needs no `InstallerSwitches`.

Cost: no MSI for GPO/Intune admins (Intune can still deploy the exe as a Win32 app). If that
is ever wanted, WiX v5 is a `dotnet tool` and needs no system install either.

Inno Setup 7.1.1 (Authenticode: Pyrsys B.V., valid) was installed with
`/CURRENTUSER /PORTABLE=1 /DIR=C:\dev\stinkshot-bench\tools\innosetup`: no uninstaller, no
registry entries, nothing outside that folder. License: permissive, commercial use allowed.

### What uninstall removes and keeps

Removes: `stinkshot.exe`, `LICENSE.txt`, the uninstaller, the Start menu shortcut, the
uninstall entry, `HKCU\...\Run\StinkShot` if it points into the install folder, the saved
`HKCU\Software\StinkShot\PrevPrintScreenSnipping` (restoring the Snipping Tool value it held;
normally already done by the graceful quit), the key itself if empty, and
`%LOCALAPPDATA%\Stinkshot\cache` (the logo animation cache).

Keeps: `%APPDATA%\Stinkshot` (config, log), the history folder and every capture, even when the
history folder was put inside the install folder (Inno deletes only files it installed; there
is deliberately no `[UninstallDelete]` wildcard).

`stinkshot quit` and the uninstaller act on the instance of the current profile in the current
session, whichever exe it runs from: a portable copy running under the default profile is asked
to quit too. Accepted: there is one instance per profile by design.

`STINKSHOT_PROFILE` is honored by the installer's Pascal code (same sanitizing as
`profile_suffix`), which is what made the install tests below safe beside the real instance.

## `stinkshot quit`

New subcommand (`CliAction::QuitRunning`). Never starts an instance: if the profile's mutex
is free it exits 0 at once. Otherwise it finds the IPC message-only window's process
(`sysutil::running_instance`), forwards `quit` (the running instance maps it to
`Command::Quit`: finishes recordings and saves, up to `QUIT_GRACE`), waits for the mutex, then
for the process itself (`WaitForSingleObject`): the mutex is released before GPU/COM teardown
finishes, and the first test showed the uninstaller then failing to delete the still-mapped
exe. Exit 1 if it does not answer or does not exit within `QUIT_GRACE` + 5 s.

## Verified update check

Before: one GET of the Forgejo latest-release API, tag compared, trusted as is.

Now (`update.rs`):

1. GET `.../api/v1/repos/beard/stinkshot/releases/latest` (unchanged; 404 = up to date). Its
   `tag_name` is untrusted and only decides whether to continue; it must match
   `[A-Za-z0-9._+-]{1,64}` (no leading dot) because it is put into a URL path.
2. If that tag is newer: GET `.../releases/download/<tag>/stinkshot-release.json` and
   `.../stinkshot-release.json.minisig`. URLs are built from a fixed base, never taken from the
   API answer. Size caps 1 MB / 64 KB / 4 KB.
3. `minisign_verify` (0.3.0, pure Rust, no dependencies) checks the signature against
   `assets/release-key.pub` (compiled in with `include_str!`). Only prehashed Ed25519 (`ED`,
   minisign 0.10+ default) is accepted; legacy `Ed` is rejected. The trusted comment is covered
   by the global signature as minisign defines it.
4. The verified manifest must have `schema` 1, `product` "stinkshot", a version equal to its tag,
   and a tag equal to the API's tag. Its `page` is used for the tray link only if it starts with
   `https://code.mccullough.dev/beard/stinkshot/`.

Nothing else is sent: three plain GETs at most, same User-Agent as before, no cookies, no query
strings. No binary is downloaded. A signature failure is an error that starts with
`update::UNTRUSTED`; a forced check shows "The release info failed its signature check, so it
was ignored." instead of the offline message.

Manifest (`stinkshot-release.json`, written by the release script, LF, UTF-8 without BOM):

```json
{ "schema": 1, "product": "stinkshot", "version": "0.1.0", "tag": "v0.1.0",
  "released": "2026-11-08", "page": "https://code.mccullough.dev/beard/stinkshot/releases/tag/v0.1.0",
  "commit": "<sha>", "files": [ { "name": "...", "size": 0, "sha256": "..." } ] }
```

Unknown fields are ignored, so later versions can add some without breaking old clients.

Keys: release keypair in `C:\Users\beard\.stinkshot-release\` (generated with minisign 0.12
`-G -W`, no password; key id A2920B49A99E16A0). Backup and password steps are in
`docs/RELEASING.md`. The test fixture under `crates/app/testdata/update` is signed with a
separate throwaway key whose secret was deleted right after signing; a test asserts it differs
from the release key. `.gitattributes` marks the fixtures and the public key `-text`
(`core.autocrlf` is on here, and a CRLF conversion would break the signature; a test checks that
it does).

## Release script

`scripts/release.ps1` (Windows PowerShell 5.1 and 7): version from `Cargo.toml`, refuses a
dirty tree (`-AllowDirty` for dry runs), requires a `CHANGELOG.md` section, fmt/clippy/test
(`-SkipTests`), `cargo build --release --locked` per arch into `-TargetDir` (default
`target-dist`, refuses the repo's `target`), checks the exe's ProductVersion resource instead of
running it, optional Authenticode signing, zip, Inno installer, CycloneDX 1.5 SBOM of the app
crate (279 components; `SOURCE_DATE_EPOCH` from the commit), `SHA256SUMS`, manifest, minisign
signatures of both, verification of both against the committed public key (catches a signing
key that does not match what the app embeds), filled winget manifest, release notes. Env vars
it sets are restored on exit. It never publishes.

`STINKSHOT_PROFILE` is cleared during the run: the core tests assert default paths and failed
with it set (first dry run).

## Version metadata

VERSIONINFO now also has CompanyName "Stinkshot contributors", InternalName, Comments (repo
URL), a full LegalCopyright, and VS_FF_PRERELEASE for versions with a `-` suffix. Checked on the
built exe: ProductName Stinkshot, ProductVersion/FileVersion 0.1.0, IsPreRelease False.

## ARM64

Rust target `aarch64-pc-windows-msvc` installed (rustup, per user). The Build Tools here have
MSVC 14.44 for x64/x86 only: no `bin\Hostx64\arm64` compiler and no `lib\arm64` CRT
(`msvcrt.lib`, `vcruntime.lib`). The Windows SDK 10.0.26100 does have arm64 `um` and `ucrt`
libs. Plain `cargo build --target aarch64-pc-windows-msvc` fails in `ring`'s build script
(no cl.exe or clang for arm64). With the installed LLVM 23 on PATH
(`CC_aarch64_pc_windows_msvc=clang-cl`, `AR_...=llvm-lib`,
`CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER=lld-link`, target dir under stinkshot-bench) every
crate compiles, including ring, rusqlite's bundled SQLite and the resource script, and only the
final link fails (318 s): `lld-link: could not open 'msvcrt.lib'` / `'oldnames.lib'`. So no ARM64
artifact can be produced on this box as it is; the only missing piece is the MSVC ARM64 CRT.

To produce ARM64 artifacts, install the VS component "MSVC v143 - VS 2022 C++ ARM64/ARM64EC
build tools (Latest)" (`Microsoft.VisualStudio.Component.VC.Tools.ARM64`) in the Visual Studio
Installer for Build Tools 2022. That is a system-wide change, so it was not done. Afterwards
`scripts\release.ps1 -Arch x64,arm64` builds both; the installer script already has an arm64
branch (`ArchitecturesAllowed=arm64`) and the winget manifest would need a second `Installers`
entry. The x64 build already runs on Windows 11 on Arm under emulation
(`ArchitecturesAllowed=x64compatible`), untested.

## Verified (dev box, Nov 8 2026)

- `cargo test -p stinkshot`: 11 update tests (signed fixture verifies; tampered manifest, CRLF
  conversion, tampered trusted comment, wrong key, garbage signature rejected; unsafe tags
  rejected; off-project page replaced; embedded key parses) and the `quit` CLI test pass.
- Ignored tests: `release_dir_verifies_with_embedded_key` passes on the dry-run output (the app's
  own code accepts what the script signed); `live_check_answers` passes against the real server
  (404: no repository yet, so "up to date").
- Release dry run (`-AllowDirty -SkipTests`, warm cache): 141 s; cold release build 7 min
  (480 s total). Output: setup 9.56 MB, portable zip 10.04 MB, exe 24.3 MB, SBOM 328 KB.
- `winget validate` passes on the draft and on the filled manifest.
- Installer, run under `STINKSHOT_PROFILE=reltest` (scratch script, real instance untouched,
  PID checked before and after):
  fresh silent install with `/TASKS=autostart` 2.1 s: files, `StinkShot-reltest` Run value,
  shortcut, uninstall entry (DisplayVersion 0.1.0) present, no plain `StinkShot` Run value;
  `stinkshot quit` on a running instance 1.0 s, exit 0, process gone; `quit` with nothing
  running exit 0, starts nothing; silent upgrade over a running instance 3.2 s, exit 0
  (log: "stinkshot quit exited with 0", "RestartManager found no applications");
  silent uninstall over a running instance 4.2 s: process gone, install folder holds only the
  planted `captures\keep.png`, Run value, shortcut, uninstall entry, planted
  `PrevPrintScreenSnipping-reltest` and the cache folder removed, config kept.
- First run of that test found two bugs, both fixed: the upgrade aborted (exit 5) because
  Restart Manager cannot close a tray app and `quit` returned before the process ended; the
  uninstall left `stinkshot.exe` behind for the same reason.

Test hygiene incident: the first test run used `New-Item -Force` on `HKCU\Software\StinkShot`,
which recreates the key and wiped the real instance's saved `PrevPrintScreenSnipping` (0). It was
restored to DWORD 0 right away and the test script fixed; the real instance kept running.

## Not verified

- Authenticode signing (no certificate here; SignPath needs the user's application).
- Downloads through Anubis on code.mccullough.dev: the API and a nonexistent download path
  answered 404 (not a challenge page) for the app's, winget's and a browser User-Agent, but no
  release exists yet to fetch a real asset.
- The full update flow against a real published release (only the pieces: live API GET, and the
  app's verifier on the script's signed output).
- Interactive installer pages (only silent runs were exercised), Windows 10, Windows on Arm.
- `-Sign signpath` and `-Sign signtool` code paths in the script never ran.
