Stinkshot {{VERSION}}: local-first screenshots and screen recording for Windows. Captures never leave your PC.

{{CHANGES}}

## Install

- **Installer** (per user, no admin): `Stinkshot-{{VERSION}}-x64-setup.exe`. Silent: `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`.
- **winget**: `winget install Stinkshot.Stinkshot` (once the manifest is merged).
- **Portable**: unzip `Stinkshot-{{VERSION}}-x64-portable.zip` anywhere and run `stinkshot.exe`.

Uninstalling keeps your captures, history and settings.

## Verify

```
minisign -Vm SHA256SUMS -P {{PUBLIC_KEY}}
```

Then compare each file's SHA-256 with `SHA256SUMS`:

```
{{SHA256SUMS}}
```

SBOM: `Stinkshot-{{VERSION}}.cdx.json` (CycloneDX). Built from commit `{{COMMIT}}`.
