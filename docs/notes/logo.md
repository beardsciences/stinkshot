# Logo notes

Files: `crates/app/src/icon_gen.rs` (renderer, shared with `build.rs`),
`crates/app/src/app_icon.rs` (window icons), `crates/app/src/tray.rs` (animation and
recording badge), `crates/app/examples/logo.rs` (PNG export), `docs/logo-256.png`,
`docs/notes/logo/*.png`.

Regenerate every PNG with `cargo run -p stinkshot --example logo`.

## Concepts

Each sheet shows 256, 32 and 16 px (small sizes also zoomed with nearest neighbor) on light
and dark backgrounds.

| Concept | Sheet | Idea |
|---|---|---|
| A, Puff | `logo/concept-a-puff.png` | Free-standing glossy green cloud, two motion puffs trailing left, soft contact shadow. |
| B, Tile | `logo/concept-b-tile.png` | The same puff on a dark rounded app tile with viewfinder corner brackets. |
| C, Lens | `logo/concept-c-lens.png` | The puff with a glossy camera lens set into its face. |

**Picked: A.** Reasons:

- It survives the tray. At 16 and 20 px A is the whole canvas worth of cloud with a clean
  silhouette. B spends a third of the pixels on the tile and brackets, so the cloud shrinks to
  a smudge, and a dark square sits heavy on a dark taskbar. C's lens turns into a single dark
  dot at 16 px that reads as an eye (a one-eyed creature, not a camera).
- The joke is the cloud. The brief keeps humor to the name and icon; a stink cloud says it
  without a camera metaphor, and the motion puffs read as "stink" while staying tidy.
- It animates well. A bare silhouette squashes and hops cleanly; a tile would need the cloud
  to move inside a fixed frame.

B is the strongest "app tile" look at 256 and could come back as a store or installer icon
if one is ever needed.

## How it is drawn

Pure std (no tiny-skia) so `build.rs` can `#[path]`-include the same file. A tiny analytic
ray tracer: each lobe is an ellipsoid facing the viewer with a closed-form front height; the
surface is a log-sum-exp soft maximum of those heights, and the normal and gloss blend with
the same weights, which smooths the joins between lobes. Shading:

- three-stop ramp deeper -> shadow -> base -> light from a wrapped key light (top left),
- lighter facing-the-viewer core (subsurface feel),
- crease occlusion from the soft-max weights (darker where two lobes meet at equal height),
- mint rim light from the lower right,
- crisp Blinn specular plus a broad sheen, strongest on the big top lobe (per-lobe gloss),
- soft elliptical contact shadow (64 px and up),
- a 1 px dark outline at 16-48 px so the cloud holds its edge on light taskbars.

Palette comes from the accent: for the default `#7fd36a`, shadow is exactly `#3f8f3a` and
light is about `#a6e596`. A custom accent recolors the tray icon (as before); the exe and
window icons always use the default.

Levels of detail (`Detail`): 16-24 px use a fatter four-lobe silhouette with no motion puffs
or shadow; 32-48 px add one motion puff; 64 px and up get both puffs and the shadow.
Supersampling is 8x8 up to 32 px, 5x5 to 64, 3x3 above.

## Dynamic behavior

- **Capture animation**: `Tray::flash_capture()` plays `icon_gen::ANIM`, 7 poses x 60 ms
  (420 ms): squash, hop with a stretch, land, settle. Motion puffs rise and fade where they
  exist (32 px and up). `Tray::poll` advances frames (it already runs every frame) and
  schedules the next repaint with `wake_after`. Each frame renders one tray-size bitmap
  on the UI thread (at most 32 x 32 px x 64 samples).
- **Recording badge**: `Tray::set_recording(bool)`, idempotent, adds a red dot with a white
  ring in the lower right. Shown while the recorder is `Rec::Active` (including paused).
- **Windows**: every viewport gets `with_icon(app_icon::window_icon())` (64 px, one shared
  `Arc` so egui never re-applies it). After the HWND is found, `app_icon::apply_native` sends
  `WM_SETICON` small and big with the exe's resource icon at the window's DPI sizes, so the
  title bar and taskbar use the hand-tuned 16/20/24/32/48 frames. The root viewport also gets
  the icon: eframe stamps the root icon onto the first window that becomes active, and without
  one it used the egui logo (that is what History showed before this change).
- **Exe icon**: `.ico` with 16, 20, 24, 32, 48, 64, 128, 256 (BMP frames, 374 KB).

## Verified

- `tray-sizes.png` and `tray-animation.png` show 16/20/24/32 and the animation frames and
  badge at 20 px on light (#eee) and dark (#1c1c1c) taskbar colors.
- Debug build under a dev profile: History and Settings title bars and the taskbar button show
  the puff (screenshots taken at 3840x2160, cropped).
- Generated `stinkshot.ico` header lists 8 frames: 16, 20, 24, 32, 48, 64, 128, 256 (0).
- Unit tests in `icon_gen.rs`: transparent corners and opaque green body at every ico size,
  lit from the top left, tray sizes at least 45% solid, red badge, animation differs from
  rest and settles, ico directory sizes.

## Not verified

- The live tray icon, its animation after a real capture and the recording dot were not
  watched on screen (the tray icon sits in the overflow area, and triggering a capture would
  have overwritten the clipboard while the user was working). The rendering of every frame is
  covered by the PNGs and tests; the wiring is three calls in `shell.rs`.
- History's title bar uses eframe's resize of the 64 px icon (it is the first active window),
  so it shows the 64 px art scaled down rather than the 16-24 px silhouette; other windows get
  the native frames. Harmless, slightly less crisp.

## Shared file edits

- `crates/app/src/shell.rs` (small, additive): `drive()` adds `.with_icon(...)` to each
  viewport builder and calls `app_icon::apply_native(hwnd)` once the HWND is found;
  `root_viewport()` adds `.with_icon(...)`; `capture_saved()` and `exported()` call
  `tray.flash_capture()`; `poll_inputs()` calls `tray.set_recording(matches!(self.rec,
  Rec::Active { .. }))`.
- `crates/app/src/tray.rs`: `make_icon` takes a `Pose`; new `flash_capture`, `set_recording`,
  private `pose`, `refresh_icon`, `tick_animation`; `poll` ticks the animation first.
- `crates/app/src/main.rs`: `mod app_icon;`.
- `crates/app/Cargo.toml`: `Win32_System_LibraryLoader` feature; `image` as a dev-dependency
  (already in the tree through core) for the example.
