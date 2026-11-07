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
- `crates/app/examples/about_anim.rs`: runs the About header (animated orb, name, version,
  tagline) alone, reports timing and can save a screenshot.
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

**March steps.** Icons use 160 steps, the About frames 256. A ray that runs out of steps
within half an output pixel of the surface counts as a hit: rays that skim a crease crawl
along it and used to run out, which punched holes in the silhouette. The About frames
first used 64 steps, which tore the right lobe (a pale flap with a notch and a dark crease
line). Measured over 32 frames at 230 px against a 4096-step reference: 2224 wrong pixels
at 64 steps, 405 at 128, 14 at 256 (alpha off by more than 20 or a channel by more than
60). 256 steps costs nothing measurable: about 100 ms per frame single-threaded either
way, because most rays converge in far fewer steps. The near-miss rule also applies to the icons; it
changed 33 pixels of the 256 px icon by more than 20 levels (crease pixels that had been
missed), and the icon sheets and `.ico` were re-rendered.

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

**Header.** `page_about` calls `logo_anim::header`, which draws the animated orb centred at
the top and then "Stinkshot" (semibold 22), "Version x.y.z" and the tagline
"Local-first screenshots and screen recording for Windows" (13, secondary text), all
centred. The old header card with the flat accent square and its "S" is gone, so the page
has one logo. The text is pulled up 12 px into the empty space under the contact shadow.

**Orb.**

- The view is 184 logical px square with a half extent of 1.75. The orb itself is about
  125 px across. The widget is 192 px tall and as wide as the page, and the rings run past
  the 184 px square to the sides.
- The CPU ray-marcher cannot run at 60 fps, so one 12 s loop (spin plus lump cycle) is
  pre-rendered into 96 frames on worker threads and played back with a crossfade between
  neighbouring frames.
  - A still frame (t = 0.06) is rendered first on all cores and shown until the loop is
    ready.
  - The loop renders on up to 8 workers (cores - 1).
  - Supersampling is 2x2 below 1.5x display scaling and 1x at 1.5x and above. This
    deviates from the spec's cut-off of 2x: at 1.5x a frame is already 276 px, and 2x2
    would double the wait for little visible gain.
- **Crossfade on the CPU.** The two neighbouring frames are blended per channel in
  premultiplied space into one image, which is uploaded to a single texture. The first
  version drew two textures, the second on top at the blend weight, so at partly
  transparent pixels (the edges and the contact shadow) the alpha added up and pulsed
  as the playhead moved. The blend is quantised to 32 steps per frame interval and redone
  only when the step changes, which at 8 frames/s is about every repaint. The test
  `crossfade_never_adds_up_alpha` covers it.
- A reaper thread drops the CPU frames and the texture 60 s after the page was last
  shown.
- Hovering the orb eases the loop to 6 s, with a time constant of about 130 ms. Pointer
  parallax was not done: it would need a re-render.

**Bloom and pulse** (dark theme only for the bloom):

- A faint accent bloom (radius 1.25 orb radii, alpha 0.18) sits behind the orb.
- The nucleus idle pulse (`1 + 0.12 sin(2 pi t / 3)`) is a soft hot-core radial overlay,
  because the frames are fixed.

**Particles** (`stinkshot_logo::motes` and `trails`, painted live with a sprite):

- The first version's rings sat at 1.18 and 1.30 orb radii, mostly hidden by the orb, with
  20 ring motes: the field barely read. The rings now sit at 1.52 and 1.68 orb radii, so
  they clear the orb, and there are 64 motes instead of 30.
- Ring A has 28 motes on a tilted plane (open ellipse) at 1 rev / 8 s. Ring B has 20 motes,
  nearly edge-on, counter-rotating at 1 rev / 13 s. Ring motes are spread evenly with a
  little jitter, so each reads as a ring.
