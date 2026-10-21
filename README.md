# Lumza

**Monitor brightness, contrast and volume from the Windows tray, in one click.**

[Русский](README.ru.md) · [Changelog](CHANGELOG.md) · [Contributing](CONTRIBUTING.md)

Lumza is a small open-source app for Windows 11 (and 10 21H2+). It lives in the tray and controls external monitors over DDC/CI, so you never have to reach for the buttons on the bezel. The core is Rust talking to the hardware through Win32 directly; the UI is a tiny Svelte front end in WebView2 (Tauri 2).

## Features

- **Tray flyout** with brightness, contrast and volume sliders for every monitor. Mica, system theme and accent colour, full keyboard control, mouse wheel over a slider.
- **Linked sliders.** One common slider per setting with a per-monitor calibration range (for example LG 0–100, AOC 10–85) so panels of different brightness look alike. Any monitor can be excluded from the link.
- **Profiles.** "Day", "Night" and "Games" out of the box, plus your own. A profile may change only some settings.
- **Hotkeys and OSD.** Brightness of all monitors or one, volume, profiles, schedule pause; `Ctrl+Alt+↑/↓` and `Ctrl+Alt+B` (open the flyout) by default. The OSD never takes focus.
- **Schedule.** Points at a fixed time or at sunrise/sunset (you enter the coordinates yourself: no geolocation, no network), smooth transitions, pause on manual change, skipped while a full-screen app is active. After sleep, unlock or plugging in a monitor the value that is right for that moment is applied.
- **Gentle on the hardware.** Writes at most every 100 ms while dragging, only the latest value is queued per monitor, transition steps no more often than every 30 s. Each monitor has its own thread, so a slow monitor never stalls the others or the UI.
- **Seven UI languages** (English, Русский, Español, Deutsch, Français, Português, 简体中文), strings in external JSON files (`locales/`).
- **Privacy.** No telemetry. The only network request is the update check against GitHub at start-up; turn it off in Settings → About.
- Runs **without administrator rights**.

## Install

Download from [Releases](../../releases):

| File | What it is |
| --- | --- |
| `Lumza_<version>_x64-setup.exe` | installer for Windows on x64 (per-user, no admin rights) |
| `Lumza_<version>_arm64-setup.exe` | installer for Windows on ARM64 |
| `Lumza-portable-x64.zip` / `-arm64.zip` | portable: settings stay next to the program, autostart stays off |

The [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) is required: it ships with Windows 11 and the installer fetches it when missing.

Until the app is code-signed, SmartScreen may warn you: choose "More info → Run anyway". In-app updates are verified with the project's signing key.

Uninstall from Settings → Apps. The uninstaller can also delete settings, cache and logs; the autostart entry is always removed.

## Using it

- **Left-click** the tray icon to open the flyout; `Esc` or a click elsewhere closes it. **Right-click** for the menu: refresh monitors, profiles, pause schedule, settings, quit.
- Launching Lumza a second time opens the flyout of the running copy.
- If a monitor does not respond, enable **DDC/CI** in its on-screen menu (OSD). DDC/CI does not pass through some docks, USB-C hubs, DisplayLink adapters and KVM switches. With HDR on, some monitors lock brightness.
- Do not run it together with Twinkle Tray or Monitorian: two programs writing DDC/CI at the same time disturb each other.

## Tested monitors

| Monitor | Connection | Brightness | Contrast | Volume | MCCS |
| --- | --- | :---: | :---: | :---: | --- |
| LG ULTRAGEAR (GP850) | DisplayPort | ✅ | ✅ | ✅* | 2.1 |
| AOC 27G2G3 | DisplayPort / HDMI | ✅ | ✅ | ✅* | 2.2 |

\* The control exists, but these models have no built-in speakers, so nothing audible changes.

Tried your model? Tell us in an [issue](../../issues/new/choose): model, connection, what works.

## Measured

Installed copy on Windows 11, two monitors:

| Metric | Target | Result |
| --- | --- | --- |
| Installer (NSIS) | ≤ 10 MB | 2.55 MB |
| Idle RAM, all processes incl. WebView2 (private working set, as in Task Manager) | ≤ 80 MB | 59.7 MB |
| Idle CPU, 10 minutes | ≈ 0 % | 0.07 % of one core (0.41 s over 10 minutes) |
| Tray icon after process start | < 1 s | 0.19 s |
| Flyout open (window is pre-created) | < 150 ms | 14–33 ms |
| First read of all monitors | < 2 s (3 monitors) | 0.42 s (2 monitors) |

## Data

| What | Where |
| --- | --- |
| Settings | `%APPDATA%\Lumza\settings.json` (has a `schemaVersion`; a corrupt file is renamed to `settings.broken-<time>.json`) |
| Last-known values cache | `%LOCALAPPDATA%\Lumza\cache.json` |
| Log (5 × 1 MB) | `%LOCALAPPDATA%\Lumza\logs\` |

## Build from source

You need [Rust](https://rustup.rs) (stable), [Node.js](https://nodejs.org) 22+ and Windows 10/11 with WebView2.

```bash
npm ci && npm --prefix ui ci
npm run tauri dev            # development mode
npm run tauri build          # installer in target/release/bundle/nsis
```

No monitor at hand? Use the mock backend: `LUMZA_MOCK=1`. A command-line tool for diagnosing DDC/CI: `cargo run -p lumza-cli -- list` (also `get`, `set`, `caps`).

Checks, as in CI: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`; in `ui/`: `npm run lint`, `npm run typecheck`, `npm test`.

## Not in 1.0

Input source switching, mute and screen power-off over DDC (P2), software dimming for monitors without DDC/CI, auto-profile on process start, brightness by mouse wheel over the tray icon. Details and plans are in the issues.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
