# Smart Redact (Find Sensitive Info)

Finds sensitive text in a capture and redacts it in one step. Everything runs locally:
Windows OCR plus hand-written detectors, no network, no model.

## Pieces

- `stinkshot_core::sensitive` (pure, unit-tested): `find_in_lines(&[Vec<TextWord>])` joins each
  OCR line's words with single spaces, runs the detectors and maps every match back to the
  words it overlaps. A finding's rect is the union of those word boxes, so a box always covers
  whole words. `find_in_text` is the line-level scanner.
- `stinkshot_platform::ocr`: `OcrLine::words` now carries each word's text and box (input
  image px). Lines without words fall back to the line rect.
- `app/src/overlay/smart_redact.rs`: scan worker, review panel, canvas highlights, applying.

## Detectors (priority order; a span claimed first wins)

| Kind | Rule |
| --- | --- |
| Key or Token | Known prefixes (`sk-`, `sk_live_`, `ghp_`, `github_pat_`, `glpat-`, `xoxb-`, `AKIA`, `AIza`, `hf_`, `npm_`, ...) with 12+ more key chars; JWTs (`eyJ` + three dot parts) |
| Email Address | `local@domain.tld`, letters-only TLD of 2+ |
| Social Security Number | `ddd-dd-dddd`, area not 000/666/9xx, group not 00, serial not 0000 |
| Card Number | 13-19 digits, plain or in groups of up to 6 split by spaces/dashes, Luhn valid |
| Phone Number | 10-15 digits with separators or a leading `+`; groups of at most 5 digits, at most one 4-digit group (so card fragments are not phones), last group 3+; dotted only as 3.3.4 |
| IP Address | IPv4 with octets <= 255, optional `:port` |
| Key or Token | Generic: 32-256 chars of `[A-Za-z0-9-_+/=]`, 2+ digits, 8+ letters, Shannon entropy >= 3.5 bits/char |
| Password | The value after `password:`, `pwd=`, `pin:`, `secret:`, `token:`, `api_key=` and similar labels (already masked values skipped) |

Digit runs that span several numbers ("555-123-4567 10.0.0.1") are tried whole, then as shorter
windows of their space-separated chunks, longest first. Digits glued to letters ("ID123456")
are never numbers. Dates (2026-11-07), times and versions do not match.

Previews in the panel are masked: `j***@example.com`, `**** 4242`, `***-**-6789`,
`192.168.*.*`, `ghp_******`, `********` (bullets in the UI).

## Editor UI

- Shield button, first in the action bar, tooltip "Find Sensitive Info (Ctrl+R)"; also Ctrl+R
  (`keys::FIND_SENSITIVE`, on the cheat sheet under Edit). Selected look while the panel is
  open; pressing it again (or Esc, or Cancel, or the panel's close button) closes the panel.
- No OCR language: the button is disabled; its tooltip is "Find Sensitive Info Needs an OCR
  Language in Windows Settings" plus the shared `workers::NO_OCR_HELP` sentence. Ctrl+R shows
  the same title in the hint bar.
- Scan: OCR + detection on the unannotated selection (`ensure_base`) on a `smart-redact`
  thread; the panel shows a spinner and "Finding Sensitive Info...". OCR errors show "Could Not
  Read the Text in This Image" in the hint bar.
- Nothing found: the panel shows "No Sensitive Info Found" for 2.5 s, then closes.
- Review panel (under the action bar, right edges aligned, above it when there is no room,
  kept on the bar's monitor): "Found N Items", one row per finding (checkbox, kind, masked
  preview), all on by default; click a row to toggle. Hovering a row fills that spot on the
  canvas; every finding is outlined while the panel is open (accent when on, white when off).
  Style choice Blur / Pixelate / Solid Box, remembered in `[editor] smart_redact_style`
  ("box" default, "blur", "pixelate"; unknown values read as box). "Redact N Selected" adds
  one shape per checked finding and records a single undo step; Cancel closes.
- Shapes: Solid Box = `ShapeKind::Redact` in black (not the annotation color), Blur and
  Pixelate use the editor's blur radius and pixelate block, i.e. the same destructive
  minimums as the tools. Word boxes are padded by 15% of their height (at least 2 px) and
  clipped to the image. The shapes are ordinary annotations: movable, resizable, deletable.
- Findings belong to the scanned image: moving or resizing the selection in capture mode
  drops the panel (the base is re-cropped), so stale boxes never land in the wrong place.

## Verified

- `cargo test -p stinkshot-core sensitive` (detectors, masking, word mapping),
  overlay `smart_redact::tests` (style config round trip, padding, shape per style),
  `keys` tests (Ctrl+R label, on the sheet, not on the region-only sheet).
- Desktop: `cargo test -p stinkshot-platform --test desktop -- --ignored
  smart_redact_finds_rendered_email_and_phone` renders "Mail jane.doe@example.com or call
  555-123-4567" offscreen with GDI, OCRs it and finds the email and phone with word boxes
  (the email box narrower than half the line).

## Not verified

- The panel was not driven interactively (no synthetic input into a live overlay); layout,
  hover highlight and Redact Selected were checked by building and by unit tests only.
  Manual steps: capture a region with an email and a card number, press Ctrl+R, hover rows,
  untick one, pick Pixelate, Redact Selected, Ctrl+Z (all shapes go in one step), reopen the
  editor and confirm Pixelate is preselected.
- Non-English OCR languages, handwriting, very small text (OCR quality bounds recall).
- International formats beyond the rules above (IBANs, national IDs other than US SSN).
