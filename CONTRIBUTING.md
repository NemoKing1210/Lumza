# Contributing

Thank you for helping! Pull requests, bug reports and "it works on my monitor" notes are all welcome.

## Reporting a problem

Use the **Bug report** form. DDC/CI behaves differently on every monitor, so please include the monitor model, the connection type (direct cable, dock, hub, KVM …) and the end of the log (`%LOCALAPPDATA%\Lumza\logs\lumza.log`; Settings → About shows the path). The command `lumzactl caps 1` prints what the monitor reports.

## Getting started

```bash
npm ci && npm --prefix ui ci
LUMZA_MOCK=1 npm run tauri dev     # mock monitors: no hardware needed
```

Layout:

| Path | What lives there |
| --- | --- |
| `crates/lumza-core` | hardware-independent logic: DDC/CI backend, capabilities parser, scaling and calibration, settings store, schedule maths. Everything here has unit tests. |
| `crates/lumza-cli` | `lumzactl`, a small diagnostic tool (`list`, `get`, `set`, `caps`) |
| `src-tauri` | the app: controller (single owner of monitor state), per-monitor workers, tray, flyout/OSD windows, hotkeys, scheduler, updater |
| `ui` | Svelte 5 front end: flyout, settings window, OSD |
| `locales` | one `<code>.json` per language (`en.json` is the reference), shared by the UI and the Rust side |

Architecture rule: tray, hotkeys, schedule, OS events and the UI never touch monitors directly. They send commands to the **controller**, which coalesces writes (only the latest value per feature is queued) and hands them to the monitor's own worker thread.

## Before you open a pull request

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd ui && npm run lint && npm run typecheck && npm test
```

- Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:`, `refactor:` …). The changelog is generated from them with [git-cliff](https://git-cliff.org) (`cliff.toml`).
- Every user-visible string goes into **every** `locales/*.json` file (English first, it is the fallback); a test fails if the files disagree or the UI uses a missing key.
- Be gentle with the hardware: no polling timers, no write loops. DDC/CI writes wear a monitor's EEPROM.
- Tested on hardware? Say which monitor and how it was connected in the PR description.

## Releases (maintainers)

Update `version` in `Cargo.toml`, `src-tauri/tauri.conf.json` and `ui/package.json`, run `git cliff --tag vX.Y.Z --prepend CHANGELOG.md --unreleased`, commit, then push a tag `vX.Y.Z`. The *Release* workflow builds x64 and ARM64 installers, the portable archives and the signed updater manifest `latest.json` as a **draft** release; publish it once you have checked the assets.

One-time setup: generate the update signing key with `npx tauri signer generate`, put its public half into `plugins.updater.pubkey` in `src-tauri/tauri.conf.json`, and store the private key and its password as the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Keep a backup of the private key: without it installed copies cannot be updated. Replace `OWNER/lumza` in `tauri.conf.json` (updater endpoint) and `src-tauri/src/update.rs` (`REPO_URL`) with the real repository.

Code signing (to silence SmartScreen) is not set up yet; [SignPath Foundation](https://signpath.org/) offers it free for open-source projects and needs an application.
