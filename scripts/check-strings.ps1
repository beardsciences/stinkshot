# Reports hard-coded UI strings per group and file (docs/notes/l10n.md).
# Usage: scripts\check-strings.ps1 [-Verbose]   (-Verbose lists each literal with its line)
# Exit code is nonzero while any are left. Set CARGO_TARGET_DIR to build outside target\.
[CmdletBinding()]
param()
$root = Split-Path -Parent $PSScriptRoot
if ($VerbosePreference -eq 'Continue') { $env:STRINGS_VERBOSE = '1' } else { $env:STRINGS_VERBOSE = '0' }
Push-Location $root
try {
    cargo test -q -p stinkshot --test check_strings -- --ignored --nocapture user_strings_use_lookup
    exit $LASTEXITCODE
} finally {
    Pop-Location
}
