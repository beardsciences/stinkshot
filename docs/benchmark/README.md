# Stinkshot benchmark: Stinkshot vs ShareX vs Greenshot vs Flameshot

Run on Nov 7 2026 on the main desktop. Raw samples and summaries are in `results.json`. The harness
(Rust, `C:\dev\stinkshot-bench\harness`) and the portable tools live outside the repo in
`C:\dev\stinkshot-bench`. Re-run everything with `harness\run-all.ps1` (about 30 minutes).

## Results

Median / p90. Lower is better everywhere.

| Metric | Stinkshot | ShareX | Greenshot | Flameshot |
|---|---|---|---|---|
| Start: launch to ready (ms), 15 runs | 843 / 1005 | 898 / 941 | 287 / 526 | 123 / 465 |
| Idle working set after 60 s (MB) | 258 / 258 | 169 / 185 | 71 / 71 | 40 / 40 |
| Idle private bytes (MB) | 620 / 621 | 113 / 148 | 45 / 45 | 8 / 8 |
| Idle handles | 920 / 920 | 839 / 877 | 472 / 472 | 534 / 534 |
| Idle CPU, avg over 60 s (% of one core) | 0.16 / 0.36 | 0.08 / 0.16 | 0.00 / 0.00 | 0.00 / 0.00 |
| Region command to overlay visible (ms), 30 runs | **102 / 461** | 648 / 985 | 383 / 408 | 379 / 692 (monitor picker, see caveats) |
| Region overlay settled at final size (ms) | 227 / 549 | 648 / 985 | 383 / 408 | 379 / 692 |
| Overlay is a frozen screenshot | yes by config, not measurable (see caveats) | yes | yes | not reached |
| Working set before overlay, warm (MB) | 374 | 289 | 116 | 40 (daemon only) |
| Peak working set during overlay + 1 s (MB) | 475 / 481 | 541 / 560 | 354 / 355 | 216 / 216 (incl. picker process) |
| Peak private bytes during overlay (MB) | 761 / 765 | 734 / 743 | 277 / 278 | 112 / 112 |
| Tool CPU per overlay open+close (ms) | 234 / 344 | 766 / 1031 | 469 / 547 | not captured (short-lived process) |
| Full-screen capture to PNG on disk (ms), 20 runs | instance 1350 / 1431; headless 1689 / 1735 | 1021 / 1120 | **817 / 897** | 1504 / 1558 |
| Full-screen image size | 3840x2160 (one monitor) | 6400x2160 (all) | 6400x2160 (all) | 6400x2160 (all) |
| SDR swatch on the HDR monitor, max channel error | 0 (exact) | 0 (exact) | 0 (exact) | 0 (exact) |

Overlay latency, all 30 samples sorted (ms):

- Stinkshot: 25 runs at 88-111, then 157, 416, 461, 570, 584, 601, 630. Bimodal; the slow mode
  is about 19% of runs and did not track background load or which tool ran before it.
- ShareX: 587-739 for 28 runs, then 911-1092.
- Greenshot: 354-417, very tight.
- Flameshot (picker): 344-445 for 31 runs, then 658-972.

## Environment

- AMD Ryzen 5 5600X (6 cores / 12 threads), 32 GB RAM, NVIDIA GeForce RTX 3060 Ti (driver 32.0.16.2101).
- Windows 11 Home 25H2, build 26200.9711, Balanced power plan.
- Monitors: LG HDR 4K, 3840x2160 at 125%, primary, HDR on (10 bpc); OMEN 27i IPS, 2560x1440 at
  100%, SDR, to the right. Virtual screen 6400x2160 physical pixels.