- 16 drift motes rise from below the orb with a wobble and a fade in/out over 5-7 s.
- **Trails.** Each ring mote in front of the orb trails the last 0.42 s of its orbit: 16
  small dim motes (26% of the head's alpha, fading and tapering away from it). With
  rings this far out, that is a streak of about 30 px, which is what makes the rings
  read as spinning. Trails are drawn before the front motes and not at all with reduced
  motion.
- Motes are perspective-projected through the orb camera, sized by depth, and twinkle.
  `motes` keeps a stable order, so index `i` is the same mote at every time, which
  `trails` relies on.
- Motes behind the orb's centre plane are drawn before it, so the orb hides them. Motes
  in front are drawn after it, with half the halo.
- Dark theme: additive #7fd36a to #d4ff86.
- Light theme: normal blending of a darker tint, #2a9a4c to #146e34 (was #3fae5a to
  #1f8a3c), so the motes hold up on the light page.
  - Over the orb, a front mote fades into the bright additive colours, using the frame's
    alpha under the mote. Without that, dark-green motes over the orb looked like specks
    of dirt.
- Layer opacity is 0.8 and peak mote alpha is 0.85.

**Only while visible.** `show` is called only while the About page is up, and only then
does it request repaints (every frame while animating). Even then it skips them:

- While the viewport is minimized it requests none. Restoring the window repaints it.
- While the window is unfocused and the foreground window's rectangle covers this
  window's (checked at most every 500 ms: `GetForegroundWindow` and `GetWindowRect`
  against egui's outer rectangle), or winit reports it occluded, it polls once a second
  instead of animating. Partial cover by several windows is not detected; it keeps
  animating then.

**Reduced motion.** `SPI_GETCLIENTAREAANIMATION` is re-read every 2 s. When animations are
off, the orb is the still frame, the motes are static at 30% opacity, there are no
trails, no bloom and no pulse, and repaints drop to every 2 s.

### Measured

These are debug builds in the worktree, on 12 cores at 1.25x display scaling. The About
frames are 230 px with 2x2 supersampling. Numbers are from
`cargo run -p stinkshot --example about_anim -- dark 6`.

| Measure | Dark | Light |
|---|---|---|
| Still frame | 172 ms | 125 ms |
| 96-frame loop (8 workers) | 5.4 s | 5.4 s |
| Frame interval, 6 s of playback (n=331) | mean 16.67 ms, p95 17.19, p99 17.44, max 18.5 | mean 16.66 ms, p95 17.26, max 21.5 (n=211) |
| `show()` CPU on the UI thread (first version) | mean 0.16 ms, p99 0.27, max 0.29 | mean 0.10 ms, max 0.51 |

- After the review fixes (256 march steps, CPU crossfade, 64 motes plus trails), the same
  run (`about_anim -- dark 4`, header included): still frame 93 ms, loop 5.0 s, frame
  interval mean 16.66 ms (p99 20.5, max 21.1, n=212), `show()` mean 2.4 ms, p99 3.7, max
  4.2. Light: loop 5.3 s, `show()` mean 2.4 ms. The `show()` increase is almost all the
  per-pixel blend of a 230 px frame (53k pixels) in an unoptimised debug build of the app
  crate; it was not measured in a release build.
- 160 px frames take about 0.18 s each on a single thread, so a 276 px frame at 1.5x takes
  about 0.25 s at 1x supersampling.
- Icon render times: 256 px 0.61 s, 128 px 0.19 s, everything else under 0.07 s each.

### Verified

- Sheets on light and dark:
  - `logo/icon-sizes.png`: every shipped size, 1:1 and zoomed.
  - `logo/tray-sizes.png`: the 16/20/24/32 bitmaps the app embeds.
  - `logo/tray-animation.png`: rest, the 7 capture frames, rest and the badge at 20 px.
  - `logo/about-animation.png`: 8 loop phases with particles and trails, composited like
    `logo_anim`. After the march-step fix the silhouette is clean in every phase.
- `logo/about-dark.png` and `logo/about-light.png`: `examples/about_anim.rs` screenshots
  of the real `logo_anim::header` (orb, rings, trails, name, version, tagline) in the
  app's fonts on the settings content colours, at 1.25x display scaling.
- The `.ico` header lists 8 frames: 16, 20, 24, 32, 48, 64, 128 and 256.
- Unit tests:
  - `stinkshot-logo`: small icons are green orbs with clear corners; tray sizes fill the
    canvas; animation frames differ; the threaded render matches the single-threaded
    one; the ico header; motes orbit, fade, and pass in front and behind.
  - `icon_gen`: embedded bitmaps are valid and match the renderer; the badge is red; the
    animation squishes, pulses and settles; a custom accent recolours to the accent's
    hue; tray size per DPI.

### Not verified

- The About page inside the running app. `examples/about_anim.rs` runs the same
  `header` in an eframe window, but the Settings window itself was not opened (the rows
  under the header are unchanged).
- The minimized and covered repaint skipping was not exercised on screen.
- The live tray animation and recording dot after a real capture, as in v1.
- The reduced-motion path on screen. It only switches what is drawn, but it was not
  toggled live.

### Shared file edits

- `Cargo.toml`: the new member `crates/logo`, the `stinkshot-logo` workspace dependency
  and the `opt-level = 3` overrides for dev and release.
- `crates/app/Cargo.toml`: `stinkshot-logo` as a dependency and a build-dependency; `gif`
  as a dev-dependency for the example.
- `crates/app/src/settings_ui.rs`: `page_about` starts with `crate::logo_anim::header(..)`
  in place of the old header card, and no longer takes the accent.
- `crates/app/src/main.rs`: `mod logo_anim;`.
- `crates/app/src/tray.rs`: doc comments only (the API is unchanged).
- `docs/ARCHITECTURE.md`: crate table row; `README.md`: logo alt text.

## v1: stink cloud (rejected)

v1 was a glossy analytic-ray-traced cartoon cloud: soft-max ellipsoid lobes, flattened
gas puffs along its base and two wavy stink lines. Concepts B (on a dark app tile) and C
(with a camera lens) were rejected earlier. It is in git history up to cc7be57.


## Nov 7 2026: particles removed

The user asked to drop the particle rings and trails around the About orb: the orb's own motion is enough. The app no longer draws motes; `stinkshot_logo::motes`/`trails` remain in the logo crate, unused by the app.

## Nov 7 2026: the About orb moves at once

**Report.** "The animated logo freezes before loading in 3-5 seconds after the page is
reached." The orb showed a still frame until all 96 loop frames had rendered, which took
5.2-5.4 s on this machine (12 logical cores, 1.25x scaling, 230 px at 2x2 supersampling).

**What changed** (`crates/app/src/logo_anim.rs`, `stinkshot_logo::Prepared`):

- **Playback order, not loop order.** The worker picks its next frame from the playhead:
  the first frame from the playhead on that is not yet at full quality. Each frame is
  split over all worker threads by rows (`Prepared::row`, rows handed out one at a time),
  so one frame lands every 60-90 ms instead of eight landing together every 0.4 s.
- **Previews.** While the page is on screen and the next frame to show is missing, a 1x
  supersampled preview of it is rendered first (about 18 ms on 12 threads, against about
  90 ms for 2x2). Full-quality frames replace previews as they arrive and overtake the
  playhead within a few frames (full frames arrive at 11-16/s, playback needs 8/s, 16/s
  while hovered). At 1.5x scaling and up there are no previews (the frames are 1x already).
- **The playhead only moves onto frames that exist** and holds on the last one, so it
  never crossfades toward a frame from elsewhere in the loop. The still frame is now loop
  frame 6 (t = 0.0625, was 0.06) and playback starts there, so the still frame flows into
  the motion.
- Coarse-to-fine (bit-reversed) order was considered and not used: crossfading frames
  0.5-4 s apart ghosts the spinning lumps, and the previews already give motion within
  2-3 UI frames.
- **Disk cache.** The finished loop is written to
  `%LOCALAPPDATA%\Stinkshot<profile>\cache\about-orb-<size>-<hash>.bin`: a header (magic,
  frame count, size, supersampling) and one PNG per frame (fast deflate) of the
  premultiplied pixels, so a load is bit-exact. The hash covers `stinkshot_logo::
  source_hash()` (FNV-1a of the renderer's `lib.rs`, so any renderer edit invalidates it),
  the size, the supersampling, the frame count and a format number. 230 px: 2.87 MB. It
  is written atomically (temp file, rename) at low priority; the folder is pruned to
  48 MB, oldest first. A damaged or undecodable file is deleted and the loop re-rendered.
  Loading decodes in playback order from the playhead on the worker threads.
  `STINKSHOT_LOGO_CACHE=off` disables it.
- **Prewarm.** The Settings window calls `logo_anim::prewarm` every frame: it starts the
  worker in the background (on the next frame, so that opening Settings straight onto
  About starts it in the foreground instead). The shell calls `prewarm_cache` once, 30 s
  after startup (pushed back while an overlay or a recording is active), which renders
  the loop only when the cache has no copy for the system DPI. The reaper frees the frames
  60 s after the last show or prewarm, but never while the worker is still running, so a
  started render always reaches the cache.
- **Priority.** Worker threads run at `THREAD_PRIORITY_BELOW_NORMAL` while the page is on
  screen (all cores but one, at most 8) and `THREAD_PRIORITY_LOWEST` in the background
  (half the cores, at most 4); the reaper drops a still-running worker to the background
  within 10 s of the page going away. Every helper thread sets its own priority (a new
  Windows thread starts at normal).

**Measured** (debug build of the app crate, renderer and `image` optimized as usual,
`cargo run -p stinkshot --example about_anim -- dark <s> [cold] [prewarm] top`; the example
now shows a blank page for 1 s first, so these are times from the header's first frame in
a window that is already up, like switching to the About page):

| Case | First image | First motion | Every frame full quality |
|---|---|---|---|
| Before (window open to motion; still frame) | 70-142 ms | 5.1-6.4 s | same |
| Cold cache | 18-40 ms | 31-50 ms | 5.6-7.7 s (previews meanwhile) |
| Warm cache | 18-73 ms | 19-69 ms | 32-386 ms (load 20-340 ms) |
| Prewarmed by Settings (2 s), cold | 0.1 ms | 12-20 ms | 5.5 s |
| Prewarmed, warm | 0.1 ms | 12 ms | at once |

- One cold run under heavy load from other processes: first image 121 ms, first motion
  186 ms.
- UI thread while the cold loop renders on 8 workers: frame interval mean 16.65-16.75 ms,
  p99 21.9-22.8 ms, max 24-26 ms, the same as after rendering (p99 21-26 ms). No
  starvation. `show()` CPU: mean 3.3-3.6 ms while frames arrive (the UI thread shares
  the cores), 2.3 ms with a warm cache and nothing rendering, against 2.2-2.4 ms before.
  It is still mostly the per-pixel blend in an unoptimised debug build.
- Startup prewarm in a dev-profile app run: 96 frames at LOWEST priority on 4 threads
  took 64 s on the busy machine, then 1.07 s to encode and write the cache. Opening
  Settings afterwards loaded the cache in the background in 35 ms (log lines in the
  dev profile's `stinkshot.log`).
- Frame intervals of about 1 s in some runs are the covered-window path: `covered()`
  compares the foreground window's rectangle only, so a topmost example window over a
  maximised foreground window counts as covered. Settings is never topmost.

**Not verified**

- The About page inside the running app (it needs a click on the About section; no
  synthetic input was sent). The Settings prewarm and startup prewarm were verified from
  the log in a dev-profile run.
- 1.5x scaling and up (1x supersampling, no previews) and multi-monitor DPI changes.
- Release-build numbers.
- Reduced motion with this change (the still is now loop frame 6 and needs only that
  frame).

## Nov 7 2026: review fixes (render order, prewarm, cache key)

- **Render order on slow machines.** Previews used to cover only the playhead and the next
  frame, and the first full-quality render was the frame already on screen. Where one full
  frame takes longer than a playback step (125 ms; 62 ms hovered), the playhead caught up
  during every full render and waited for it. `pick` now keeps previews from the playhead to
  `PREVIEW_AHEAD` (4) frames past it, and full-quality renders start one past the playhead
  once the frame on screen has something (without previews, at 1x supersampling, the empty
  frame on screen still comes first). The frame on screen gets its full frame last.
- `Stats::held` (printed by the `about_anim` example and in the debug log line) is the
  playback time spent waiting on a missing frame after the first movement.
  `STINKSHOT_LOGO_CORES=<n>` makes `worker_count` pretend the machine has `n` cores.
- Measured (`STINKSHOT_LOGO_CACHE=off`, `about_anim -- dark 8 top`, 230 px ss 2, 8 s):

  | Cores (workers) | Before: held | After: held |
  |---|---|---|
  | 4 (3) | 894 ms | 0 |
  | 12 (8) | 36 ms | 0 |
  | 2 (1) | not run | 504 ms |

  With one worker a preview itself takes about a playback step, so some waiting remains.
- **Shell prewarm vs Settings.** `prewarm_cache` (the shell's one-time 30 s prewarm at the
  system DPI) now does nothing while frames are loaded, loading or about to start
  (`started_for` or `prewarm_pending` set). Before, with Settings or About open on a monitor
  at another scale, it reset their frames to the system size and the orb restarted twice.
  Not verified on a real second monitor.
- **Cache key.** The hash now also covers the app version and the `ecolor`, `egui` and
  `epaint` versions from Cargo.lock (`build.rs` sets `STINKSHOT_EGUI_VERSIONS`), since
  frames are stored after egui's premultiply (`ColorImage::from_rgba_unmultiplied`). An
  egui upgrade re-renders the loop once in the background.
