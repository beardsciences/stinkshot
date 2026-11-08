# history

## What was built

`stinkshot-history` is complete: every `todo!()` is replaced. Code lives in
`crates/history/src/lib.rs` (public types, unchanged) and `crates/history/src/store.rs`
(`History` implementation plus 21 unit tests). `stinkshot_history::History` is re-exported
from the crate root, so the public path is unchanged.

- Schema v1 (`PRAGMA user_version`): `items` table, `path` UNIQUE NOCASE, created stored as
  unix milliseconds. FTS5 external-content table `items_fts` over
  `ocr_text, window_title, app, name` (file name) with insert/delete/update triggers.
  Tokenizer `unicode61 remove_diacritics 2`.
- WAL mode, `synchronous=NORMAL`, 5 s busy timeout. `Mutex<Connection>`; a static
  `Send + Sync` assertion test exists. A poisoned mutex is recovered, not propagated.
- Search: user text is split into alphanumeric words, each double-quoted (so FTS operators,
  column filters and punctuation are inert), AND-ed, last word gets a `*` prefix. Text with
  no word characters means no text filter.
- Filters: app (case-insensitive exact), kind, starred, since; newest first; limit 0 means
  unlimited; offset works without limit.
- `add` on an already-indexed path updates that row (and clears OCR text) and returns the
  existing id. Dimensions come from the image header (no full decode); width/height given
  in `NewItem` are trusted when both nonzero. Videos have 0x0 unless provided.
- `delete`: removes file (missing file is fine), core sidecar, thumbnail, row.
- `apply_retention`: age first (strictly older than N days), then oldest non-starred until
  total bytes <= cap. The total counts starred items too; starred are never removed.
- `rescan`: top level only, skips dotfiles and `.stinkshot`, extensions png/jpg/jpeg/webp/
  mp4/gif (case-insensitive). Existing rows keep their metadata. Rows with vanished files
  are dropped along with thumbnail and sidecar. Path comparison is case-insensitive.
- Thumbnails: `.stinkshot/thumbs/<id>.png`, regenerated when the source is newer, or when a
  larger size than cached is requested; never upscaled; atomic write. Videos only return a
  path after `store_thumbnail`.
- Corrupt index: moved to `index.sqlite.corrupt` and recreated empty (caller should
  `rescan`). An index with a newer schema version is an error and is left untouched.

## Deviations

- Thumbnails decode/resize/encode with the `image` crate directly (added dependency with the
  `gif` feature, version from the workspace), not `stinkshot_core::Rgba::load/thumbnail`.
  Reason: core is still `todo!()` in this branch, and header-only dimension reads need
  `image` anyway. `store_thumbnail` also encodes through `image`.
- `delete`/rescan call `stinkshot_core::annotate::delete_editable` inside `catch_unwind`
  so that a not-yet-implemented core (or a sidecar failure) never blocks deletion. Once core
  is real the panic guard is harmless and can be removed.
- `Stats.recordings` counts videos and GIFs. `apps()` omits items with an empty app.

## Contract changes

- Only an added `image` dependency in `crates/history/Cargo.toml`.
- `History::refresh_bytes(id)` (Nov 8 2026, additive): re-read the stored file size only,
  after the app's background PNG re-compression swapped in a smaller file with the same
  pixels (OCR text and thumbnail stay).

## Contract requests

None. Note for core: `cargo clippy -p stinkshot-core -- -D warnings` currently fails with
`derivable_impls` in config.rs (run history clippy with `--no-deps`).

## Known limitations

- No MP4 header parsing, so video width/height stay 0 unless the caller passes them.
- Subfolders are not scanned (spec: top level).
- Size-cap accounting uses stored byte counts, which can lag if files are edited externally
  until `touch`/rescan-after-delete.

## Manual verification checklist

- Open a real captures folder with existing PNGs, run `rescan`, confirm items and sizes.
- Drop a file into the folder while the app runs, run `rescan`, confirm it appears.
- Re-edit an item, confirm `touch` clears OCR and the OCR worker re-indexes it.
- Delete an annotated item and confirm `.stinkshot\edit\` sidecar is gone (needs real core).
- Search "invo" finds OCR text containing "invoice" after the worker stores text.
- Retention with real-size files, starred items survive.

## Edit sidecars hold redacted pixels only

`annotate::save_editable` writes the base with every destructive shape (Blur, Pixelate,
Redact) already applied (`annotate::bake_destructive`). The flattened file and the sidecar
base therefore both lack whatever a redaction hid; only the vector shapes stay editable.
Covered by `annotate::tests::sidecar_base_has_redactions_baked_in`. Sidecars written by older
builds still hold the unredacted base until the item is edited and saved again or deleted.
