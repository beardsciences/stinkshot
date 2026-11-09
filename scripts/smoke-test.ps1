<#
.SYNOPSIS
Smoke-tests built release artifacts: install (or unzip), start, IPC, capture, OCR, recording,
quit, uninstall. Sends no input to any window and touches no other Stinkshot instance.

.DESCRIPTION
Runs on Windows PowerShell 5.1 and PowerShell 7. Everything runs under
STINKSHOT_PROFILE=smoke (its own config, single-instance mutex, IPC window, Run value and
saved PrintScreen value) with PrintScreen takeover, hotkeys, clipboard and toasts off, and
installs into a temp folder with the smoke-test installer release.ps1 builds
(dist\<version>\test\Stinkshot-Test-<version>-<arch>-setup.exe: own AppId, Apps entry and
shortcut, so a real install is never upgraded or removed).

Checks, in order:
  install      silent per-user install into the temp folder (or -Mode portable: unzip)
  version      the exe's ProductVersion matches the artifact version
  start        `stinkshot --tray` comes up and its IPC window answers to this process
  ipc capture  a second `stinkshot full` is forwarded over IPC and the running instance saves
               a full-screen capture to the history folder
  headless     `stinkshot full --out` writes a PNG and exits
  ocr          `stinkshot ocr --file` reads known text from an image the script draws
  record       3 s MP4 of the test's own swatch window (record crate example, release build,
               same target dir; the shipped exe has no headless recording), duration, size and
               colors checked
  quit         `stinkshot quit` stops the instance over IPC
  uninstall    with the app running again: the uninstaller quits it, removes the exe and the
               Apps entry, and leaves the history folder and config alone

Screen captures land in a temp folder that is deleted at the end unless -Keep.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File scripts\smoke-test.ps1 -DistDir C:\dev\stinkshot-bench\dist-dryrun -TargetDir C:\dev\stinkshot-bench\target-dist-dryrun
#>
[CmdletBinding()]
param(
    [string]$DistDir,
    [ValidateSet('installer', 'portable')]
    [string]$Mode = 'installer',
    [ValidateSet('x64', 'arm64')]
    [string]$Arch = 'x64',
    # Cargo target dir of the release build; smoke_record.exe is built there if missing.
    [string]$TargetDir,
    [string]$WorkDir,
    [switch]$SkipRecord,
    [switch]$Keep
)

Set-StrictMode -Version 3
$ErrorActionPreference = 'Stop'

$Repo = Split-Path -Parent $PSScriptRoot
$Profile_ = 'smoke'
$Triples = @{ x64 = 'x86_64-pc-windows-msvc'; arm64 = 'aarch64-pc-windows-msvc' }
$TestAppId = '{8638BB59-F403-4458-8CD8-7B748885E811}_is1'
$UninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$TestAppId"

