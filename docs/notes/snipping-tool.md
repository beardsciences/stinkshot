# Replace Snipping Tool (Nov 7 2026)

User request: an option that replaces the Windows Snipping Tool, including its shortcuts.

## What it does

Settings > Hotkeys > Windows Integration > **Replace Snipping Tool**
(`[hotkeys] replace_snipping_tool`, default off). When on:

1. **PrtSc**: the existing takeover (`system::take_over_printscreen`, sets
   `HKCU\Control Panel\Keyboard\PrintScreenKeyForSnippingEnabled = 0` and remembers the old
   value) is forced on. The Take Over PrintScreen row shows on and locked. As before, the
   takeover only happens when some Stinkshot shortcut uses PrintScreen; otherwise Settings
   says that PrtSc still opens the Snipping Tool (taking over with nothing bound would only
   hand the key to the legacy clipboard copy).
2. **Win+Shift+S** (Snipping Tool snip) runs Region, **Win+Shift+R** (Snipping Tool
   recording) runs Record MP4, through a `WH_KEYBOARD_LL` hook
   (`stinkshot_platform::snip::SnipHook`). If a Stinkshot action is bound to one of those
   exact chords, the chord runs that action instead (`hotkeys::snip_routes`), and the binding
   is not registered with Windows while the hook is on, so it cannot fire twice.
3. Settings shows a status line under the toggle: Active (with the routed actions), Off,
   Turning On / Turning Off for the one frame before the shell applies the change, or the
   reason the hook could not be installed (danger color). The shortcut list ends with two
   fixed rows, Snipping Tool Snip (Win+Shift+S) and Snipping Tool Recording (Win+Shift+R),
   whose descriptions say where the chord goes.

