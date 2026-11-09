# First-run tour and Help

## Tour (`crates/app/src/tour.rs`)

- Own small window "Welcome to Stinkshot" (560x400, not resizable), four steps: Capture With a
  Hotkey (region key plus Window / Full Screen / Record MP4 keys from the live config), It Lives
  in the Tray, Nothing Ever Leaves Your PC, Where Captures Go (the resolved history folder).
- Skippable at every step (Skip Tour, Esc, close box). Enter / Right go forward, Left goes back.
  The last step has Open Help (Settings > Help) and Done.
- Shown once: the shell queues `Command::OpenTour` at start-up when `general.tour_seen` is false
  and the launch was a plain one (no subcommand, not `--tray`: autostart, silent upgrades and
  the smoke test stay silent; the tour waits for the next plain launch). Closing it any way sets `tour_seen = true`
  and marks the config dirty, so the shell saves it.
- Existing configs lack the field, so serde defaults it to false: users upgrading to the first
  build with the tour see it once too. Deliberate (0.1.0 is the first public release).
- Settings > About > Show Tour reopens it (`Command::OpenTour`; an open tour is focused instead).

## Help (`crates/app/src/help.rs`)

- Settings section "Help" between Privacy and About; also the tray menu's Help item
  (`Command::OpenSettingsAt(Section::Help)`).
- Global Hotkeys: generated from `Action::ALL` and the live `config.hotkeys` (unbound shows
  Not Set), with a Change Hotkeys button.
- Editor Shortcuts: `overlay::keys::sheet(false)` verbatim (the same table the overlay's F1 cheat
  sheet and key handling use). `overlay::keys` is now `pub(crate)` for this.
- Tests assert every action, every tool letter, every chord and every row of both cheat sheets
  appear, so Help cannot drift from the bindings.
- Also: tray icon clicks, a short feature list, File Locations (captures folder, config.toml,
  stinkshot.log; Open / Show in Explorer). Nothing on the page touches the network.

## CLI

`stinkshot settings --page <name>` opens Settings on a page by its nav name, any case
(`help`, `about`, ...); an unknown name opens Settings as before. `Section::from_name` maps it.

## Strings

Each new file keeps its user-visible strings in a `mod text` const table at the top, ready to
move into the localization lookup.

## Manual verification

Test instance: `STINKSHOT_PROFILE=ux-tour`, config with `take_over_printscreen = false`, all
hotkeys `""`, `copy_to_clipboard = false`, `tour_seen = false`. Screenshots in
`C:\dev\stinkshot-bench\ux-shots\tour-*.png`, `help-*.png`, `about-show-tour.png`.

Verified (Nov 8 2026, debug build): tour opens on first start, Right/Esc/Enter keys step and
close it, `tour_seen` is written true on close (Esc and Done both), `settings --page help|about`
opens those pages. Driven with posted key messages to the test window only.

Not verified: mouse clicks (posted mouse messages do not reach egui because Windows sends
WM_MOUSELEAVE while the real cursor is elsewhere), so the Skip / Back / Open Help / Show Tour
/ Change Hotkeys / Open buttons, the tray Help item and the lower half of the Help page
(editor groups, features, file locations) were not exercised or screenshotted. Light theme not
checked.
