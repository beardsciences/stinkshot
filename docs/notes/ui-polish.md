# UI polish pass

Visual-only pass over the app surfaces (no behaviour or logic changes). Checked in the
`STINKSHOT_PROFILE=ui` dev profile on the 4K monitor at 125 % and the 1440p monitor at 100 %,
in both the dark and the light theme. Screenshots are in `ui-polish/` (capture thumbnails
pixelated).

## Changes, most impact first

1. **Light theme hairlines were invisible.** `LIGHT.stroke` was a premultiplied *white*
   (`0x0f,0x0f,0x0f,0x0f`), so every card, search box, layer edge and button border vanished
   on white. Now a ~9 % black hairline, as in Windows 11. (`theme.rs`)
   See `settings-light-*.png`, `history-light-*.png`.
2. **Control borders.** New `theme::control_stroke(dark)` (white 22 / black 30 alpha) used by
   neutral buttons, `rect_button`, and egui's inactive/hovered/active widget visuals, so combo
   boxes, text fields and value boxes read as controls in both themes.
3. **Toggle switch in light theme.** The off state used fixed `#a0a0a0` / `#d0d0d0` greys that
   disappeared on white cards. Off outline and thumb now use the secondary text colour (what
   Windows 11 does), the thumb grows 1 pt on hover with a fade, and a faint hover wash shows
   behind the off track.
4. **Real Semibold face and a type hierarchy.** `semibold()` was backed by Segoe UI *Bold*
   (core loads `segoeuib.ttf`). `theme::install` now loads `seguisb.ttf` (weight 600, as in the
   mockups) and falls back to the bold face. Then applied it where the design calls for 600:
   settings page title (28), group titles (14), info card title (15), About name, toast title,
   history empty-state title (now 20), delete dialog title, countdown digit, Trim title, the
   overlay hint-bar keys (`.strong()` only changed colour, not weight), and filled
   accent/danger `rect_button`s.
5. **Accent used as text met 4.5:1 only on dark.** New `theme::accent_fg(accent, dark)`
   darkens (light theme) or lightens (dark theme) the accent just enough to reach 4.5:1. Used
   for links, selected icon-button glyphs, the active aspect preset, the purpose chip, the hint
   bar's leading label and the selected width dot. Fills still use the raw accent.
6. **Icon buttons: hover fade, pressed state, cursor bug.** Hover background now fades over
   100 ms, pressed uses a stronger wash and shrinks the glyph 1 pt, disabled glyphs are dimmer.
   Fixed `icon_button` setting the pointing-hand cursor every frame whether hovered or not
   (whole windows showed a hand cursor while any icon button was on screen).
7. **One focus ring.** Buttons, icon buttons and toggles share a Windows-style 2 px ring in
   the ink colour, 1 px outside the control's own corners (was accent, sometimes inside,
   sometimes outside, and low-contrast with light accents).
8. **Hover fades on all buttons.** `widgets::button`/`accent_button` and `uc::rect_button` fade
   their hover fill in and out (100 ms). Neutral `rect_button` hover now darkens on the light
   theme (lightening `#fbfbfb` did nothing).
9. **Danger button text.** Stop / Delete used dark ink on the light theme's deep red (about
   2.8:1). `theme::on_danger(dark)` gives white on light, dark ink on dark.
10. **ComboBox chevron.** All four combo boxes draw a Fluent chevron (`Icon::ChevronDown`,
    `widgets::combo_chevron`) instead of egui's filled triangle.
11. **Sliders.** Round thumb (`HandleShape::Circle`), 4 pt rail, and in settings rows a 24 pt
    row so the thumb is Fluent-sized and the value box shares its centre line.
12. **History header alignment.** The date filter combo was 32 pt tall next to the 36 pt search
    box and Open Folder button; now 36 pt so all three line up.
13. **History empty state.** Icon sits on a soft disc, title is 20 pt semibold, and the body is
    centred line by line (it was a left-aligned block under a centred title). See
    `history-empty-*.png`.
14. **History cards.** Loading placeholder bars and the hover border use the ink colour so they
    show on the light theme (they were white). The star always sits on a dark disc, so the
    unstarred outline reads over placeholders and bright thumbnails.
15. **Crisp layer edges.** History and Settings draw the content layer's top/left hairlines
    snapped to pixel centres at one physical pixel (`uc::layer_edges`), plus the rounded corner
    arc that was missing. At 125 % / 150 % the old 1 pt lines at +0.5 pt smeared over two rows.
16. **Overlay chrome consistency.** Size chip and loupe plate use the UI font (Segoe digits are
    tabular) instead of egui's monospace fallback, which looked foreign. The stroke-width
    button and colour swatch now hover/press like the icon buttons (the width button had no
    hover at all); swatch ring follows the design (18 px swatch, gap, 1.5 px ring). Palette and
    width popups use the same wash and the theme hairline instead of fixed white alpha.
17. **Loupe corners.** The square corner cells poked out past the 8 pt rounded bezel; the four
    corner cells now take the bezel's rounding.
18. **Toast entrance.** The toast rises 10 px (the stack gap, so it never overlaps the toast
    below) into its slot over 180 ms with ease-out and fades its content in. Tested by
    `entrance_eases_out_and_settles`.
19. **Toast dismiss hover** used the control fill, invisible on the light flyout; now the
    shared subtle wash.
20. **Trim window** "Loading Preview" sits on an always-dark well; it now uses the dark theme's
    grey so it keeps 4.5:1 in the light theme.

New tests in `theme.rs` pin the contrast rules: secondary and tertiary text reach 4.5:1 on
every surface of both palettes, `accent_fg` reaches 4.5:1 for all four design accents, and
`on_danger` reaches 4.5:1.

## Seen but left alone

- "Press Not Set to capture a region" in the empty History state when no region hotkey is
  bound, the "C to copy" casing in the loupe plate, and the toast body's "3840 x 2160" using
  `x` instead of `×` (built in the shell). Copy and behaviour belong to the UX pass.
- egui's slider fill uses the translucent text-selection colour, so the filled part of a
  slider is a faint accent rather than solid. Making it solid would also make text selection
  opaque; left as is.
- Mica: windows paint an opaque `bg`, so the DWM Mica backdrop never shows through. Making the
  root fill transparent is a bigger change (eframe transparency, light/dark tint) and was out
  of scope.
- Settings and History nav rows place the accent bar and label differently (Settings indents
  the label 26 pt, History 12 pt). Both are reasonable; unifying would move every row.
- Icon joins are mitred polylines; at 18 pt they read fine at 100-200 %, so no round-join
  tessellation was added.
- The pin toolbar and pin hint use fixed dark chrome over the image in both themes, which is
  deliberate (it sits on arbitrary image content).
- Only 100 % and 125 % were checked on real monitors; 150 % / 200 % were reasoned about
  (pixel-snapped hairlines, point-based sizes) but not captured.
- Reading `seguisb.ttf` happens once in `theme::install` on the UI thread (about 1 MB, a few
  ms); fine at startup, noted in case install ever runs per frame.
