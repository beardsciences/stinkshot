# Stinkshot benchmark: Stinkshot vs ShareX vs Greenshot vs Flameshot

Third run, Nov 7 2026 (22:32-23:00), on the main desktop, **displays on for every number in
this file unless a row says otherwise**. Stinkshot is main at f334f45 (GPU-loss recovery,
smaller idle memory, faster cold start). All four tools were interleaved in one batch. Raw
samples, summaries and the display state per step are in `results.json`.

The harness (Rust, `C:\dev\stinkshot-bench\harness-mem`) and the portable tools live outside the
repo in `C:\dev\stinkshot-bench`. Re-run with `harness-mem\run-final.ps1` (about 30 minutes),
then `python -I harness-mem\make-results.py`. Earlier runs are kept for reference:
`results-second-run` (20:00, b5409a9, displays off) and `results-first-run` (17:05, 3d3c876).

## Display state

The console display state (0 off, 1 on, 2 dimmed) was logged before every step and every 5 s
during the run (`display-state.log` next to the raw results; `display_state_per_step` in
`results.json`). It was 1 (on) in all 324 samples (22:32:17-22:59:53), before every step and
after the last. Nothing was done to keep the displays on: no input, no execution-state
request. With a 15-minute display timeout that means the PC was in use (background load
below).

## Headline

**Overlay ready** (region command issued until the tool's overlay window is shown at its final
size), median / p90, 30 runs each, displays on:

| | Overlay ready (ms) | Slow runs |
|---|---|---|
| Stinkshot f334f45 | **134 / 289** | 4 of 30 (289-488 ms) |
| ShareX 22.3.0 | 626 / 771 | 3 of 30 (784-1470 ms) |
| Greenshot 1.3.328 (window message, no process start: a lower bound, see caveats) | 329 / 348 | 0 of 30 |
| Flameshot 15.0.0 (its monitor picker, not an overlay, see caveats) | 354 / 785 | 6 of 30 (644-900 ms) |

26 of Stinkshot's 30 runs (122-156 ms) were faster than any run of the other tools (fastest
313 ms, Greenshot). Its median was 2.5x lower than Greenshot's, 2.6x lower than Flameshot's
picker and 4.7x lower than ShareX's, and its p90 was the lowest of the four. Its worst run
(488 ms) was slower than Greenshot's worst (375 ms). Greenshot is the fairest comparison point
only with its trigger advantage in mind: it skips the process start that every other tool's
sample includes.

## Results

Median / p90 unless marked as a range. Lower is better everywhere. Displays on for every row.

| Metric | Stinkshot | ShareX | Greenshot | Flameshot |
|---|---|---|---|---|
| **Overlay ready (ms)**, 30 runs | **134 / 289** | 626 / 771 | 329 / 348 | 354 / 785 (picker) |
| Slow runs (more than 150 ms above the median) | 4 of 30 | 3 of 30 | 0 of 30 | 6 of 30 |
| Runs where the window changed after it became visible | 0 of 30 | 0 of 30 | 0 of 30 | 0 of 30 |
| Launch to ready (tray / main window up, ms), 15 runs | 34 / 490 | 861 / 1033 | 297 / 659 | 117 / 370 |
| Cold launch to first overlay ready (ms), 10 runs | 593 / 643 (region about 40 ms after launch); 568 / 590 (region right after the tray) | not measured | not measured | not measured |
| Idle working set after 60 s (MB), range of 3 | 120 | 163-164 | 68 | 33-38 |
| Idle private bytes after 60 s (MB), range of 3 | 134-135 | 112-114 | 45 | 6-8 |
| Idle handles, range of 3 | 707-709 | 835-836 | 468 | 447-528 |
| Idle CPU over 60 s (% of one core), range of 3 | 0.00-0.13 | 0.00-0.08 | 0.00 | 0.00 |
| Working set before the overlay, warm (MB) | 149 / 204 | 274 / 281 | 112 / 113 | 38 (daemon only) |
| Peak working set, overlay + 1 s (MB) | 279 / 335 | 533 / 541 | 349 / 350 | 211 / 211 (incl. picker process) |
| Peak private bytes during the overlay (MB) | 414 / 477 | 725 / 730 | 277 / 277 | 112 / 112 |
| Tool CPU per overlay open + close (ms) | 148 / 250 | 781 / 1141 | 312 / 469 | not captured |
| Full-screen capture to PNG on disk (ms), 20 runs | 156 / 465 (headless, one 4K monitor) | 1037 / 1445 | 785 / 827 | 1546 / 1706 |
| Captured area | 3840x2160, 8.3 MP | 6400x2160, 13.8 MP | 6400x2160, 13.8 MP | 6400x2160, 13.8 MP |
| PNG size, median | 8.58 MB | 9.49 MB | 10.65 MB | 7.50 MB |
| SDR swatch on the HDR monitor, max channel error | 0 | 0 | 0 | 0 |

