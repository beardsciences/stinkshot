# Localization: Settings group

Converted Nov 8 2026. Every user-visible literal in `crates/app/src/settings_ui.rs` goes
through `tr!` / `l10n::lookup`; strings live in `crates/app/src/l10n/en/settings.rs`. The
checker reports 0 for settings_ui.rs.

## Patterns used

- Section names: `SECTIONS` holds keys (`settings.nav.*`), looked up when drawn. The key is
  also the nav row's id salt, so ids stay stable across languages.
- Theme choices: `THEMES` is a `(config value, key)` table like `l10n::CHOICES`.
- Frame-rate options: one `settings.fps` template (`{n} FPS`) via `fps_labels` + `as_refs`.
- Placeholders: `{n}`, `{name}`, `{path}`, `{count}`, `{size}`, `{list}`, `{ago}`,
  `{preview}`, `{help}`. The file-name token list (`{yyyy}`, `{title}`, ...) is literal text
  in `settings.capture.pattern.desc`; `fill` only replaces the named arguments.
- `errors::report(context, ...)`: the context is log-only and stays English; the hint is
  translated.

## Left in English on purpose

- Window title "Stinkshot Settings": `shell.rs` hashes the title into the viewport id, so a
  translated title would recreate the window when the language changes. Translating it
  needs the shell to use a fixed id first.
- "Ctrl+H", "GPL-3.0-or-later", the `Stinkshot_{yyyy}-{MM}-{dd}` pattern hint, `#rrggbb`
  hints and slider unit suffixes (` s`, ` px`, ` ms`, ` kbps`).

## Strings shown in Settings that belong to other groups

Hotkey row labels and descriptions (`actions.rs`), "Not Set" and key names
(`ui_hotkey.rs`), hotkey error text (`hotkeys.rs` `ErrorKind::row_text`, `SnipStatus`
reasons), `workers::NO_OCR_HELP`, the toggle's On/Off (`theme.rs`), the About header
(`logo_anim.rs`) and the sidebar link (`toast/ui_common.rs`) are shell strings.

## Layout under pseudo

`widgets::setting_row` (only Settings uses it) now wraps the title and description beside
the controls using the controls' width from the last frame, and moves the controls to their
own line when the text would get narrower than 200 pt. Before, long text ran under combos,
sliders and the On/Off label. Screenshots at the 760 pt minimum window width:
`C:\dev\stinkshot-bench\ux-shots\pseudo-settings`.

Open: the sidebar "Open History" link overlaps its "Ctrl+H" hint in pseudo
(`uc::sidebar_link`, shared with History; not changed here).