$cargoToml = Get-Content -Raw (Join-Path $Repo 'Cargo.toml')
$Version = [regex]::Match($cargoToml, '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $DistDir) { $DistDir = Join-Path $Repo "dist\$Version" }
if (-not $TargetDir) { $TargetDir = Join-Path $Repo 'target-dist' }
if (-not $WorkDir) { $WorkDir = Join-Path $env:TEMP "stinkshot-smoke-$PID" }
$AppDir = Join-Path $WorkDir 'app'
$Captures = Join-Path $WorkDir 'captures'
$Exe = Join-Path $AppDir 'stinkshot.exe'
$ConfigDir = Join-Path $env:APPDATA "Stinkshot-$Profile_"

Add-Type -Namespace SmokeTest -Name Native -MemberDefinition @'
[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern IntPtr FindWindowExW(IntPtr parent, IntPtr after, string cls, string title);
[DllImport("user32.dll")]
public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
'@

$results = New-Object System.Collections.ArrayList
$clock = [Diagnostics.Stopwatch]::new()

function Check([string]$Name, [scriptblock]$Body) {
    $clock.Restart()
    $detail = ''
    $ok = $false
    try {
        $detail = & $Body
        $ok = $true
    } catch {
        $detail = $_.Exception.Message
    }
    $secs = [math]::Round($clock.Elapsed.TotalSeconds, 1)
    [void]$results.Add([pscustomobject]@{ Check = $Name; Result = $(if ($ok) { 'PASS' } else { 'FAIL' }); Seconds = $secs; Detail = "$detail" })
    $color = if ($ok) { 'Green' } else { 'Red' }
    Write-Host ("{0,-12} {1}  {2}s  {3}" -f $Name, $(if ($ok) { 'PASS' } else { 'FAIL' }), $secs, $detail) -ForegroundColor $color
    return $ok
}

# The smoke profile's IPC window and the pid that owns it (0 if none).
function Get-IpcPid {
    $hwnd = [SmokeTest.Native]::FindWindowExW([IntPtr](-3), [IntPtr]::Zero, [NullString]::Value, "StinkShot.IPC-$Profile_")
    if ($hwnd -eq [IntPtr]::Zero) { return 0 }
    $p = [uint32]0
    [void][SmokeTest.Native]::GetWindowThreadProcessId($hwnd, [ref]$p)
    return [int]$p
}

function Wait-Until([int]$Seconds, [scriptblock]$Cond) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $Seconds) {
        if (& $Cond) { return $true }
        Start-Sleep -Milliseconds 200
    }
    return [bool](& $Cond)
}

# Runs the exe to completion with stdout and stderr captured; returns @{ Code; Out; Err }.
function Invoke-Exe([string]$Path, [string[]]$Arguments, [int]$TimeoutSec = 60) {
    $o = Join-Path $WorkDir "out-$([guid]::NewGuid().ToString('N')).txt"
    $e = "$o.err"
    $p = Start-Process -FilePath $Path -ArgumentList $Arguments -NoNewWindow -PassThru -RedirectStandardOutput $o -RedirectStandardError $e
    # Windows PowerShell 5.1 reports no ExitCode unless the handle was opened while it ran.
    $null = $p.Handle
    if (-not $p.WaitForExit($TimeoutSec * 1000)) {
        Stop-Process -Id $p.Id -Force
        throw "$([IO.Path]::GetFileName($Path)) $($Arguments -join ' ') did not exit within $TimeoutSec s"
    }
    $r = @{ Code = $p.ExitCode; Out = (Get-Content -Raw -LiteralPath $o -ErrorAction SilentlyContinue); Err = (Get-Content -Raw -LiteralPath $e -ErrorAction SilentlyContinue) }
    if ($null -eq $r.Out) { $r.Out = '' }
    if ($null -eq $r.Err) { $r.Err = '' }
    Remove-Item -LiteralPath $o, $e -ErrorAction SilentlyContinue
    return $r
}

# Pictures and videos in the history folder, not its index (`.stinkshot`). -Include is not
# used: Windows PowerShell 5.1 ignores it with -LiteralPath.
function Get-Captures {
    Get-ChildItem -LiteralPath $Captures -Recurse -File | Where-Object {
        ('.png', '.jpg', '.webp', '.mp4', '.gif') -contains $_.Extension.ToLowerInvariant() -and $_.FullName -notlike '*\.stinkshot\*'
    }
}

function Start-Tray {
    $p = Start-Process -FilePath $Exe -ArgumentList '--tray' -PassThru
    $null = $p.Handle
    $script:TrayProc = $p
    if (-not (Wait-Until 30 { (Get-IpcPid) -eq $p.Id })) {
        if ($p.HasExited) { throw "exited with $($p.ExitCode) before its IPC window appeared" }
        throw 'no IPC window after 30 s'
    }
    return $p
}

$savedProfile = $env:STINKSHOT_PROFILE
$script:TrayProc = $null
$installed = $false
$total = [Diagnostics.Stopwatch]::StartNew()