Stinkshot's `full` through the running instance (20 runs, 52 / 415 ms) captured the monitor
under the cursor, which was the 2560x1440 SDR monitor for this run (3.7 MP, 1.02 MB PNGs, no
swatch on it), so it is not in the table; see caveats.

All samples sorted (ms):

- Overlay ready. Stinkshot: 122-156 for 26 runs, then 289, 436, 448, 488. ShareX: 565-701 for
  26, then 771, 784, 1259, 1470. Greenshot: 313-375. Flameshot (picker): 323-385 for 23, then
  473, 644, 665, 785, 792, 854, 900.
- Launch to ready. Stinkshot: 31-42 for 11, then 311, 430, 490, 764. ShareX: 813-1033, then
  2014. Greenshot: 264-321 for 10, then 461-752. Flameshot: 99-122 for 10, then 175-517.
- Cold launch to overlay, region about 40 ms after launch (30 ms sleep after `CreateProcess`
  returned; 37-40 ms measured, once 88): 546-675. Region 1 ms after the tray window exists
  (32-43 ms after launch): 549-592.
- Full-screen to PNG. Stinkshot headless: 151-177 for 16, then 245, 465, 593, 607. ShareX:
  995-1113 for 17, then 1445-1454. Greenshot: 760-845. Flameshot: 1492-1952.

### Stinkshot across the three runs

| Metric | First run (17:05, 3d3c876) | Second run (20:00, b5409a9) | Third run (22:32, f334f45) |
|---|---|---|---|
| Display state | most likely on (not logged) | **off** for the whole run | **on** for the whole run |
| Overlay ready (ms) | 227 / 549 (old "settled") | 220 / 547 | 134 / 289 |
| Launch to ready (ms) | 843 / 1005 | 32 / 36 | 34 / 490 |
| Idle private bytes (MB) | 620 / 621 | 164-166 (displays off) | 134-135 |
| Full-screen to PNG (ms) | 1350 / 1431 (instance) | 168 / 175 (instance), 161 / 172 (headless) | 156 / 465 (headless) |

