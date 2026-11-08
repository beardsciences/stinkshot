# Stinkshot benchmark: Stinkshot vs ShareX vs Greenshot vs Flameshot

Second run, Nov 7 2026 (20:00-20:30), on the main desktop. Raw samples and summaries are in
`results.json`. The harness (Rust, `C:\dev\stinkshot-bench\harness`) and the portable tools live
outside the repo in `C:\dev\stinkshot-bench`. Re-run everything with `harness\run-all.ps1`
(about 35 minutes), then `python -I harness\make-results.py`. The first run (17:05, before the
app and capture fixes) is kept in `C:\dev\stinkshot-bench\results-first-run`.

## Headline

**Overlay ready** (region command issued until the tool's overlay window is shown at its final
size), median / p90, 30 runs each, all tools in the same session:

| | Overlay ready (ms) | Slow runs |
|---|---|---|
| Stinkshot, current main (b5409a9) | **220 / 547** | 4 of 30 (547-949 ms) |
| Stinkshot, build of the first run (3d3c876), same session | 238 / 254 | 1 of 30 (543 ms) |
| ShareX 22.3.0 | 644 / 1028 | 6 of 30 (923-1089 ms) |
| Greenshot 1.3.328 (window message, no process start, see caveats) | 382 / 400 | 0 of 30 |
| Flameshot 15.0.0 (monitor picker, not the overlay, see caveats) | 357 / 387 | 1 of 30 (764 ms) |

On this machine, in this session, 25 of Stinkshot's 30 runs (197-266 ms) were faster than any
run of the other tools (fastest 329 ms), and its median was 1.6-2.9x lower than theirs. Its p90 is not: 4 of
30 runs took 0.5-0.95 s, above Greenshot's and Flameshot's p90. The current build's median was
18 ms below the old build's, while the displays were off (see caveats); the main change is that
the window now shows up at its final size, not oversized first. Idle CPU was low for all of them
(ranges below).

## Results

Median / p90 unless marked as a range. Lower is better everywhere. "Stinkshot" is current main;
"Stinkshot (old)" is commit 3d3c876, the code the first run measured, run in this session as a
same-conditions baseline.

| Metric | Stinkshot | Stinkshot (old) | ShareX | Greenshot | Flameshot |
|---|---|---|---|---|---|
| **Overlay ready (ms)**, 30 runs | **220 / 547** | 238 / 254 | 644 / 1028 | 382 / 400 | 357 / 387 (picker) |
| Slow runs (more than 150 ms above the median) | 4 of 30 | 1 of 30 | 6 of 30 | 0 of 30 | 1 of 30 |
| Overlay first visible, any size (ms) | 220 / 547 | 106 / 124 | 644 / 1028 | 382 / 400 | 357 / 387 |
| Runs where the window changed after it became visible | 0 of 30 | 30 of 30 | 0 of 30 | 0 of 30 | 0 of 30 |
| Start: launch to ready (ms), 15 runs | 32 / 36 | 786 / 856 | 815 / 909 | 260 / 269 | 113 / 206 |
| Idle working set after 60 s (MB), range of 3 | 117-118 | 253-254 | 163-164 | 67 | 38 |
| Idle private bytes (MB), range of 3 | 164-166 (displays off, see caveats) | 620-622 | 113-114 | 45 | 8 |
| Idle handles, range of 3 | 704 | 917 | 836-837 | 469 | 529 |
| Idle CPU over 60 s (% of one core), range of 3 | 0.05-0.23 | 0.08-0.26 | 0.05-0.08 | 0.00 | 0.00 |
| Working set before the overlay, warm (MB) | 149 / 204 | 366 / 368 | 284 / 330 | 112 / 112 | 38 (daemon only) |
| Peak working set, overlay + 1 s (MB) | 325 / 361 | 472 / 474 | 541 / 588 | 349 / 350 | 211 / 211 (incl. picker process) |
| Peak private bytes during the overlay (MB) | 425 / 458 | 762 / 764 | 736 / 748 | 277 / 277 | 112 / 112 |
| Tool CPU per overlay open + close (ms) | 156 / 281 | 211 / 406 | 797 / 1078 | 477 / 531 | not captured |
| Full-screen capture to PNG on disk (ms), 20 runs | instance 168 / 175; headless 161 / 172 | instance 1460 / 1649; headless 2352 / 2702 | 1031 / 1066 | 834 / 864 | 1625 / 2031 |
| Full-screen image size | 3840x2160 (one monitor) | 3840x2160 | 6400x2160 (all) | 6400x2160 (all) | 6400x2160 (all) |
| SDR swatch on the HDR monitor, max channel error | 0 (instance and headless) | instance **194 (stale frame)**; headless 0 | 0 | 0 | 0 |

