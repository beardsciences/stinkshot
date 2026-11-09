# Security policy

## What Stinkshot promises

- Captures never leave your PC. There are no upload targets, share targets, accounts or
  telemetry, and there never will be.
- The only network request is the update check. It is off by default. When on, it makes plain
  HTTPS GETs to stinkshot.com (`/release/stinkshot-release.json` and its signature) at most
  once a day, and sends nothing beyond what
  any GET carries (a `Stinkshot/<version>` User-Agent).
- The update check trusts version info only after verifying its minisign (Ed25519) signature
  against the public key built into the app. It never downloads or runs installers.

A finding that breaks any of these is a security bug, as is anything that lets another
program or web page read captures through Stinkshot, or a redaction that leaves the original
pixels in the exported image.

Known and documented: with Keep Editable Originals on, the unredacted original of an annotated
capture is kept in `<history folder>\.stinkshot\edit` (see README, Privacy).

## Supported versions

Only the latest release gets fixes.

## Reporting a vulnerability

Email the maintainer at christian@mccullough.dev. Do not open an issue: issues on the
repository are public.

Include the Stinkshot version (`stinkshot --version`), Windows version, and steps to reproduce.
You should get an answer within a week. Fixed issues are credited in CHANGELOG.md unless you
ask otherwise.

## Verifying a download

Every release ships `SHA256SUMS` and `SHA256SUMS.minisig`. With
[minisign](https://jedisct1.github.io/minisign/):

```
minisign -Vm SHA256SUMS -P RWSgFp6pSQuSoo99V89nCiRKQ+SToPCxbnLeyViYxFzzfnbNKV77DFH3
```

then compare the file's hash (`Get-FileHash <file>` in PowerShell) with its line in
`SHA256SUMS`. The same key is in `crates/app/assets/release-key.pub`.

Releases may also be Authenticode-signed; unsigned builds are not tampered builds, but check the
minisign signature either way.
