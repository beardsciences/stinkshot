# Contributing

Thanks for helping. Stinkshot is a small project with firm rules; reading these first saves a
round trip.

## Ground rules

- **Local only.** No uploads, share targets, cloud destinations, accounts or telemetry, ever.
  The update check in `crates/app/src/update.rs` is the only network code. Pull requests that
  add network access anywhere else are closed.
- **Windows first.** Windows 10 1903+ and 11. Keep OS calls inside `crates/platform` and
  `crates/record` so other platforms stay possible later.
- **UI copy.** Title Case for labels (buttons, menus, headings, setting names), sentence case
  for descriptions. One short line per description; errors say what to do. Use the tokens and
  widgets in `crates/app/src/theme.rs`.
- **No AI attribution** in commits, code, comments or docs.

## Building

Requirements: Rust 1.95+ (MSVC toolchain), Visual Studio Build Tools with the C++ workload and
a Windows SDK.

```
cargo build
cargo test --workspace
```

Before sending a change:

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Desktop integration tests (real screen, clipboard, input hooks) run with
`cargo test --workspace -- --ignored --test-threads=1` on an interactive session.

To run a dev build beside an installed Stinkshot without touching its config, history,
hotkeys or registry values, set a profile: `$env:STINKSHOT_PROFILE = "dev"`.

## Where things live

`docs/ARCHITECTURE.md` maps the crates and their contracts. Each area keeps notes in
`docs/notes/<area>.md`; update the one you touch, including manual verification steps.

## Commits and pull requests

- One topic per pull request, with a short description of what changed and how you checked it.
- Add a line to the `[Unreleased]` section of `CHANGELOG.md` for anything a user would notice.
- By contributing you agree your work is licensed under GPL-3.0-or-later (see `LICENSE`).

## Releases

Maintainers only: `docs/RELEASING.md`.
