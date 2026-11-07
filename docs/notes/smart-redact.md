# Smart Redact (Find Sensitive Info)

Finds sensitive text in a capture and redacts it in one step. Everything runs locally:
Windows OCR plus hand-written detectors, no network, no model.

## Pieces

- `stinkshot_core::sensitive` (pure, unit-tested): `find_in_lines(&[Vec<TextWord>])` cuts each
  OCR line into segments wherever the gap between neighbouring words is wider than 1.5x the
  word height (Windows OCR often merges table columns into one line), joins each segment's
  words with single spaces, runs the detectors and maps every match back to the words it
  overlaps. So a match never joins two columns. Passwords are also looked for across the
  whole line, because a form's label and value usually sit in different columns. A finding's
  rect is the union of the matched word boxes, so a box always covers whole words.
  `find_in_text` is the line-level scanner.
- `stinkshot_platform::ocr`: `OcrLine::words` now carries each word's text and box (input
  image px). Lines without words fall back to the line rect.
- `app/src/overlay/smart_redact.rs`: scan worker, review panel, canvas highlights, applying.

## Detectors (priority order; a span claimed first wins)

| Kind | Rule |
| --- | --- |
| Key or Token | Known prefixes (`sk-`, `sk_live_`, `ghp_`, `github_pat_`, `glpat-`, `xoxb-`, `AKIA`, `AIza`, `hf_`, `npm_`, ...) with 12+ more key chars (bare `sk-` needs 20+; `sk-proj-` / `sk-ant-` 12+), never a body of dash-joined words (`sk-learn-compatible`); JWTs (`eyJ` + three dot parts) |
| Email Address | `local@domain.tld`, letters-only TLD of 2+ |
| Social Security Number | `ddd-dd-dddd`, area not 000/666/9xx, group not 00, serial not 0000 |
| Card Number | 13-19 digits, plain or in groups of up to 6 split by spaces/dashes, Luhn valid |
| Phone Number | 10-15 digits. With a leading `+`: any groups of at most 5 digits (`+44 20 7946 0958`). With `(`, `)`, `-` or `.`: groups of at most 5, at most one 4-digit group (so card fragments are not phones), last group 3+; dotted only as 3.3.4. Spaces only: the whole run of numbers must be a national pattern (3-3-4, 1-3-3-4, or 0-led 3-4-4 / 5-6), so table rows such as `120 350 480 1200` are not phones |
| IP Address | IPv4 with octets <= 255, optional `:port` or `/prefix`; IPv6 (eight hex groups, or a `::` form with 3+ groups, optional `/prefix`) |
| Key or Token | Generic: 32-256 chars of `[A-Za-z0-9-_+/=]`, 2+ digits, 8+ letters, Shannon entropy >= 3.5 bits/char, and mixed case or entropy >= 4.2. Never hex-only (git SHAs, SHA-256, UUIDs) and never slug-like (half or more of its `-_/.` parts are plain words: URL paths, kebab-case names) |
| Password | The value after a label: `password`, `pwd`, `pin`, `pass`, or any label ending (separators ignored) in password, passwd, passcode, secret, token, apikey, secretkey, accesskey, privatekey and similar (`DB_PASSWORD=`, `client_secret=`, `csrf-token:`); two-word `API Key:`, `Secret Key:`, `Access Key:`. Already masked values skipped |

URLs, `mailto:` links and `key=value` pairs: each token is also split on `: / ? & = # , ;` and
quotes, and the email, IPv4, prefixed-key and generic-key detectors run on every piece, after
the whole token. The box still covers the whole OCR word (`http://192.168.12.201:8032/web`,
`mailto:jane@example.com`, `...?key=AIza...`).

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
- Enter while the review list is open is Redact Selected (the hint bar says "Enter Redact
  Selected"). Enter while scanning, or with nothing ticked, does nothing. Ctrl+C is held while
  the panel is scanning or reviewing, so neither key copies the image with the findings in
  the clear; close the panel (Esc) or redact first. With only the "No Sensitive Info Found"
  notice up, Enter and Ctrl+C copy as usual. Decided in `smart_redact::panel_key` (tested).
  The toolbar Copy / Save buttons are not blocked: a click on them is deliberate.
- Shapes: Solid Box = `ShapeKind::Redact` in black (not the annotation color). Blur and
  Pixelate start from the editor's blur radius and pixelate block but grow with the found
  text: radius at least a third of the padded box height, blocks at least half of it, so
  large text (4K, 200% scale) never stays legible and digits from a known format cannot be
  brute-forced from fine cells. The tools' destructive minimums still apply on top. Word boxes are padded by 15% of their height (at least 2 px) and
  clipped to the image. The shapes are ordinary annotations: movable, resizable, deletable.
- Findings belong to the scanned image: moving or resizing the selection in capture mode
  drops the panel (the base is re-cropped), so stale boxes never land in the wrong place.
- On disk: the edit sidecar (`.stinkshot\edit\<name>.base.png`) stores the base with every
  Blur / Pixelate / Redact shape already baked in (`annotate::bake_destructive`, called by
  `save_editable`), so a redacted secret is not readable from the history folder. The
  shapes stay editable after Edit, but moving or deleting one leaves its old spot redacted.

## Verified

- `cargo test -p stinkshot-core sensitive` (detectors, masking, word mapping, column
  segments; negatives for git SHAs, SHA-256, UUIDs, URL paths, slugs, `sk-learn-...`, table
  rows; positives for URLs, mailto, `DB_PASSWORD=`, `API Key:`, CIDR, IPv6, +44 / +61 phones),
  `annotate::tests::sidecar_base_has_redactions_baked_in`,
  overlay `smart_redact::tests` (style config round trip, padding, shape per style, strength
  scaling, Enter / Ctrl+C policy),
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