- Background: the PC was in normal use (browser, Discord, the user's own Stinkshot) and the
  Podman/WSL VM was running. Machine-wide CPU outside the benchmarked tools sat at roughly 10-25%
  (median 14-20% per test), mostly `vmmemWSL`.
- Versions: Stinkshot 0.1.0 at commit 6e1485e (release build, separate target dir, `STINKSHOT_PROFILE=bench`);
  ShareX 22.3.0 portable (.NET 10, Avalonia UI); Greenshot 1.3.328 portable (.NET Framework);
  Flameshot 15.0.0 win64 zip (Qt 6.9). SHA256 of each download checked against the GitHub release
  digest (and Flameshot's published `.sha256sum`).

## Method

The harness is a Rust console program using Win32 directly. Times come from
QueryPerformanceCounter. The start time is taken right before `CreateProcess` (or `PostMessage`
for Greenshot) and the end time is when a poller first sees the condition. Window polling runs a
tight loop over `EnumWindows` plus message-only windows (`FindWindowEx(HWND_MESSAGE)`), filtered
to processes whose image path is under `C:\dev\stinkshot-bench`. Measured poll rates were
2.7-5.4 kHz. A `SetWinEventHook(EVENT_OBJECT_SHOW)` hook runs too, as a cross-check. The harness
is per-monitor DPI aware, so all rects are physical pixels.

No synthetic mouse or keyboard input was sent. Overlays were closed with `WM_CLOSE`, and every
close succeeded. Tools were stopped with `TerminateProcess`, after checking that the image path
was under the bench folder. Leftover tray icons were then removed with `NIM_DELETE`.

Background load: before each trial the harness measures machine-wide busy CPU for 500 ms, minus
the benchmarked tools' own CPU. A trial whose background exceeded 35% is flagged and repeated once
at the end, and the summaries use the repeat in place of the flagged sample. A 15% threshold
would have flagged most runs, because of the steady VM load.

### Configuration (each tool's own portable config)

- Stinkshot: `%APPDATA%\Stinkshot-bench\config.toml`. Settings: `take_over_printscreen=false`,
  all hotkeys empty, `copy_to_clipboard=false`, `show_toast=false`, `play_sound=false`, update
  check off, history folder `C:\dev\stinkshot-bench\captures\stinkshot`. Everything else at
  defaults, including OCR indexing.
- ShareX: `Portable` flag file plus `ShareX\ApplicationConfig.json` / `HotkeysConfig.json`.
  Settings: start screen off, update checks off, no hotkeys, after-capture task
  `SaveImageToFile` only (no upload, no clipboard), toasts and sounds off, PNG forced with
  `ImageAutoUseJPEG=false`. The default switches to JPEG above 2 MB, which made the first test
  write a .jpg.
- Greenshot: `greenshot.ini` next to the exe. Settings: `IsFirstLaunch=False`,
  `UpdateCheckInterval=0`, `Destinations=FileNoDialog` into the captures folder, PNG, tray
  notification off, `ScreenCaptureMode=FullScreen` (all monitors). Hotkeys: region
  Ctrl+Alt+Shift+F10, fullscreen Ctrl+Alt+Shift+F11, all others None.
- Flameshot: it has no portable mode on Windows, so the user's `%APPDATA%\flameshot\flameshot.ini`
  was backed up, swapped for `tools\flameshot\flameshot.bench.ini` during runs, and restored
  afterwards. Settings: startup launch, update check, notifications and help off, both shortcuts
  empty, save path fixed to the captures folder. `captureActiveMonitor=true` was also tried, and
  it did not stop the monitor picker.

### Definitions

**Ready** (test 1). The process is killed between runs, so OS file caches stay warm ("cold-ish").
The ready event for each tool:

- Stinkshot: the `tray_icon_app` window exists. The tray-icon crate creates it right before
  `Shell_NotifyIcon(NIM_ADD)`. It appears at about 1.3 s, after the wgpu device (about 1.0 s) and
  the IPC window (about 0.4 s).
- ShareX: the main window gets its title "ShareX 22.3 Portable". This lands within a few ms of
  ShareX's own log line "Startup time: 831 ms" and after "HotkeyManager started".
- Greenshot: the main form gets its title "Greenshot - the revolutionary screenshot utility". The
  form owns the hotkeys and the tray icon.
- Flameshot: the daemon creates `Qt693TrayIconMessageWindowClass`. This is QSystemTrayIcon's
  callback window, created when the tray icon is shown.

These events are comparable ("the tray icon is up / the app can take commands"), but they are not
identical internal milestones.

**Visible** (test 3). The first poll that sees a window from the tool that is visible, not cloaked,
and covers at least 95% of some monitor. For Flameshot this also includes its topmost
"flameshot" monitor-picker window. **Settled** is the last rect change within 600 ms after
visible. Stinkshot first shows its overlay at 8000x2700 (6400x2160 scaled by 125%) and resizes it
to 6400x2160 about 125 ms later. The other tools show the overlay at its final size.

**Frozen.** A small window of the harness animates one color patch every 15 ms. While the overlay
is up, the harness reads that pixel from the screen DC five times, 40 ms apart. If the color stays
the same, the overlay is a frozen screenshot.

**Written** (test 4). The command is issued, then the harness polls the output folder every
millisecond for a .png whose last 12 bytes are the IEND chunk. The file must also keep the same
size for 300 ms afterwards. The triggers per tool:

- Stinkshot: `stinkshot full` sent to the running instance, which saves to history. Also
  `stinkshot full --out <file> --monitor 1`, headless, which starts a fresh process each time.
- ShareX: `ShareX.exe -PrintScreen`.
- Greenshot: `WM_HOTKEY` for its fullscreen hotkey.
- Flameshot: `flameshot full -p <dir>`.

**Swatch check.** The harness opens a window of six sRGB patches (pure R, G, B, 50% gray, white,
200/150/50) on the HDR monitor. It then compares the center pixel of each patch in the first
three PNGs of each variant with the source values.

**Memory/CPU around the overlay** (test 5). A sampler thread sums working set and private bytes
over the tool's processes every 5 ms, from trigger until 1 s after close. The table reports the
peak. CPU is the tool's process CPU time over the same window.

## Caveats

- **Greenshot has no command line for captures.** Its CLI only accepts `/help`, `/exit`,
  `/reload` and `/language`. Both Greenshot triggers post `WM_HOTKEY` (id 1 = region, 2 =
  fullscreen) to its main form, which is what Windows does when the hotkey is pressed. Greenshot
  skips process creation and argument forwarding, which the other tools pay for. This makes its
  latencies a lower bound compared with the CLI-driven tools.
- **Flameshot 15 never reached its region overlay.** On this mixed-DPI layout, `flameshot gui`
  first shows a monitor picker ("Click or press 1 to select"), and choosing a monitor needs input.
  The Flameshot overlay numbers are command-to-picker-visible, not command-to-overlay. This picker
  is an extra step for real users too. The `--region WxH+X+Y` option still showed the picker.
  `--region screen0` and `--region all` were rejected as invalid, although the help text lists
  them. Flameshot's overlay CPU and peak memory come from a separate short-lived `gui` process:
  CPU could not be captured, and the memory peak may be understated.
- **Stinkshot's overlay contents cannot be checked from outside.** Its overlay window sets
  `WDA_EXCLUDEFROMCAPTURE` (`crates/app/src/overlay/mod.rs`). Screen-DC reads, BitBlt and desktop
  duplication all see the desktop behind it, so the frozen check reads the live desktop. "Frozen"
  for Stinkshot comes from its config (`freeze_screen=true`), not from a measurement. For the
  same reason, the harness cannot tell when the overlay first paints. "Visible" means shown and
  sized, for every tool.
- **Full-screen captures differ in scope.** Stinkshot captures one monitor: the one under the
  cursor for `full`, or `--monitor 1` for headless. That is 8.3 MP. The others capture the whole
  6400x2160 desktop, 13.8 MP. Per megapixel, Stinkshot is slower still. Stinkshot's headless time
  includes starting the process every run.
- **Clipboard and toasts were off for every tool**, so the user's clipboard was left alone.
  Real-world capture-to-done times with clipboard copy enabled will be a little longer.
- **The swatch test covers SDR content on the HDR monitor**, which is the common case. All four
  tools returned exact sRGB values for it. Real HDR content (an HDR video or game) was not tested.
- **The test PC was in use**, with a steady 10-25% machine-wide load from the WSL VM. Medians are
  robust to this; the max values and some p90s are not.
- **Idle footprint** was measured with all four tools running at once, after 60 s idle; CPU is
  the average over the next 60 s, in 3 repetitions. CPU time comes from `GetProcessTimes`, so
  values under 0.1% of a core are at the limit of its resolution.
- **Ghost tray icons.** Killing a process leaves its tray icon behind. The harness removes them
  with `NIM_DELETE` (uID 0-31 per window), on a best-effort basis.

## Surprising findings

1. **Stinkshot opens its region overlay 3.7-6.4x faster than the others** (median 102 ms against
   379-648 ms). It also has a slow mode of 0.4-0.6 s in about 1 run in 5. The cause was not
   isolated. One possibility is that the duplication session is rebuilt (`capture.rs`
   `Session::create`, "several hundred ms").
2. **Stinkshot is the heaviest tool at idle**: 258 MB working set and 620 MB private bytes,
   against 40-169 MB / 8-113 MB for the others. Most of this is likely the pre-warmed
   desktop-duplication session (shadow and staging textures for a 4K FP16 HDR output) plus the
   wgpu device. Startup is about 0.85 s to tray. In the window probe, the tray window is created
   only after the wgpu device window, so GPU init sits on the startup path.
3. **Stinkshot full-screen capture is the slowest of the CLI paths per pixel.** It takes 1.35 s
   through the instance for one 4K monitor, while Greenshot needs 0.82 s for both monitors. PNG
   encoding is the likely cost; it was not profiled.
4. **A Stinkshot instance capture returned a stale frame in a trial run.** In a 2-per-variant
   trial, both `stinkshot full` captures through the running instance lacked the swatch window.
   The swatch had been open for 15 s or more before those captures. Both images instead showed the
   screen as it was when the instance started, including Flameshot's startup toast. The headless
   capture taken in the same trial showed the swatch. The crops are in `evidence\` in the bench
   folder. The full run (0 of 20) and a targeted re-check (0 of 4, `raw.stale_check`) did not
   reproduce it. The code path fits the symptom: the warm-up seeds the shadow texture from GDI,
   and `refresh` returns that shadow on a duplication timeout. This is worth a look, because it
   would also freeze a stale image into the region overlay.
5. **ShareX 22 is a different app from classic ShareX.** It runs on .NET 10 with an Avalonia UI.
   By default it saves large screenshots as JPEG with random file names (`ImageAutoUseJPEG`), and
   its region overlay is the slowest here (median 648 ms).
6. **Flameshot 15 on Windows asks which monitor to use before every region capture** when the
   monitors have mixed DPI. Its documented `--region screenN` / `all` forms are rejected as
   invalid.
