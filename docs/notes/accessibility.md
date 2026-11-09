# Accessibility

Screen readers (Narrator, NVDA), keyboard-only use, contrast themes and text size.

## Screen readers: AccessKit on every window

- eframe's `accesskit` feature is on, but eframe only adapts its root viewport, and our root
  is the invisible 1x1 `StinkShot.Root`. Every real window (Settings, History, Trim, HUDs,
  toasts, pins, overlay) is an immediate viewport, so `a11y.rs` does the rest:
  - `a11y::attach` runs once per surface when the shell finds its HWND (`shell::drive`). It
    subclasses the window (`SetWindowSubclass`) and answers `WM_GETOBJECT` with an
    `accesskit_windows::Adapter`; the `Attached` handle in `HwndState` removes the subclass
    when the surface closes (and the proc removes itself on `WM_NCDESTROY`).
  - The first UI Automation query calls `Context::enable_accesskit`. Until a screen reader
    (or any UIA client) asks, egui builds no tree: zero per-frame cost.
  - The `Bridge` egui plugin keeps a stack of the viewports whose passes are running (immediate
    passes nest inside the root's), hands each pass's `TreeUpdate` to that window's adapter
    and injects queued screen reader actions (click, focus, set value) into the matching
    viewport's input.
- Hand-painted controls need a name: `a11y::describe` / `describe_with` (role, name, on/off
  state) or `a11y::role` (already named by a tooltip). Both also draw the keyboard focus ring.
  The theme widgets (`button`, `accent_button`, `icon_button` named by its tooltip, `toggle`
  as a Switch) report themselves; `setting_row` names unnamed controls after its title via
  `Ui::label_inputs_by`.
- Covered so far: Settings nav (Tab items), accent swatches (radio), History nav and app
  filters (Tab items), History cards (list items named "source, W by H, time"), search box
  (named from its placeholder), toast actions, `ui_common` buttons and sidebar links, Trim
  handles (sliders).

## Overlay

- A polite live region (`a11y::live_status`, Role::Status) says the mode, the tool and the
  region size ("Select. 1220 by 1110"), plus the key hints while no region exists. Narrator
  reads it on every change.
- Keyboard-only selection: with no region, an arrow key starts one from the hover highlight
  (else the middle half of the pointer's monitor, `selection::keyboard_start`). Arrows move
  it, Shift+Arrows resize it, Ctrl makes steps 10 px, Enter confirms, Esc cancels. Listed in
  the Help page via `keys::sheet`. Behavior change: Shift+Arrows used to move 10 px and
  Ctrl+Arrows resized; shapes keep Shift = 10 px.

## Contrast themes and text size

- `a11y::prefs` reads `SPI_GETHIGHCONTRAST` + `GetSysColor` and `HKCU\Software\Microsoft\
  Accessibility\TextScaleFactor` (cached 1 s; the shell checks every frame and reinstalls the
  theme on change).
- Contrast theme: `theme::install` swaps every palette token for the system colors
  (`theme::high_contrast_palette`; `palette()` returns it while active), uses the system
  highlight as the accent, text-colored solid borders, no shadows.
- Text size: all `TextStyle` sizes and the theme widgets' text scale by the factor. Not yet
  scaled: hand-set sizes in other files (`RichText::size`, `FontId::proportional(N)`: overlay
  chrome, Settings/History detail text). Fixed-size windows may clip at 200%+.

## Verification

- `C:\dev\stinkshot-bench\uia\uia-dump.ps1 -ProcessId N [-Title T] [-Invoke Name]`
  (Windows PowerShell 5.1) prints a window's UIA tree; `-Invoke` / `-Focus` act through UIA
  only.
- Run a test instance with `STINKSHOT_PROFILE=ux-<label>` (hotkeys "", no PrintScreen
  takeover, no clipboard).
- Manual: Narrator (Ctrl+Win+Enter) on Settings: Tab moves through nav and rows, each toggle
  reads its row title and on or off state. Overlay: arrows start a region, Narrator
  speaks the size.
- Cost (debug build, Nov 8): idle 156.9 MB private with AccessKit inactive; History open
  158.0 MB inactive, 159.0 MB once a UIA client activated it. Overlay open timing showed no
  difference beyond debug-build noise; not measured on a release build.
- Not verified here: NVDA, real contrast themes and text-size changes on screen (code path
  unit-tested only: `theme::hc_tests`).

## Review of the merged ux branch (Nov 8 2026)

UIA dump on a debug build (profile `ux-review`), tour and pages driven through UIA only.
- Fixed: Settings nav tabs were named by their l10n keys ("settings.nav.general") after the
  integration; they now use the looked-up text.
- Fixed: tour hotkey names were painter text, missing from the UIA tree; now labels.
- Fixed: History's date filter combo had no name; now "Date Filter" (`a11y::name`).
- Verified: tour opens once on first start, Done/Skip set `tour_seen`, Settings > About >
  Show Tour reopens it; Help lists match `keys::sheet` and the tray behavior.
- Not verified: the real full-screen overlay (no synthetic input, no display wake), NVDA,
  Narrator speech.