try {
    $env:STINKSHOT_PROFILE = $Profile_
    if ((Get-IpcPid) -ne 0) { throw "a Stinkshot instance with profile '$Profile_' is already running. Quit it first." }
    if (Test-Path -LiteralPath $WorkDir) { throw "$WorkDir already exists. Pass another -WorkDir." }
    New-Item -ItemType Directory -Force -Path $WorkDir, $Captures | Out-Null
    $installer = Join-Path $DistDir "test\Stinkshot-Test-$Version-$Arch-setup.exe"
    $zip = Join-Path $DistDir "Stinkshot-$Version-$Arch-portable.zip"

    Write-Host "Stinkshot $Version smoke test ($Mode, profile $Profile_) in $WorkDir" -ForegroundColor Cyan

    # Config: nothing global (PrintScreen, hotkeys, clipboard, Run value), no popups, history
    # in the work folder. serde fills every other field with its default.
    New-Item -ItemType Directory -Force -Path $ConfigDir | Out-Null
    $toml = @"
[general]
start_with_windows = false
copy_to_clipboard = false
show_toast = false
play_sound = false

[hotkeys]
take_over_printscreen = false
replace_snipping_tool = false
region = ""
window = ""
fullscreen = ""
scrolling = ""
ocr = ""
repeat_last_region = ""
record_mp4 = ""
record_gif = ""

[capture]
open_editor_after_capture = false

[history]
folder = '$Captures'

[privacy]
check_for_updates = false
"@
    [IO.File]::WriteAllText((Join-Path $ConfigDir 'config.toml'), $toml, (New-Object System.Text.UTF8Encoding($false)))

    $ok = Check 'install' {
        if ($Mode -eq 'installer') {
            if (-not (Test-Path -LiteralPath $installer)) { throw "$installer not found. Run scripts\release.ps1 first." }
            if (Test-Path -LiteralPath $UninstallKey) { throw 'a Stinkshot Test install already exists (Apps). Uninstall it first.' }
            $log = Join-Path $WorkDir 'install.log'
            $p = Start-Process -FilePath $installer -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER', "/DIR=`"$AppDir`"", '/TASKS=""', "/LOG=`"$log`"" -PassThru -Wait
            if ($p.ExitCode -ne 0) { throw "installer exited with $($p.ExitCode) (log: $log)" }
            if (-not (Test-Path -LiteralPath $Exe)) { throw "no $Exe after install" }
            if (-not (Test-Path -LiteralPath $UninstallKey)) { throw 'no Apps entry after install' }
            $script:installed = $true
            "$([IO.Path]::GetFileName($installer)) -> $AppDir"
        } else {
            if (-not (Test-Path -LiteralPath $zip)) { throw "$zip not found. Run scripts\release.ps1 first." }
            Expand-Archive -LiteralPath $zip -DestinationPath $AppDir
            if (-not (Test-Path -LiteralPath $Exe)) { throw "no stinkshot.exe in $zip" }
            "$([IO.Path]::GetFileName($zip)) -> $AppDir"
        }
    }
    if (-not $ok) { throw 'install failed; nothing else to test' }

    [void](Check 'version' {
        $v = (Get-Item -LiteralPath $Exe).VersionInfo.ProductVersion
        if ($v -ne $Version) { throw "ProductVersion '$v', expected $Version" }
        $r = Invoke-Exe $Exe @('--version') 20
        if ($r.Code -ne 0) { throw "--version exited with $($r.Code)" }
        "$v; --version: $($r.Out.Trim())"
    })

    $ok = Check 'start' {
        $p = Start-Tray
        "pid $($p.Id), IPC window StinkShot.IPC-$Profile_"
    }

    if ($ok) {
        [void](Check 'ipc capture' {
            $before = @(Get-Captures).Count
            $r = Invoke-Exe $Exe @('full') 20
            if ($r.Code -ne 0) { throw "forwarding 'full' exited with $($r.Code) $($r.Err.Trim())" }
            if ($script:TrayProc.HasExited) { throw 'the running instance exited' }
            $saved = Wait-Until 20 { @(Get-Captures).Count -gt $before }
            if (-not $saved) { throw "no capture appeared in $Captures within 20 s" }
            $f = Get-Captures | Sort-Object LastWriteTime | Select-Object -Last 1
            "$($f.Name), $([math]::Round($f.Length / 1KB)) KB"
        })
    }

    $headless = Join-Path $Captures 'headless.png'
    [void](Check 'headless' {
        $r = Invoke-Exe $Exe @('full', '--out', $headless, '--monitor', '1') 30
        if ($r.Code -ne 0) { throw "exited with $($r.Code) $($r.Err.Trim())" }
        if (-not (Test-Path -LiteralPath $headless)) { throw 'no file written' }
        Add-Type -AssemblyName System.Drawing
        $img = [Drawing.Image]::FromFile($headless)
        try { $dims = "$($img.Width)x$($img.Height)" } finally { $img.Dispose() }
        "monitor 1, $dims PNG"
    })

    [void](Check 'ocr' {
        Add-Type -AssemblyName System.Drawing
        $text = 'Stinkshot Smoke Test 4271'
        $png = Join-Path $WorkDir 'ocr-sample.png'
        $bmp = New-Object Drawing.Bitmap 900, 160
        $g = [Drawing.Graphics]::FromImage($bmp)
        try {
            $g.Clear([Drawing.Color]::White)
            $g.TextRenderingHint = [Drawing.Text.TextRenderingHint]::AntiAliasGridFit
            $font = New-Object Drawing.Font 'Segoe UI', 40
            $g.DrawString($text, $font, [Drawing.Brushes]::Black, 20, 40)
            $font.Dispose()
            $bmp.Save($png, [Drawing.Imaging.ImageFormat]::Png)
        } finally { $g.Dispose(); $bmp.Dispose() }
        $r = Invoke-Exe $Exe @('ocr', '--file', $png) 60
        if ($r.Code -ne 0) { throw "exited with $($r.Code) $($r.Err.Trim())" }
        $got = ($r.Out -replace '\s+', ' ').Trim()
        if ($got -notmatch 'Smoke' -or $got -notmatch '4271') { throw "read '$got', expected '$text'" }
        "read '$got'"
    })

    if (-not $SkipRecord) {
        [void](Check 'record' {
            $rec = Join-Path $TargetDir "$($Triples[$Arch])\release\examples\smoke_record.exe"
            if (-not (Test-Path -LiteralPath $rec)) {
                $saved = $env:CARGO_TARGET_DIR
                $env:CARGO_TARGET_DIR = $TargetDir
                try {
                    $ErrorActionPreference = 'Continue'
                    & cargo build --release --locked -q -p stinkshot-record --example smoke_record --target $Triples[$Arch] 2>&1 | Out-Host
                    if ($LASTEXITCODE -ne 0) { throw "building smoke_record failed ($LASTEXITCODE)" }
                } finally { $env:CARGO_TARGET_DIR = $saved }
            }
            $mp4 = Join-Path $WorkDir 'smoke.mp4'
            # Desktop Duplication allows only a few sessions per output; a busy box (other
            # recorders, soak tests) can refuse one for a moment, so try three times.
            $tries = @()
            for ($i = 1; $i -le 3; $i++) {
                $r = Invoke-Exe $rec @($mp4, '3') 60
                $line = ($r.Out.Trim() -split "`n")[-1].Trim()
                if ($r.Code -eq 0) { return "$line (attempt $i)" }
                $tries += "attempt ${i}: $line $($r.Err.Trim())"
                Start-Sleep -Seconds 5
            }
            throw ($tries -join ' | ')
        })
    }

    if ($script:TrayProc -and -not $script:TrayProc.HasExited) {
        [void](Check 'quit' {
            $r = Invoke-Exe $Exe @('quit') 60
            if ($r.Code -ne 0) { throw "quit exited with $($r.Code) $($r.Err.Trim())" }
            if (-not $script:TrayProc.WaitForExit(10000)) { throw 'process still running 10 s after quit returned' }
            if ((Get-IpcPid) -ne 0) { throw 'IPC window still there' }
            "exit code $($script:TrayProc.ExitCode)"
        })
    }

    if ($Mode -eq 'installer') {
        [void](Check 'uninstall' {
            $p = Start-Tray
            $unins = Join-Path $AppDir 'unins000.exe'
            if (-not (Test-Path -LiteralPath $unins)) { throw "no $unins" }
            $capsBefore = @(Get-Captures | ForEach-Object FullName)
            # The uninstaller copies itself to %TEMP% and returns at once; wait for its effects.
            Start-Process -FilePath $unins -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait | Out-Null
            if (-not (Wait-Until 60 { $p.HasExited -and -not (Test-Path -LiteralPath $Exe) -and -not (Test-Path -LiteralPath $UninstallKey) })) {
                if (-not $p.HasExited) { throw 'the running instance was not closed' }
                if (Test-Path -LiteralPath $Exe) { throw 'stinkshot.exe still installed' }
                throw 'Apps entry still there'
            }
            $script:installed = $false
            if ($capsBefore.Count -eq 0) { throw 'no captures in the history folder to check' }
            # The app may add files on the way out (thumbnails); none may disappear.
            $gone = @($capsBefore | Where-Object { -not (Test-Path -LiteralPath $_) })
            if ($gone.Count) { throw "uninstall removed $($gone.Count) of $($capsBefore.Count) captures: $($gone -join ', ')" }
            $capsAfter = $capsBefore.Count
            if (-not (Test-Path -LiteralPath (Join-Path $ConfigDir 'config.toml'))) { throw 'config removed' }
            $run = Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name "StinkShot-$Profile_" -ErrorAction SilentlyContinue
            if ($run) { throw 'Run value left behind' }
            "closed the app, removed exe and Apps entry, kept $capsAfter capture(s) and config"
        })
    }
}
catch {
    [void]$results.Add([pscustomobject]@{ Check = 'setup'; Result = 'FAIL'; Seconds = 0; Detail = $_.Exception.Message })
    Write-Host "setup FAIL  $($_.Exception.Message)" -ForegroundColor Red
}
finally {
    # Only the process this script started, and only if a check left it running.
    if ($script:TrayProc -and -not $script:TrayProc.HasExited) {
        $q = Start-Process -FilePath $Exe -ArgumentList 'quit' -PassThru -Wait -ErrorAction SilentlyContinue
        if (-not $script:TrayProc.WaitForExit(15000)) { Stop-Process -Id $script:TrayProc.Id -Force -ErrorAction SilentlyContinue }
    }
    if ($installed) {
        $unins = Join-Path $AppDir 'unins000.exe'
        if (Test-Path -LiteralPath $unins) { Start-Process -FilePath $unins -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait | Out-Null }
        [void](Wait-Until 30 { -not (Test-Path -LiteralPath $Exe) })
    }
    $env:STINKSHOT_PROFILE = $savedProfile
    if (-not $Keep) {
        Remove-Item -Recurse -Force -LiteralPath $WorkDir -ErrorAction SilentlyContinue
        Remove-Item -Recurse -Force -LiteralPath $ConfigDir -ErrorAction SilentlyContinue
        Remove-Item -Recurse -Force -LiteralPath (Join-Path $env:LOCALAPPDATA "Stinkshot-$Profile_") -ErrorAction SilentlyContinue
    } else {
        Write-Host "Kept $WorkDir and $ConfigDir"
    }
}

$failed = @($results | Where-Object Result -eq 'FAIL').Count
$results | Format-Table -AutoSize | Out-String -Width 220 | Write-Host
Write-Host ("{0} checks, {1} failed, {2:N0} s" -f $results.Count, $failed, $total.Elapsed.TotalSeconds) -ForegroundColor $(if ($failed) { 'Red' } else { 'Green' })
exit $failed
