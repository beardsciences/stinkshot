<#
.SYNOPSIS
Builds a Stinkshot release: tests, release exe, portable zip, per-user installer, SBOM,
SHA256SUMS and the signed update manifest. Publishes nothing.

.DESCRIPTION
Runs on Windows PowerShell 5.1 and PowerShell 7. Everything lands in -OutDir
(default dist\<version>); docs/RELEASING.md has the full procedure and the publish steps.

Release builds go to -TargetDir (default target-dist next to the repo's target folder),
never to target\ itself, so a running target\release\stinkshot.exe is not touched.

Tools are looked up in this order: the parameter, an environment variable, PATH, then
C:\dev\stinkshot-bench\tools (where the dev box keeps them):
  ISCC.exe          Inno Setup 7          -Iscc       STINKSHOT_ISCC
  minisign.exe      minisign 0.11+        -Minisign   STINKSHOT_MINISIGN
  cargo-cyclonedx   cargo-cyclonedx 0.5+  -Cyclonedx  STINKSHOT_CYCLONEDX

Code signing is optional (-Sign none is the default and produces unsigned artifacts):
  -Sign signtool  -CertThumbprint <sha1>   signs with a certificate in the user's store
  -Sign signpath  -SignPathOrganizationId ... -SignPathProjectSlug ... -SignPathPolicySlug ...
                  (API token in SIGNPATH_API_TOKEN; needs the SignPath PowerShell module)

.EXAMPLE
powershell -ExecutionPolicy Bypass -File scripts\release.ps1

.EXAMPLE
pwsh scripts\release.ps1 -TargetDir C:\dev\stinkshot-bench\target-release -OutDir C:\dev\stinkshot-bench\dist -AllowDirty
#>
[CmdletBinding()]
param(
    [string]$Version,
    [ValidateSet('x64', 'arm64')]
    [string[]]$Arch = @('x64'),
    [string]$TargetDir,
    [string]$OutDir,
    [switch]$SkipTests,
    [switch]$AllowDirty,
    [string]$Iscc,
    [string]$Minisign,
    [string]$Cyclonedx,
    [string]$SigningKey = (Join-Path $env:USERPROFILE '.stinkshot-release\stinkshot-release.key'),
    [ValidateSet('none', 'signtool', 'signpath')]
    [string]$Sign = 'none',
    [string]$CertThumbprint,
    [string]$TimestampUrl = 'http://timestamp.digicert.com',
    [string]$SignTool,
    [string]$SignPathOrganizationId,
    [string]$SignPathProjectSlug,
    [string]$SignPathPolicySlug,
    [string]$SignPathArtifactConfigurationSlug
)

Set-StrictMode -Version 3
$ErrorActionPreference = 'Stop'

$Repo = Split-Path -Parent $PSScriptRoot
$ProjectUrl = 'https://code.mccullough.dev/beard/stinkshot'
$PublicKeyFile = Join-Path $Repo 'crates\app\assets\release-key.pub'
$BenchTools = 'C:\dev\stinkshot-bench\tools'
$Utf8 = New-Object System.Text.UTF8Encoding($false)
$Triples = @{ x64 = 'x86_64-pc-windows-msvc'; arm64 = 'aarch64-pc-windows-msvc' }

function Step([string]$Text) { Write-Host "==> $Text" -ForegroundColor Cyan }
function Fail([string]$Text) { throw "release: $Text" }

function Invoke-Native {
    param([string]$Exe, [string[]]$Arguments)
    # Windows PowerShell 5.1 turns a native tool's stderr (cargo's progress lines) into error
    # records when output is redirected; with 'Stop' that would end the script. Judge by the
    # exit code only.
    $ErrorActionPreference = 'Continue'
    & $Exe @Arguments 2>&1 | ForEach-Object { if ($_ -is [System.Management.Automation.ErrorRecord]) { Write-Host $_.Exception.Message } else { $_ } }
    if ($LASTEXITCODE -ne 0) { Fail "$([IO.Path]::GetFileName($Exe)) $($Arguments -join ' ') exited with $LASTEXITCODE" }
}

function Find-Tool {
    param([string]$Given, [string]$EnvName, [string]$ExeName, [string[]]$Fallbacks, [switch]$Optional)
    $candidates = @()
    if ($Given) { $candidates += $Given }
    $fromEnv = [Environment]::GetEnvironmentVariable($EnvName)
    if ($fromEnv) { $candidates += $fromEnv }
    $onPath = Get-Command $ExeName -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($onPath) { $candidates += $onPath.Source }
    $candidates += $Fallbacks
    foreach ($c in $candidates) { if ($c -and (Test-Path -LiteralPath $c)) { return (Resolve-Path -LiteralPath $c).Path } }
    if ($Optional) { return $null }
    Fail "$ExeName not found. Pass -$($EnvName -replace '^STINKSHOT_','') or set $EnvName (docs/RELEASING.md lists where to get it)."
}

function Write-TextLf([string]$Path, [string]$Text) {
    [IO.File]::WriteAllText($Path, ($Text -replace "`r`n", "`n"), $Utf8)
}

function Get-Sha256([string]$Path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant() }

function Find-SignTool {
    if ($SignTool) { return $SignTool }
    $kits = 'C:\Program Files (x86)\Windows Kits\10\bin'
    $found = Get-ChildItem -Path $kits -Filter signtool.exe -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\' } | Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $found) { Fail 'signtool.exe not found (Windows SDK). Pass -SignTool.' }
    return $found.FullName
}

# Signs files in place. No-op for -Sign none.
function Invoke-CodeSign([string[]]$Files) {
    if ($Sign -eq 'none') { return }
    foreach ($f in $Files) {
        if ($Sign -eq 'signtool') {
            if (-not $CertThumbprint) { Fail '-Sign signtool needs -CertThumbprint.' }
            $st = Find-SignTool
            Invoke-Native $st @('sign', '/sha1', $CertThumbprint, '/fd', 'SHA256', '/tr', $TimestampUrl, '/td', 'SHA256', '/d', 'Stinkshot', '/du', $ProjectUrl, $f)
            Invoke-Native $st @('verify', '/pa', $f)
        } else {
            $token = $env:SIGNPATH_API_TOKEN
            if (-not $token) { Fail 'SIGNPATH_API_TOKEN is not set.' }
            foreach ($p in 'SignPathOrganizationId', 'SignPathProjectSlug', 'SignPathPolicySlug') {
                if (-not (Get-Variable $p -ValueOnly)) { Fail "-Sign signpath needs -$p." }
            }
            if (-not (Get-Command Submit-SigningRequest -ErrorAction SilentlyContinue)) {
                Fail 'SignPath module missing: Install-Module SignPath -Scope CurrentUser'
            }
            $signed = "$f.signed"
            $req = @{
                InputArtifactPath  = $f
                OutputArtifactPath = $signed
                ApiToken           = $token
                OrganizationId     = $SignPathOrganizationId
                ProjectSlug        = $SignPathProjectSlug
                SigningPolicySlug  = $SignPathPolicySlug
                WaitForCompletion  = $true
                Force              = $true
            }
            if ($SignPathArtifactConfigurationSlug) { $req.ArtifactConfigurationSlug = $SignPathArtifactConfigurationSlug }
            Submit-SigningRequest @req | Out-Null
            Move-Item -LiteralPath $signed -Destination $f -Force
        }
        $sig = Get-AuthenticodeSignature -LiteralPath $f
        if ($sig.Status -ne 'Valid') { Fail "$f is not validly signed after signing ($($sig.Status))." }
    }
}

# ------------------------------------------------------------------ inputs

$savedEnv = @{}
foreach ($n in 'CARGO_TARGET_DIR', 'STINKSHOT_PROFILE', 'SOURCE_DATE_EPOCH', 'PATH') { $savedEnv[$n] = [Environment]::GetEnvironmentVariable($n) }

Push-Location $Repo
try {
    $cargoToml = Get-Content -Raw (Join-Path $Repo 'Cargo.toml')
    $m = [regex]::Match($cargoToml, '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"')
    if (-not $m.Success) { Fail 'workspace.package version not found in Cargo.toml' }
    $cargoVersion = $m.Groups[1].Value
    if (-not $Version) { $Version = $cargoVersion }
    if ($Version -ne $cargoVersion) { Fail "-Version $Version does not match Cargo.toml ($cargoVersion). Bump Cargo.toml first." }
    $Tag = "v$Version"

    if (-not $TargetDir) { $TargetDir = Join-Path $Repo 'target-dist' }
    if (-not $OutDir) { $OutDir = Join-Path $Repo "dist\$Version" }
    $TargetDir = [IO.Path]::GetFullPath($TargetDir).TrimEnd('\')
    if ($TargetDir -ieq (Join-Path $Repo 'target')) {
        Fail "-TargetDir must not be the repo's target folder (a running stinkshot.exe may live there)."
    }

    $dirty = git status --porcelain
    if ($dirty -and -not $AllowDirty) { Fail 'working tree has changes. Commit them or pass -AllowDirty (for dry runs).' }
    $commit = (git rev-parse HEAD).Trim()

    $changelog = Get-Content -Raw (Join-Path $Repo 'CHANGELOG.md')
    $section = [regex]::Match($changelog, "(?ms)^## \[$([regex]::Escape($Version))\][^\n]*\n(.*?)(?=^## \[|\z)")
    if (-not $section.Success) {
        if ($AllowDirty) { Write-Warning "CHANGELOG.md has no [$Version] section; release notes will be empty." }
        else { Fail "CHANGELOG.md has no ## [$Version] section." }
    }
    $changes = if ($section.Success) { $section.Groups[1].Value.Trim() } else { '' }

    $isccExe = Find-Tool $Iscc 'STINKSHOT_ISCC' 'ISCC.exe' @("$BenchTools\innosetup\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 7\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 7\ISCC.exe", "$env:ProgramFiles\Inno Setup 7\ISCC.exe")
    $minisignExe = Find-Tool $Minisign 'STINKSHOT_MINISIGN' 'minisign.exe' @("$BenchTools\minisign\minisign-win64\x86_64\minisign.exe")
    $cyclonedxExe = Find-Tool $Cyclonedx 'STINKSHOT_CYCLONEDX' 'cargo-cyclonedx.exe' @("$BenchTools\cargo\bin\cargo-cyclonedx.exe", "$env:USERPROFILE\.cargo\bin\cargo-cyclonedx.exe")
    $haveKey = Test-Path -LiteralPath $SigningKey
    if (-not $haveKey) { Write-Warning "signing key $SigningKey not found: the update manifest and SHA256SUMS stay unsigned (the app will ignore this release)." }

    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    $env:CARGO_TARGET_DIR = $TargetDir
    # The tests check default paths, so no dev profile may leak in. The script never runs
    # the built exe (the version check reads its resource), so the real instance is safe.
    $env:STINKSHOT_PROFILE = $null

    Step "Stinkshot $Version ($commit) -> $OutDir"

    # -------------------------------------------------------------- checks
    if (-not $SkipTests) {
        Step 'cargo fmt --check'
        Invoke-Native cargo @('fmt', '--all', '--', '--check')
        Step 'cargo clippy'
        Invoke-Native cargo @('clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings')
        Step 'cargo test'
        Invoke-Native cargo @('test', '--workspace', '--locked')
    }

    $artifacts = @()
    foreach ($a in $Arch) {
        $triple = $Triples[$a]
        Step "cargo build --release --target $triple"
        $installed = rustup target list --installed
        if ($installed -notcontains $triple) { Fail "Rust target $triple missing: rustup target add $triple" }
        if ($a -eq 'arm64') {
            # ring's build script needs clang for aarch64 Windows; the MSVC ARM64 build tools
            # (VS component Microsoft.VisualStudio.Component.VC.Tools.ARM64) provide the CRT.
            $llvm = Join-Path $env:ProgramFiles 'LLVM\bin'
            if (-not (Get-Command clang -ErrorAction SilentlyContinue) -and (Test-Path $llvm)) { $env:PATH = "$llvm;$env:PATH" }
        }
        Invoke-Native cargo @('build', '--release', '--locked', '-p', 'stinkshot', '--target', $triple)
        $exe = Join-Path $TargetDir "$triple\release\stinkshot.exe"
        $info = (Get-Item -LiteralPath $exe).VersionInfo
        if ($info.ProductVersion -ne $Version) { Fail "exe ProductVersion is '$($info.ProductVersion)', expected $Version" }

        $stage = Join-Path $OutDir "stage-$a"
        if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
        New-Item -ItemType Directory -Force -Path $stage | Out-Null
        Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'stinkshot.exe')
        Copy-Item -LiteralPath (Join-Path $Repo 'LICENSE') -Destination (Join-Path $stage 'LICENSE.txt')
        Copy-Item -LiteralPath (Join-Path $Repo 'README.md') -Destination (Join-Path $stage 'README.md')
        Invoke-CodeSign @((Join-Path $stage 'stinkshot.exe'))

        Step "portable zip ($a)"
        $zip = Join-Path $OutDir "Stinkshot-$Version-$a-portable.zip"
        if (Test-Path $zip) { Remove-Item -Force $zip }
        Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal
        $artifacts += $zip

        Step "installer ($a)"
        $ico = Get-ChildItem -Path (Join-Path $TargetDir "$triple\release\build") -Filter stinkshot.ico -Recurse -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
        $isccArgs = @('/Q', "/DAppVersion=$Version", "/DArch=$a", "/DStageDir=$stage", "/DOutputDir=$OutDir")
        if ($ico) { $isccArgs += "/DIconFile=$($ico.FullName)" }
        if ($Sign -eq 'signtool') {
            $st = Find-SignTool
            $isccArgs += "/Ssigntool=`$q$st`$q sign /sha1 $CertThumbprint /fd SHA256 /tr $TimestampUrl /td SHA256 /d `$qStinkshot Setup`$q `$f"
            $isccArgs += '/DSignTool=1'
        }
        $isccArgs += (Join-Path $Repo 'packaging\inno\stinkshot.iss')
        Invoke-Native $isccExe $isccArgs
        $setup = Join-Path $OutDir "Stinkshot-$Version-$a-setup.exe"
        if ($Sign -eq 'signpath') { Invoke-CodeSign @($setup) }
        $artifacts += $setup

        # Smoke-test installer (own AppId, name and shortcut; docs/RELEASING.md step 5). Not a
        # release artifact: never hashed, signed or uploaded.
        Step "smoke-test installer ($a)"
        $testArgs = @('/Q', '/DTestBuild=1', "/DAppVersion=$Version", "/DArch=$a", "/DStageDir=$stage", "/DOutputDir=$(Join-Path $OutDir 'test')")
        if ($ico) { $testArgs += "/DIconFile=$($ico.FullName)" }
        $testArgs += (Join-Path $Repo 'packaging\inno\stinkshot.iss')
        Invoke-Native $isccExe $testArgs
    }

    # ----------------------------------------------------------------- SBOM
    Step 'SBOM (CycloneDX)'
    $sbomName = "Stinkshot-$Version.cdx"
    $env:SOURCE_DATE_EPOCH = (git log -1 --format=%ct).Trim()
    Invoke-Native $cyclonedxExe @('cyclonedx', '--manifest-path', (Join-Path $Repo 'crates\app\Cargo.toml'), '--format', 'json', '--spec-version', '1.5', '--target', $Triples[$Arch[0]], '--override-filename', $sbomName, '-q')
    $sbomSrc = Join-Path $Repo "crates\app\$sbomName.json"
    $sbom = Join-Path $OutDir "$sbomName.json"
    Move-Item -LiteralPath $sbomSrc -Destination $sbom -Force
    # cargo-cyclonedx writes one file per workspace member; the app's covers the whole tree.
    Get-ChildItem -Path (Join-Path $Repo 'crates') -Filter "$sbomName.json" -Recurse | Remove-Item -Force
    $artifacts += $sbom

    # ------------------------------------------------- checksums + manifest
    Step 'SHA256SUMS'
    $sums = Join-Path $OutDir 'SHA256SUMS'
    $lines = foreach ($f in $artifacts) { "$(Get-Sha256 $f)  $([IO.Path]::GetFileName($f))" }
    Write-TextLf $sums (($lines -join "`n") + "`n")

    Step 'update manifest'
    $files = foreach ($f in $artifacts) {
        [ordered]@{ name = [IO.Path]::GetFileName($f); size = (Get-Item -LiteralPath $f).Length; sha256 = (Get-Sha256 $f) }
    }
    $manifest = [ordered]@{
        schema   = 1
        product  = 'stinkshot'
        version  = $Version
        tag      = $Tag
        released = (Get-Date -Format 'yyyy-MM-dd')
        page     = "https://stinkshot.com/download"
        commit   = $commit
        files    = @($files)
    }
    $manifestPath = Join-Path $OutDir 'stinkshot-release.json'
    Write-TextLf $manifestPath (($manifest | ConvertTo-Json -Depth 5) + "`n")

    if ($haveKey) {
        Step 'minisign: manifest and SHA256SUMS'
        foreach ($f in @($manifestPath, $sums)) {
            $name = [IO.Path]::GetFileName($f)
            # minisign asks for the key password here if the key has one.
            Invoke-Native $minisignExe @('-S', '-s', $SigningKey, '-m', $f, '-t', "stinkshot $Tag $name")
            # Must verify with the key the app embeds, or every installed copy rejects it.
            Invoke-Native $minisignExe @('-V', '-q', '-p', $PublicKeyFile, '-m', $f)
        }
    }

    # ------------------------------------------------- winget (filled draft)
    $x64Setup = Join-Path $OutDir "Stinkshot-$Version-x64-setup.exe"
    if (Test-Path -LiteralPath $x64Setup) {
        Step 'winget manifest (not submitted)'
        $src = Join-Path $Repo 'packaging\winget\manifests\s\Stinkshot\Stinkshot\0.1.0'
        $dst = Join-Path $OutDir "winget\manifests\s\Stinkshot\Stinkshot\$Version"
        New-Item -ItemType Directory -Force -Path $dst | Out-Null
        $setupHash = (Get-Sha256 $x64Setup).ToUpperInvariant()
        foreach ($y in Get-ChildItem -LiteralPath $src -Filter *.yaml) {
            $text = (Get-Content -Raw -LiteralPath $y.FullName).Replace('0.1.0', $Version).
                Replace(('0' * 64), $setupHash) -replace 'ReleaseDate: \d{4}-\d{2}-\d{2}', "ReleaseDate: $(Get-Date -Format 'yyyy-MM-dd')"
            $text = $text -replace '(?m)^# Draft.*\r?\n', ''
            Write-TextLf (Join-Path $dst $y.Name) $text
        }
    }

    # ------------------------------------------------------- release notes
    $template = Get-Content -Raw (Join-Path $Repo 'packaging\release-notes-template.md')
    $pubKey = (Get-Content -LiteralPath $PublicKeyFile)[1].Trim()
    $notes = $template.Replace('{{VERSION}}', $Version).Replace('{{TAG}}', $Tag).Replace('{{CHANGES}}', $changes).
        Replace('{{SHA256SUMS}}', ((Get-Content -Raw $sums).Trim())).Replace('{{PUBLIC_KEY}}', $pubKey).Replace('{{COMMIT}}', $commit)
    Write-TextLf (Join-Path $OutDir 'RELEASE_NOTES.md') $notes

    foreach ($a in $Arch) { Remove-Item -Recurse -Force (Join-Path $OutDir "stage-$a") }

    Step 'done'
    Get-ChildItem -LiteralPath $OutDir | Select-Object Name, Length | Format-Table -AutoSize | Out-String | Write-Host
    if ($Sign -eq 'none') { Write-Host 'Artifacts are not Authenticode-signed (-Sign none).' }
    Write-Host "Upload everything above except RELEASE_NOTES.md to the $Tag release (docs/RELEASING.md)."
}
finally {
    foreach ($n in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($n, $savedEnv[$n]) }
    Pop-Location
}
