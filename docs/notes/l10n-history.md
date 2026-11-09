# Localization: History window

Status of the `history` group (see l10n.md for the framework). Every user-visible string in
`crates/app/src/history_ui.rs` goes through `tr!`; English lives in `l10n/en/history.rs`.
The checker reports 0 for the file.

## Not translated (on purpose or for later)

- Month names and time in `friendly_date` come from chrono's `%b %-d` / `%-H:%M`, which are
  English-only. A real translation needs a locale-aware date formatter.
- Byte and size units (`GB`, and `uc::fmt_bytes` / `fmt_thousands` / `fmt_dims` in shell code)
  and the `Ctrl+,` shortcut text.
- App names and window titles are user data; unknown capture modes show as stored.
- `crate::workers::NO_OCR_HELP` (appended to the no-OCR notice) belongs to the shell group.
- `errors::report` context strings ("history query") go to the log only.
- The empty-state hint compares the region hotkey against `ui_hotkey::display("")`, so it keeps
  working once "Not Set" is translated.

## Layout for longer text

- The header's Open Folder button and date picker are sized from their text (minimum 108 and
  130 px, as before); the search box takes the rest and never goes below 80 px.
- Pseudo-locale at the 900 px minimum window: the search hint truncates with an ellipsis, all
  other labels fit. Sidebar rows, retention footer and the empty state wrap or fit at 240 px.
- Screenshots: `C:\dev\stinkshot-bench\ux-shots\pseudo-history`.
- Not checked in pseudo-locale (they need input in the window): the selection action bar, its
  tooltips, the delete dialog (360 px wide, two 170 px buttons) and the no-match empty state.
