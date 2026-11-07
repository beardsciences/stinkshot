# Logo notes

Files:

- `crates/logo/src/lib.rs` (`stinkshot-logo`): the renderer and the particle field. It is
  pure std.
- `crates/app/build.rs`: renders the exe `.ico` and the embedded tray and window bitmaps.
- `crates/app/src/icon_gen.rs`: the embedded bitmaps, the tray pose animation, accent
  recolouring and the recording badge.
- `crates/app/src/logo_anim.rs`: the animated About page orb.
- `crates/app/src/app_icon.rs`: the window icons.
- `crates/app/src/tray.rs`: the tray.
- `crates/app/examples/logo.rs`: PNG and GIF export.
- `crates/app/examples/about_anim.rs`: runs the About animation alone and reports timing.
- Images: `docs/logo-256.png` and `docs/notes/logo/*.png`.

To regenerate every PNG, run `cargo run -p stinkshot --example logo [-- out.gif]`.

## v2: gel orb (current)

### Why v1 was replaced

v1 was a cartoon green cloud with stink lines and a trail of gas puffs. The user rejected
it: it was not modern-looking enough, and the stink lines and trail did not work. They
asked for "more of a green puffy lumpy orb", and for the orb on the About page to spin and
have particles.

### Concepts

There were three ray-marched concepts, each with a sheet showing 256, 48, 32, 20 and 16 px
on light (#f3f3f3) and dark (#202020) backgrounds, with zooms.

| Concept | Sheet | Idea |
|---|---|---|
| Gel | `logo/concept-gel-sizes.png`, `logo/concept-gel-anim.png` | Translucent glossy gel orb with a strip-light rim, a glowing core and a contact shadow. |
| Plasma | `logo/concept-plasma-sizes.png` | Luminous lime core inside an emerald shell, with orbiting particle rings. |
| Clay | `logo/concept-clay-sizes.png` | Matte soft-touch cluster of fused spheres. |

**Picked: gel.** Reasons:

- At 256 it reads as a current 3D app icon. It is one orb with lumps, not a cluster, and
  nothing about it suggests a cloud.
- It holds up best at tray sizes: a bright rounded blob with a white glint pixel and a
  dark edge. The edge separates it from a light taskbar, and the bright body stands out
  on a dark one.
- Plasma was soft and muddy at 256 and became a grey-outlined rounded square at 16-32.
- Clay reads as broccoli or cauliflower (a real risk for a green bumpy ball). It looks
  friendly rather than premium.

### Changes made to gel before shipping

Each change was checked against the original gel strip
(`logo/concept-gel-anim.png` vs `logo/about-animation.png`).

1. **Stable highlight.** In the original, the highlight smeared into streaks on some
   rotation frames. The specular is now computed on a normal blended toward the orb's
   sphere normal: 50% for the broad sheen, 78% for the crisp glint. The softbox seen in the
   Fresnel reflection uses the 50% normal too. The glint is now a round dot that slides
   over the dome.
   - Testing showed the streaks came from the sharp glint term, not from the reflection.
   - The spec's 0.5 blend still streaked.
   - The broad sheen is weaker than specified, `0.15 (Ns.H)^60` instead of
     `0.35 (Ns.H)^40`, because the specified one turned the 256 glint into haze. The crisp
     term is `4.0 (Ng.H)^700`; the higher power keeps it small on the flatter blended
     normal.
2. **Crease tint (from clay).** Creases between lumps blend toward deep green #06301a
   (`(1-AO)*0.45`) instead of going grey-black.
3. **Neon patch.** The low glow was moved to (0.05, -0.30, 0.15). The transmitted
   environment is capped at 0.45, not the specified 0.7: at 0.7, thin lumps seen against
   the softbox still glowed neon lime (found by switching off one term at a time).
4. **Lighter outline at small sizes.** The outline is #0f4a14, 0.85 px at 16, 0.8 px at 20,
   0.75 px at 24, 0.7 px at 32 and none from 48. It is accumulated in premultiplied
   linear space, so it does not go grey.

### How it is drawn (`stinkshot-logo`)

**Shape.** The orb is a signed distance field: a core sphere (r 0.86) smooth-unioned
(`k` 0.30) with eight lumps. As the orb spins, the lumps wander over the surface (once or
twice per loop) and breathe (twice per loop), so the loop is seamless. Large sizes add a
0.02 sine displacement. The march stops early for rays that miss the bounding sphere
(radius 1.45). The camera is a perspective view from (0, 1, 6.2).

**Shading** (in linear light):

- A studio environment: an elliptical softbox to the upper left, a broad sheen around it,
  a rim strip on the right, a sky gradient and a green floor bounce.
- Schlick Fresnel reflection.
- Refraction (IOR 1.42) with Beer-Lambert absorption (`sigma` 3.4/0.85/4.2) over the real
  thickness, found by marching to the far side.
- An inner glow and a luminous nucleus, integrated along the refracted path.
- Wrap diffuse, a bottom caustic, 5-tap AO plus the crease tint, and a mint Fresnel rim.

**Output.** ACES tone mapping, then supersampling in premultiplied linear space: 5x5 at
128-256, 6x6 at 64, 8x8 at 48 and below. The result is dithered by about half an LSB to
straight-alpha sRGB8.

**Small sizes (32 and below).** `k` is 0.40 at 16-24 and 0.34 at 32, with no displacement
and no shadow. The softbox has a wider edge so the glint survives as one or two white
pixels, absorption is lighter and exposure is 20% higher. Below 20 px the lumps disappear,
and the silhouette, glint and bright core are what read in the tray.

**Contact shadow.** 0.55 at 256, 0.50 at 128, 0.42 at 64, 0.35 at 48, and 0.30 on the
About page.

### Build and runtime cost

**Workspace profile overrides.** `stinkshot-logo` is built with `opt-level = 3` in dev and
release. Without the release override, cargo builds build-script dependencies at
`opt-level` 0, which would make the build script take minutes.

**Build script.** `build.rs` renders all eight `.ico` sizes once, which takes 1.4 s in
`cargo build --timings` on this 12-core machine. It writes:

- `stinkshot.ico`: 16, 20, 24, 32, 48, 64, 128 and 256 px, BMP frames, 374 KB.
- `logo-{16,20,24,32,64}.rgba`, which `icon_gen.rs` includes with `include_bytes!`.

So the app never ray-marches an icon at runtime.

**Test.** `embedded_bitmaps_match_the_renderer` checks that the embedded bitmaps are
current.

**Accent colour.** A custom accent still recolours the tray icon, by shifting hue and
scaling saturation from #7fd36a, which keeps the lightness and so the gloss. The exe and
window icons always use the default.

### Tray

- **Capture animation.** `Tray::flash_capture()` plays `icon_gen::ANIM`: 7 poses x 60 ms.
  The orb squishes (wider and flatter, about its base), then pops narrow with a bright
  pulse toward the hot core colour #d4ff86, then wobbles back while the pulse fades.
  - Each frame is a bilinear resample of the embedded bitmap plus the brightening.
  - Poses never grow past the canvas, because the orb already fills it. An earlier hop
    clipped the top.
  - `Tray::poll` advances the frames and schedules the next repaint.
- **Recording badge.** `Tray::set_recording(bool)` adds a red dot with a white ring in the
  lower right. It is now supersampled.
- **Windows.** Unchanged from v1: every viewport, toasts included, gets
  `app_icon::window_icon()` (now the embedded 64 px orb). The native `WM_SETICON` uses the
  exe's resource frames at the window's DPI.

### About page (`logo_anim.rs`)

`settings_ui.rs` calls `crate::logo_anim::show(ui)` as the first line of `page_about`,
which adds 192 logical px above the existing card.

**Orb.**

- The view is 184 logical px square with a half extent of 1.75, which leaves room for the
  particle rings. The orb itself is about 125 px across.
- The CPU ray-marcher cannot run at 60 fps, so one 12 s loop (spin plus lump cycle) is
  pre-rendered into 96 frames on worker threads and played back with a crossfade between
  neighbouring frames.
  - A still frame (t = 0.06) is rendered first on all cores and shown until the loop is
    ready.
  - The loop renders on up to 8 workers (cores - 1).
  - Supersampling is 2x2 below 1.5x display scaling and 1x at 1.5x and above. This
    deviates from the spec's cut-off of 2x: at 1.5x a frame is already 276 px, and 2x2
    would double the wait for little visible gain.
- Two GPU textures are re-uploaded as the playhead moves. That is about 7.5 uploads/s of
  one frame each, so the GPU never holds 96 textures.
- A reaper thread drops the CPU frames and textures 60 s after the page was last shown.
- Hovering the orb eases the loop to 6 s, with a time constant of about 130 ms. Pointer
  parallax was not done: it would need a re-render.

**Bloom and pulse** (dark theme only for the bloom):

- A faint accent bloom (radius 1.25 orb radii, alpha 0.18) sits behind the orb.
- The nucleus idle pulse (`1 + 0.12 sin(2 pi t / 3)`) is a soft hot-core radial overlay,
  because the frames are fixed.

**Particles** (`stinkshot_logo::motes`, painted live with a sprite):

- Ring A has 12 motes on a tilted plane at 1 rev / 8 s. Ring B has 8 motes, counter-
  rotating at 1 rev / 13 s.
- 10 drift motes rise from below the orb with a wobble and a fade in/out over 5-7 s.
- Motes are perspective-projected through the orb camera, sized by depth, and twinkle.
- Motes behind the orb's centre plane are drawn before it, so the orb hides them. Motes
  in front are drawn after it, with half the halo.
- Dark theme: additive #7fd36a to #d4ff86.
- Light theme: normal blending of #3fae5a to #1f8a3c.
  - Over the orb, a front mote fades into the bright additive colours, using the frame's
    alpha under the mote. Without that, dark-green motes over the orb looked like specks
    of dirt.
- Layer opacity is 0.8 and peak mote alpha is 0.85.

**Only while visible.** `show` is called only while the About page is up, and only then
does it request repaints (every frame while animating).

**Reduced motion.** `SPI_GETCLIENTAREAANIMATION` is re-read every 2 s. When animations are
off, the orb is the still frame, the motes are static at 30% opacity, there is no bloom
or pulse, and repaints drop to every 2 s.

### Measured

These are debug builds in the worktree, on 12 cores at 1.25x display scaling. The About
frames are 230 px with 2x2 supersampling. Numbers are from
`cargo run -p stinkshot --example about_anim -- dark 6`.

| Measure | Dark | Light |
|---|---|---|
| Still frame | 172 ms | 125 ms |
| 96-frame loop (8 workers) | 5.4 s | 5.4 s |
| Frame interval, 6 s of playback (n=331) | mean 16.67 ms, p95 17.19, p99 17.44, max 18.5 | mean 16.66 ms, p95 17.26, max 21.5 (n=211) |
| `show()` CPU on the UI thread | mean 0.16 ms, p99 0.27, max 0.29 | mean 0.10 ms, max 0.51 |

- 160 px frames take about 0.18 s each on a single thread, so a 276 px frame at 1.5x takes
  about 0.25 s at 1x supersampling.
- Icon render times: 256 px 0.61 s, 128 px 0.19 s, everything else under 0.07 s each.

### Verified

- Sheets on light and dark:
  - `logo/icon-sizes.png`: every shipped size, 1:1 and zoomed.
  - `logo/tray-sizes.png`: the 16/20/24/32 bitmaps the app embeds.
  - `logo/tray-animation.png`: rest, the 7 capture frames, rest and the badge at 20 px.
  - `logo/about-animation.png`: 8 loop phases with particles, composited like
    `logo_anim`.
- `examples/about_anim.rs` screenshots of the real egui widget on dark and light (saved
  outside the repo) match the strip.
- The `.ico` header lists 8 frames: 16, 20, 24, 32, 48, 64, 128 and 256.
- Unit tests:
  - `stinkshot-logo`: small icons are green orbs with clear corners; tray sizes fill the
    canvas; animation frames differ; the threaded render matches the single-threaded
    one; the ico header; motes orbit, fade, and pass in front and behind.
  - `icon_gen`: embedded bitmaps are valid and match the renderer; the badge is red; the
    animation squishes, pulses and settles; a custom accent recolours to the accent's
    hue; tray size per DPI.

### Not verified

- The About page inside the running app. `examples/about_anim.rs` runs the same `show`
  in an eframe window, but the Settings window itself was not opened, because the user's
  release exe was running.
- The live tray animation and recording dot after a real capture, as in v1.
- The reduced-motion path on screen. It only switches what is drawn, but it was not
  toggled live.

### Shared file edits

- `Cargo.toml`: the new member `crates/logo`, the `stinkshot-logo` workspace dependency
  and the `opt-level = 3` overrides for dev and release.
- `crates/app/Cargo.toml`: `stinkshot-logo` as a dependency and a build-dependency; `gif`
  as a dev-dependency for the example.
- `crates/app/src/settings_ui.rs`: one line, `crate::logo_anim::show(ui);`, at the top of
  `page_about`.
- `crates/app/src/main.rs`: `mod logo_anim;`.
- `crates/app/src/tray.rs`: doc comments only (the API is unchanged).
- `docs/ARCHITECTURE.md`: crate table row; `README.md`: logo alt text.

## v1: stink cloud (rejected)

v1 was a glossy analytic-ray-traced cartoon cloud: soft-max ellipsoid lobes, flattened
gas puffs along its base and two wavy stink lines. Concepts B (on a dark app tile) and C
(with a camera lens) were rejected earlier. It is in git history up to cc7be57.