Overlay ready, all 30 samples sorted (ms):

- Stinkshot: 197-266 for 25 runs, then 326, 547, 618, 643, 949.
- Stinkshot (old): 190-254 for 27 runs, then 287, 297, 543.
- ShareX: 603-705 for 24 runs, then 923-1089.
- Greenshot: 362-414.
- Flameshot (picker): 329-387 for 27 runs, then 445, 469, 764.

### Stinkshot before -> after

| Metric | First run (17:05, old build) | This run, old build | This run, current main |
|---|---|---|---|
| Overlay ready (ms) | 227 / 549 (old "settled") | 238 / 254 | 220 / 547 |
| Slow overlay runs | about 1 in 5 | 1 of 30 | 4 of 30 |
| Launch to ready (ms) | 843 / 1005 | 786 / 856 | 32 / 36 |
| Idle private bytes (MB) | 620 / 621 | 620-622 | 164-166 |
| Idle CPU (% of one core) | 0.16 / 0.36 (median / p90) | 0.08-0.26 | 0.05-0.23 |
| Full-screen via instance (ms) | 1350 / 1431 | 1460 / 1649 | 168 / 175 |
| Instance capture shows the current screen | yes in the run, no in a trial | no (swatch error 194) | yes |

The first run's "settled" is the closest match to "ready": it is the last rect change, but it
missed hide/re-show (the old build hid and re-showed its overlay 80-140 ms after showing it at
8000x2700). The first run most likely had the displays on; this run had them off.

## Environment

- AMD Ryzen 5 5600X (6 cores / 12 threads), 32 GB RAM, NVIDIA GeForce RTX 3060 Ti (driver 32.0.16.2101).
- Windows 11 Home 25H2, build 26200.9711, Balanced power plan.
- Monitors: LG HDR 4K, 3840x2160 at 125%, primary, HDR on (10 bpc); OMEN 27i IPS, 2560x1440 at
  100%, SDR, to the right. Virtual screen 6400x2160 physical pixels.
- **Displays off.** The console display state was 0 (off, idle timeout) before the run, at the
  start of the idle step and after the run. Nobody was at the PC.
- Background: the user's own Stinkshot, browser, Discord and the Podman/WSL VM were running.
  Machine-wide CPU outside the benchmarked tools, measured for 500 ms before each trial, had a
  median of 15% (start-up), 21% (overlay, max 48%) and 21% (full-screen); 22-23% during the idle
  windows.