Turning it off removes the hook at once and restores the PrintScreen value Stinkshot
remembered (or keeps the takeover if Take Over PrintScreen is still on by itself). Quitting
does the same (`Hotkeys::shutdown`). Nothing else on the system is changed; a crash removes
the hook automatically (Windows drops a process's hooks when it exits).

## The hook (`crates/platform/src/snip.rs`)

- `ChordFilter` is a pure state machine over `KeyEvent { vk, down }`, unit tested (17 tests):
  - Only the chord letter is ever swallowed: its first down, its auto-repeats, and its
    matching up, in any release order (Win or Shift released first still swallows the S up;
    an S up whose down it did not swallow passes). Win/Shift/Ctrl/Alt always pass, so no key
    can be left logically down.
  - Win+Shift plus Ctrl or Alt, Win+S, Shift+S, Win+Shift+other letters all pass.
  - Left and right modifiers are tracked separately; a generic `VK_SHIFT` (injected input)
    counts.
  - `sync(Mods)`: before deciding on an S/R down the hook reads `GetAsyncKeyState` and
    corrects the tracked state. This covers missed key-ups (Win+L locks the session and the
    hook never sees Win come up on the secure desktop; a later Shift+S must type a capital S)
    and modifiers held before the hook was installed.
- Start menu: swallowing the letter means Explorer sees Win go down and up with nothing in
  between and would open Start. The hook injects an unassigned key (VK 0xE8, the AutoHotkey
  "menu mask" trick) from inside the callback, while Win is still down.
- Injected events tagged `SNIP_EXTRA_INFO` (our mask key) are ignored; other injected input
  (AutoHotkey, Stream Deck, the desktop test) is treated like typing.
- Callback work: the state machine, the mask `SendInput` when a chord fires, and a
  `PostThreadMessageW` to its own thread. The message loop sends the chord to a channel; an
  app-side bridge thread forwards it to `Hotkeys` and wakes the UI. Nothing in the callback
  blocks.
- Hook timeouts: Windows 7+ silently removes a low-level hook whose callback exceeds
  `LowLevelHooksTimeout`. The hook thread re-installs the hook every 30 s (new one first, then
  the old one is removed; callbacks only run while that thread waits for messages, so no
  event is processed twice).
- While Settings is recording a shortcut (`Hotkeys::set_suspended`), routed chords are
  dropped, like hotkeys. The hook keeps swallowing them, so the Snipping Tool does not open
  either.

## ms-screenclip: and other Snipping Tool entry points (investigated, not implemented)

Other ways to start a snip: the `ms-screenclip:` URI (pen button, Quick Settings / Action
Center "Snip" tile, apps that launch it), the Snipping Tool app itself, and the Game Bar.

Read-only check on this PC (Nov 7 2026, Windows 11 26200):
- `HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\ms-screenclip\UserChoice`
  holds `ProgId = AppXfeq5vwnakrw6cy02kzhq8ekhhsremh62` (the Snipping Tool package) with a
  `Hash`, plus the newer `UserChoiceLatest` key with its own hash.
- The `UCPD` (User Choice Protection Driver) service is running.

A per-user override would need a new `UserChoice` for `ms-screenclip`. Windows only honors a
`UserChoice` whose hash matches, the hash algorithm is undocumented (and changed with
`UserChoiceLatest`), UCPD exists to block programmatic association changes (whether its
list covers `ms-screenclip` was not checked), and the only supported way to change a protocol
default is the user picking it in Settings > Apps > Default apps. A
`HKCU\Software\Classes\ms-screenclip\shell\open\command` key is not expected to win over a
valid `UserChoice` (not tested: writing it would be a persistent change on the user's PC).
None of this is supported or cleanly reversible, so Stinkshot does not touch it. Documented limitation: the Snip tile, the pen button and apps
that launch `ms-screenclip:` still open the Snipping Tool. Uninstalling or disabling the
Snipping Tool app is the user's call and is not something Stinkshot does.

## Verification (Nov 7 2026)

- `cargo test -p stinkshot-platform --lib snip`: 17 state-machine tests.
- App unit tests: `hotkeys::snip_routes_default_and_follow_bindings`,
  `hotkeys::snip_chord_strings_parse_and_match_recorded_form`,
  `settings_ui::snip_status_lines`, `settings_ui::snip_chord_rows`.
- Desktop, `cargo test -p stinkshot-platform --test desktop -- --ignored --exact
  snip_hook_swallows_and_routes_win_shift_s --nocapture`: waits until keyboard and mouse have
  been idle for 2 s with nothing held, opens its own topmost window under the cursor, checks
  it is foreground and under the cursor, installs `SnipHook`, injects LWin, LShift, S downs,
  waits for the route, then injects the ups. Result: routed `[Region]` once, 3.2 ms after the
  injection; the window saw 0 S downs and 0 S ups and the Win up; no key left down; the test
  window stayed foreground (no Start menu, no Snipping Tool; no Snipping Tool process
  afterwards).
- Dev profile app (`STINKSHOT_PROFILE=snip`, all hotkeys empty, replace on): log shows
  "Snipping Tool shortcuts routed to Stinkshot"; `PrintScreenKeyForSnippingEnabled` was not
  touched (no PrintScreen binding); Settings > Hotkeys rendered the toggle, the Active line,
  the PrtSc note, the locked Take Over PrintScreen row and the two chord rows (window captured
  with `PrintWindow`, no input sent).

Not verified: pressing the real Win+Shift+S / Win+Shift+R on a running app (the end-to-end
route into the region overlay and recorder; the same `Command`s as the tray), toggling the
option off and on in the UI, and a hook refresh after an actual timeout.

## Manual checks

- [ ] Turn on Replace Snipping Tool: Win+Shift+S opens the Stinkshot overlay, Win+Shift+R
      starts Record MP4 (and stops it while recording); the Start menu does not open; typing
      a capital S and R, Win+S, Win+Shift+Left still work.
- [ ] Bind Win+Shift+S to Copy Text (OCR): the chord runs OCR once, the row shows no error.
- [ ] Turn it off: Win+Shift+S opens the Snipping Tool again; PrtSc follows Take Over
      PrintScreen.
- [ ] Lock with Win+L, unlock, type Shift+S in a text box: a capital S appears.