The second run's 164 MB was not a lower bound for b5409a9: with the displays off its warm
capture session got no frames. The memory work that followed measured the same b5409a9 build at
399 MB idle private bytes with the displays on (docs/notes/app-shell.md, "GPU device loss,
memory and cold start"); f334f45 idles at 134 MB with the displays on, matching that work's
134.2 / 134.3 MB.

## Environment

- AMD Ryzen 5 5600X (6 cores / 12 threads), 32 GB RAM, NVIDIA GeForce RTX 3060 Ti (driver
  32.0.16.2101).
- Windows 11 Home 25H2, build 26200.9711, Balanced power plan.
- Monitors: LG HDR 4K, 3840x2160 at 125%, primary, HDR on (10 bpc); OMEN 27i IPS, 2560x1440 at
  100%, SDR, to the right. Virtual screen 6400x2160 physical pixels.
- **Displays on** (state 1 in every sample, see above).
- Background: the user's own Stinkshot (not touched), browser, Discord and the Podman/WSL VM
  were running. Machine-wide CPU outside the benchmarked tools, measured for 500 ms before each
  trial, had a median of 12-21% per tool for start-up, 14-19% for the overlay (max 34%) and
  11-20% for full-screen.
- Versions: Stinkshot 0.1.0 at commit f334f45 (release, `CARGO_TARGET_DIR=stinkshot-target`,
  `STINKSHOT_PROFILE=bench`); ShareX 22.3.0 portable (.NET 10, Avalonia UI); Greenshot 1.3.328
  portable (.NET Framework); Flameshot 15.0.0 win64 zip (Qt 6.9). SHA256 of each download checked
  against the GitHub release digest (and Flameshot's published `.sha256sum`).

## Method

The harness is a Rust console program using Win32 directly. Times come from
QueryPerformanceCounter. The start time is taken right before `CreateProcess` (or `PostMessage`
for Greenshot) and the end time is when a poller first sees the condition. Window polling runs a
tight loop over `EnumWindows` plus message-only windows (`FindWindowEx(HWND_MESSAGE)`), filtered
to processes whose image path is under the tool's folder in `C:\dev\stinkshot-bench`. The
harness is per-monitor DPI aware, so all rects are physical pixels. Trials are interleaved
across the four tools in a shuffled order with fixed seeds.

No synthetic mouse or keyboard input was sent. Overlays were closed with `WM_CLOSE`, and every
close succeeded. Tools were stopped with `TerminateProcess`, after checking that the image path
was under the bench folder. Leftover tray icons were then removed with `NIM_DELETE`.

Background load: before each trial the harness measures machine-wide busy CPU for 500 ms, minus
the benchmarked tools' own CPU. A trial whose background exceeded 35% is flagged and repeated
once at the end, and the summaries use the repeat in place of the flagged sample (overlay: 4 of
120 trials; start-up: 7 of 60).

### Configuration (each tool's own portable config)

- Stinkshot: `%APPDATA%\Stinkshot-bench\config.toml`. Settings: `take_over_printscreen=false`,
  all hotkeys empty, `copy_to_clipboard=false`, `show_toast=false`, `play_sound=false`, update
  check off, history folder under `C:\dev\stinkshot-bench\captures`. Everything else at
  defaults, including OCR indexing and `freeze_screen=true`.
- ShareX: `Portable` flag file plus `ShareX\ApplicationConfig.json` / `HotkeysConfig.json`.
  Settings: start screen off, update checks off, no hotkeys, after-capture task
  `SaveImageToFile` only (no upload, no clipboard), toasts and sounds off, PNG forced with
  `ImageAutoUseJPEG=false`.
- Greenshot: `greenshot.ini` next to the exe. Settings: `IsFirstLaunch=False`,
  `UpdateCheckInterval=0`, `Destinations=FileNoDialog` into the captures folder, PNG, tray
  notification off, `ScreenCaptureMode=FullScreen` (all monitors). Hotkeys: region
  Ctrl+Alt+Shift+F10, fullscreen Ctrl+Alt+Shift+F11, all others None.
- Flameshot: no portable mode on Windows, so `run-final.ps1` backs up the user's
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
restarts the clock. In this run every tool showed its overlay at its final size straight away,
so ready equals first visible for all four. For Flameshot the topmost "flameshot"
monitor-picker window also counts (it is the first UI its region command produces).

**Slow runs.** Runs more than 150 ms above the tool's own median.

**Launch to ready.** The process is killed between runs, so OS file caches stay warm. The event
per tool: Stinkshot, the `tray_icon_app` window (created right before
`Shell_NotifyIcon(NIM_ADD)`, before fonts, history and the GPU); ShareX, the main window gets
its title "ShareX 22.3 Portable"; Greenshot, the main form gets its title (it owns the hotkeys
and tray icon); Flameshot, the daemon's `Qt693TrayIconMessageWindowClass`. These are comparable
("the tray icon is up / the app can take commands"), not identical internal milestones. For
Stinkshot the tray is up before the app can paint, which is why it also has a cold-launch row.

**Cold launch to first overlay** (Stinkshot only). Kill, wait 3 s, launch `stinkshot --tray`,
send `stinkshot region` either 30 ms after `CreateProcess` returned (37-40 ms after the launch
timestamp, which is taken before `CreateProcess`) or 1 ms after the tray window exists, and
measure launch to overlay ready. The other tools were not measured this way.

**Idle.** All four tools running at once, footprint after 60 s idle, CPU averaged over the next
60 s, 3 repetitions; reported as the range over the 3. CPU time comes from `GetProcessTimes`, so
values under 0.1% of a core are at the limit of its resolution. Stinkshot releases its capture
shadow textures 30 s after the last capture, so its 60 s figure is after that release.

**Memory/CPU around the overlay.** A sampler thread sums working set and private bytes over the
tool's processes every 5 ms, from trigger until 1 s after close; the table reports the peak. CPU
is the tool's process CPU time over the same window.

**Full-screen to PNG.** The command is issued, then the harness polls the output folder every
millisecond for a .png whose last 12 bytes are the IEND chunk, unchanged for 300 ms afterwards.
Triggers: Stinkshot `stinkshot full --out <file> --monitor 1` (headless, new process each time)
and `stinkshot full` to the running instance (saves to history, monitor under the cursor);
ShareX `ShareX.exe -PrintScreen`; Greenshot `WM_HOTKEY` for its fullscreen hotkey; Flameshot
`flameshot full -p <dir>`.

**Swatch check.** A harness window of six sRGB patches sits on the HDR monitor and is open
before the tools start; the first three PNGs per variant are checked at each patch center.

**Frozen.** A small harness window animates one color patch every 15 ms; while the overlay is
up the harness reads that pixel from the screen DC five times, 40 ms apart. ShareX and Greenshot
were frozen in 30 of 30 runs each. Stinkshot cannot be checked (below); Flameshot never reached
its overlay.

## Caveats

- **Triggering differs between tools.** Stinkshot, ShareX and Flameshot were triggered through
  their command line, so every overlay and full-screen sample includes starting a short-lived
  process that forwards the command to the running instance (Stinkshot's headless full-screen
  starts the whole app). None of these is a real key press: no tool was measured from PrtSc or
  a hotkey, which is how most people trigger them. The slow runs of every CLI-triggered tool
  (Stinkshot 4, ShareX 3, Flameshot 6 of 30) are consistent with process start-up on this
  machine: Stinkshot's own trace put 98-181 ms inside the instance on every run, and the extra
  time of slow runs before the forwarding client reached `main`; unrelated exes (`whoami.exe`)
  show the same tail (docs/notes/app-shell.md). A hotkey or tray trigger does not start a
  process; that was not measured.
- **Greenshot's numbers are a lower bound against the others.** It has no capture command line
  (its CLI only accepts `/help`, `/exit`, `/reload`, `/language`), so the harness posts
  `WM_HOTKEY` straight to its main form. That skips process creation, which every other tool's
  sample pays (tens of ms normally, 0.2-0.6 s in slow runs), and it is also why Greenshot has no
  slow runs.
- **Flameshot 15 never reached its region overlay.** On this mixed-DPI layout `flameshot gui`
  first shows a monitor picker ("Click or press 1 to select"), which needs input. Its "overlay
  ready" is command to picker shown at its final rect (1135x441), not to an overlay; the time to
  a real overlay would be longer. Real users also go through the picker. Its overlay CPU could
  not be captured (the `gui` process is short-lived) and its memory peak may be understated.
- **Full-screen captures differ in scope.** Stinkshot captures one monitor; the others capture
  the whole 6400x2160 desktop. The headless figure in the table is the 4K monitor (8.3 MP, 60%
  of the others' 13.8 MP) and includes starting the process every run. The instance figure
  (52 / 415 ms) is for the monitor under the cursor, which was the 1440p monitor in this run
  (3.7 MP), so it is shown only in the note under the table, not compared. Neither is
  normalized per pixel.
- **PNG sizes differ, and so does what was captured.** Stinkshot's 4K PNG was 8.58 MB (about
  1.03 MB per megapixel); ShareX 9.49 MB, Greenshot 10.65 MB and Flameshot 7.50 MB for the full
  desktop (0.69, 0.77 and 0.54 MB per megapixel). Stinkshot's encoder favors speed over
  compression, so part of its full-screen lead is bought with bigger files. The per-megapixel
  figures are rough: the desktop images include the 4K monitor's content plus the 1440p monitor
  and a 2560x720 black strip that compresses to almost nothing.
- **Stinkshot's capture path depends on the display state.** With the displays on (this run) it
  freezes from its warm Desktop Duplication session; with them off it falls back to GDI (the
  second run's 220 ms median). ShareX, Greenshot and Flameshot capture with GDI either way.
- **Stinkshot's overlay contents cannot be checked from outside.** Its overlay sets
  `WDA_EXCLUDEFROMCAPTURE`, so screen-DC reads see the desktop behind it. "Frozen" for Stinkshot
  comes from its config (`freeze_screen=true`). For the same reason the harness cannot see when
  the overlay first paints; "ready" means shown at its final rect for every tool. Stinkshot
  keeps the window cloaked until two frames have been presented (docs/notes/app-overlay.md),
  so for it shown means painted; the other tools were not checked for a blank first frame.
- **Stinkshot still uses more memory than Greenshot and Flameshot.** Idle private bytes 134 MB
  against ShareX 113, Greenshot 45 and Flameshot 8; its overlay peak (414 MB private) is above
  Greenshot's and Flameshot's but below ShareX's.
- **Clipboard and toasts were off for every tool**, so the user's clipboard was left alone.
  Real capture-to-done times with clipboard copy on will be a little longer.
- **The swatch test covers SDR content on the HDR monitor.** Every checked variant returned exact
  sRGB values. Real HDR content was not tested.
- **The test PC was in use**, with a 11-21% median machine-wide background load (34% max).
  Medians are robust to this; maxima and p90s are not, which is why start-up p90s are
  dominated by a few slow runs for every tool.
- **Ghost tray icons.** Killing a process leaves its tray icon behind; the harness removes them
  with `NIM_DELETE` (uID 0-31 per window), best effort.

## Findings

1. **Overlay ready: Stinkshot 134 ms median with the displays on**, 2.5x below Greenshot (with
   Greenshot's trigger advantage) and 4.7x below ShareX; 26 of 30 runs beat every run of the
   other tools. 4 of 30 runs took 0.29-0.49 s.
2. **Launch to tray stays at about 34 ms** (11 of 15 runs 31-42 ms), below Flameshot (117),
   Greenshot (297) and ShareX (861). A cold launch with a region command right away reaches a
   ready overlay in 568-593 ms (median), which is the time that matters for a first capture.
3. **Full-screen to PNG: 156 ms for one 4K monitor headless**, against 785-1546 ms for the
   others' whole desktop, with bigger PNGs per pixel.
4. **Idle footprint: 120 MB working set / 134 MB private**, down from 399 MB private for the
   b5409a9 build with the displays on. ShareX (164 / 113), Greenshot (68 / 45) and Flameshot
   (38 / 8) are still smaller on private bytes. Idle CPU was at or below 0.13% of a core for
   all four.

## Follow-up: perf track (Nov 8 2026)

Changes after this run, measured against the benchmark's own Stinkshot build (f334f45) on
the same machine. **The displays were off for the latency and memory numbers in this
section** (console display state 0 throughout, 00:30-01:55), so Stinkshot captured with GDI
instead of its warm Desktop Duplication session; the tables above (displays on) were not
re-run. Method and raw numbers: docs/notes/app-shell.md, "Perf track".

**PNG size on identical pixels.** Each tool's sample capture from this run
(`C:\dev\stinkshot-bench\results\sample-captures`) decoded and re-encoded by Stinkshot
(`crates/core/examples/png_opt_bench`). "Saved" is what the capture path writes at once;
"after background pass" is the file once the low-priority re-compression has replaced it
(seconds later; miniz level 10, zopfli for images up to 1 MP). MB = 10^6 bytes.

| Pixels | Other tool's file | Stinkshot before | Saved now | After background pass |
|---|---|---|---|---|
| ShareX's 6400x2160 sample | ShareX 9.48 MB (0.686 MB/MP) | 9.68 MB (0.700) | 8.54 MB (0.617) | **7.43 MB (0.538)** |
| Flameshot's 6400x2160 sample (same pixels as ShareX's) | Flameshot 7.50 MB (0.542) | 9.68 MB (0.700) | 8.54 MB (0.617) | **7.43 MB (0.538)** |
| Greenshot's 6400x2160 sample (has transparent pixels) | Greenshot 10.65 MB (0.770) | 9.68 MB (0.700) | 9.68 MB (0.700) | **8.19 MB (0.592)** |
| Stinkshot's 3840x2160 sample | - | 8.58 MB (1.034) | 7.54 MB (0.909) | **6.73 MB (0.812)** |

On the same pixels the re-compressed file is 22-23 % smaller than ShareX's and Greenshot's
and 1 % smaller than Flameshot's; the file written at once is 10-12 % smaller than before
(opaque images no longer carry an alpha channel) and is written as fast (59-78 ms here). The
background pass took 6.3 s (4K) and 7.6-9.1 s (6400x2160) on one core with the PC in use
(the app runs it at background priority).

**Cold launch to first overlay** (`cold`, region about 40 ms after launch, 8 runs, median,
displays off): f334f45 548-563 ms; now the frozen screen is on display after **204 ms** (a
GDI window that takes the first drag and Esc, see the notes) and the full region overlay
replaces it at 592 ms.

**Idle** (60 s, 2 runs, displays off): private bytes 135.5 -> 124.7 MB (fonts memory-mapped);
working set 124 -> 8 MB once the app has been idle for 60 s (handed back to Windows; private
bytes unchanged, no measurable cost on the next overlay). The 60 MB target was not reached:
the GPU driver alone keeps 68 MB committed even after both of Stinkshot's devices are
released, so only a design that keeps the GPU out of the idle process could get there
(frontier in the notes). Warm overlay ready unchanged (218 -> 228 ms median with GDI, within
the noise).