- Versions: Stinkshot 0.1.0 at commit b5409a9 (release, `CARGO_TARGET_DIR=stinkshot-target`,
  `STINKSHOT_PROFILE=bench`) and at commit 3d3c876 (release from `git archive` into
  `src-before`, `stinkshot-before-target`, profile `benchbefore`, same settings, own history
  folder); ShareX 22.3.0 portable (.NET 10, Avalonia UI); Greenshot 1.3.328 portable (.NET
  Framework); Flameshot 15.0.0 win64 zip (Qt 6.9). SHA256 of each download checked against the
  GitHub release digest (and Flameshot's published `.sha256sum`).

## Method

The harness is a Rust console program using Win32 directly. Times come from
QueryPerformanceCounter. The start time is taken right before `CreateProcess` (or `PostMessage`
for Greenshot) and the end time is when a poller first sees the condition. Window polling runs a
tight loop over `EnumWindows` plus message-only windows (`FindWindowEx(HWND_MESSAGE)`), filtered
to processes whose image path is under the tool's folder in `C:\dev\stinkshot-bench`. The
harness is per-monitor DPI aware, so all rects are physical pixels. Trials are interleaved
across tools in a shuffled order with fixed seeds.

No synthetic mouse or keyboard input was sent. Overlays were closed with `WM_CLOSE`, and every
close succeeded. Tools were stopped with `TerminateProcess`, after checking that the image path
was under the bench folder. Leftover tray icons were then removed with `NIM_DELETE`.

Background load: before each trial the harness measures machine-wide busy CPU for 500 ms, minus
the benchmarked tools' own CPU. A trial whose background exceeded 35% is flagged and repeated once
at the end, and the summaries use the repeat in place of the flagged sample (overlay: 5 of 150
trials flagged).

### Configuration (each tool's own portable config)

- Stinkshot (both builds): `%APPDATA%\Stinkshot-bench\config.toml` and
  `%APPDATA%\Stinkshot-benchbefore\config.toml`. Settings: `take_over_printscreen=false`, all
  hotkeys empty, `copy_to_clipboard=false`, `show_toast=false`, `play_sound=false`, update check
  off, history folder under `C:\dev\stinkshot-bench\captures`. Everything else at defaults,
  including OCR indexing and `freeze_screen=true`.
- ShareX: `Portable` flag file plus `ShareX\ApplicationConfig.json` / `HotkeysConfig.json`.
  Settings: start screen off, update checks off, no hotkeys, after-capture task
  `SaveImageToFile` only (no upload, no clipboard), toasts and sounds off, PNG forced with
  `ImageAutoUseJPEG=false`.
- Greenshot: `greenshot.ini` next to the exe. Settings: `IsFirstLaunch=False`,
  `UpdateCheckInterval=0`, `Destinations=FileNoDialog` into the captures folder, PNG, tray
  notification off, `ScreenCaptureMode=FullScreen` (all monitors). Hotkeys: region
  Ctrl+Alt+Shift+F10, fullscreen Ctrl+Alt+Shift+F11, all others None.
- Flameshot: no portable mode on Windows, so `run-all.ps1` backs up the user's
  `%APPDATA%\flameshot\flameshot.ini`, swaps in `tools\flameshot\flameshot.bench.ini` for the
  run and restores it in a `finally` block. Settings: startup launch, update check,
  notifications and help off, both shortcuts empty, save path fixed to the captures folder.
- After the run `harness\verify-baseline.ps1` compared the HKCU Run and Keyboard keys and
  `%APPDATA%\flameshot` with the pre-benchmark exports in `..\baseline`: all unchanged, no bench
  process left (`baseline-check.txt`).

### Definitions

**Overlay ready** (headline). The harness polls until a window of the tool is visible, not
cloaked, and covers at least 95% of some monitor ("first visible"). It then keeps polling at
the same rate for 600 ms and records every change of the shown overlay (which window, its rect,
or no overlay shown at all). The final rect is the one the overlay has at the end of those
600 ms. **Ready is the start of the last unbroken stretch in which the overlay is shown at that
final rect**, so a window counts only once it is at its final size, and a hide and re-show
restarts the clock. For a tool that shows its overlay at the final size straight away, ready
equals first visible. For Flameshot the topmost "flameshot" monitor-picker window also counts
(it is the first UI its region command produces). The first run's "visible" counted the old
Stinkshot overlay while it was still 8000x2700 (6400x2160 scaled by 125% again), and its
"settled" ignored the hide and re-show; this run's ready replaces both.

**Slow runs.** Runs more than 150 ms above the tool's own median. Every tool except
Greenshot had a small cluster of runs 0.3-0.7 s above its main mode.

**Ready** (start-up). The process is killed between runs, so OS file caches stay warm. The
event per tool: Stinkshot, the `tray_icon_app` window (created right before
`Shell_NotifyIcon(NIM_ADD)`; current main creates it before fonts, history and the GPU);
ShareX, the main window gets its title "ShareX 22.3 Portable"; Greenshot, the main form gets its
title (it owns the hotkeys and tray icon); Flameshot, the daemon's
`Qt693TrayIconMessageWindowClass`. These are comparable ("the tray icon is up / the app can take
commands"), not identical internal milestones. For current Stinkshot the tray is up before the
app can paint (eframe is up at about 0.5-0.6 s); a click or hotkey before that waits in the
queue.

**Idle.** All five tools running at once, footprint after 60 s idle, CPU averaged over the next
60 s, 3 repetitions; reported as the range over the 3. CPU time comes from `GetProcessTimes`, so
values under 0.1% of a core are at the limit of its resolution.

**Memory/CPU around the overlay.** A sampler thread sums working set and private bytes over the
tool's processes every 5 ms, from trigger until 1 s after close; the table reports the peak. CPU
is the tool's process CPU time over the same window.

**Written** (full-screen). The command is issued, then the harness polls the output folder every
millisecond for a .png whose last 12 bytes are the IEND chunk, unchanged for 300 ms afterwards.
Triggers: Stinkshot `stinkshot full` to the running instance (saves to history) and
`stinkshot full --out <file> --monitor 1` (headless, new process each time); ShareX
`ShareX.exe -PrintScreen`; Greenshot `WM_HOTKEY` for its fullscreen hotkey; Flameshot
`flameshot full -p <dir>`.

**Swatch check.** A harness window of six sRGB patches sits on the HDR monitor and is open
before the tools start; the first three PNGs per variant are checked at each patch center.

**Frozen.** A small harness window animates one color patch every 15 ms; while the overlay is
up the harness reads that pixel from the screen DC five times, 40 ms apart. ShareX and Greenshot
were frozen in 30 of 30 runs each. Stinkshot cannot be checked (below); Flameshot never reached
its overlay.

## Caveats

- **Triggering differs between tools.** Stinkshot, ShareX and Flameshot were triggered through
  their command line, so every sample includes starting a short-lived process that forwards the
  command to the running instance. Greenshot has no capture command line (its CLI only accepts
  `/help`, `/exit`, `/reload`, `/language`), so the harness posts `WM_HOTKEY` straight to its
  main form, which skips process creation; its times are a lower bound against the others. None
  of these is a real key press: no tool was measured from PrtSc or a hotkey, which is how most
  people trigger them.
- **The slow mode is process start-up, as far as it was checked.** Stinkshot's own trace
  (docs/notes/app-shell.md) put 98-181 ms inside the instance on every run, and the extra
  0.2-0.6 s of slow runs before the forwarding client reached `main`; unrelated exes
  (`whoami.exe`) show the same tail on this machine. ShareX and Flameshot show a similar
  cluster. A hotkey or tray trigger does not start a process, so it should not have this tail;
  that was not measured.
- **Flameshot 15 never reached its region overlay.** On this mixed-DPI layout `flameshot gui`
  first shows a monitor picker ("Click or press 1 to select"), which needs input. Its "overlay
  ready" is command to picker shown at its final rect (1135x441), not to an overlay. Real users
  also go through the picker. Its overlay CPU and peak memory come from a short-lived `gui`
  process; CPU could not be captured and the memory peak may be understated.
- **The displays were off for the whole run.** Desktop Duplication delivers no frames then.
  Current Stinkshot detects this and takes every freeze from GDI (about 90-150 ms for both
  monitors); with the displays on it uses its warm duplication session (about 40-60 ms), so its
  overlay times here are likely higher than with the displays on (an earlier unattended run
  measured about 212 ms with the displays off against about 120 ms "visible" with them on,
  docs/notes/platform.md). The old build does not detect it and hands back the duplication's
  last picture, which is fast but stale: its full-screen captures through the instance missed
  the swatch window (error 194) in 3 of 3 checks, and its frozen overlay picture would have been
  stale the same way. ShareX, Greenshot and Flameshot capture with GDI either way.
- **Stinkshot's idle memory depends on the display state.** With the displays off the warm
  capture session never receives a frame, so its frame textures are not filled. The app track
  measured 400 MB idle private bytes for its build, before the capture changes were merged
  (display state not recorded then; docs/notes/app-shell.md). Treat 164 MB as a lower bound for current main; the
  old build's 620 MB matched the first run.
- **Stinkshot's overlay contents cannot be checked from outside.** Its overlay sets
  `WDA_EXCLUDEFROMCAPTURE`, so screen-DC reads see the desktop behind it. "Frozen" for Stinkshot
  comes from its config (`freeze_screen=true`). For the same reason the harness cannot see when
  the overlay first paints; "ready" means shown at its final rect for every tool. Stinkshot
  keeps the window cloaked until two frames have been presented (docs/notes/app-overlay.md),
  so for it shown means painted; the other tools were not checked for a blank first frame.
- **Full-screen captures differ in scope.** Stinkshot captures one monitor (the one under the
  cursor for `full`, `--monitor 1` headless): 8.3 MP. The others capture the whole 6400x2160
  desktop, 13.8 MP. Stinkshot's headless time includes starting the process every run.
- **Clipboard and toasts were off for every tool**, so the user's clipboard was left alone.
  Real capture-to-done times with clipboard copy on will be a little longer.
- **The swatch test covers SDR content on the HDR monitor.** All tools except the old
  Stinkshot instance path returned exact sRGB values. Real HDR content was not tested.
- **The test PC was in use**, with a steady 15-23% machine-wide background load. Medians are
  robust to this; maxima and some p90s are not.
- **`run.log` is incomplete.** A `tail -F` on it during the run blocked the harness's
  `Add-Content` writes, so the per-trial lines of the idle, overlay and full-screen steps are
  missing. The raw JSON files are complete.
- **Ghost tray icons.** Killing a process leaves its tray icon behind; the harness removes them
  with `NIM_DELETE` (uID 0-31 per window), best effort.

## Findings

1. **The oversized overlay is fixed.** The old build showed its overlay at 8000x2700, resized
   it, then hid and re-showed it, in 30 of 30 runs; current main shows it once, at 6400x2160,
   in 30 of 30. Counted at the final size, the old build's 106 ms "first visible" becomes 238 ms
   ready.
2. **Launch to tray dropped from 786 ms to 32 ms**, now below Flameshot (113 ms) and Greenshot
   (260 ms). The app is still not able to paint until about 0.5 s.
3. **Full-screen capture through the instance dropped from 1460 ms to 168 ms** (one 4K monitor)
   and now shows the current screen with the displays off; the old build returned a stale
   frame in every check.
4. **Idle footprint**: current main idled at 117 MB working set / 164 MB private with the
   displays off, against 253 / 620 MB for the old build. ShareX (163 / 113), Greenshot (67 / 45)
   and Flameshot (38 / 8) are still smaller on private bytes. Idle CPU was 0.05-0.23% of a core
   for current Stinkshot, 0.05-0.08% for ShareX and below the timer resolution for Greenshot and
   Flameshot.
5. **Stinkshot's slow overlay runs remain** (4 of 30 here, 1 of 30 for the old build in the same
   session). They come and go with the machine's state; the evidence points at process start-up
   for CLI triggers, not the app.
