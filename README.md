<p align="center">
  <img src="resources/appIcon.png" width="128" height="128" />
</p>

<h1 align="center">Mi</h1>

<p align="center">
  A lightweight local music player built with Rust and <a href="https://www.gpui.rs">GPUI</a>,
  with synchronized lyrics, three window modes, and a live equalizer.
</p>

---

## Screenshots

<p align="center">
  <img src="examples/full.png" width="45%" />
  <img src="examples/mini.png" width="45%" />
</p>

## Features

- **Three window modes**: the full player, a compact mini player, and a "dynamic island" pill
  pinned to the top of the screen, with animated transitions between them. Mi reopens in the
  mode and place it was closed in
- **Play queue**: search, multi-select (Ctrl/Shift-click), drag to reorder, "play next",
  remove and clear; files and folders can be dropped anywhere on the window (or onto a row to
  insert them there). Tracks that went missing or can't be decoded are marked, and can be
  located again (other moved tracks from the same folder follow), retried or removed
- **Music library**: add your music folders once; Mi scans them recursively and afterwards
  only re-reads files that are new or changed. Browse by song, album (with covers, including
  `cover.jpg` / `folder.jpg`), artist or folder; keep favorites and see what was played
  recently. Playing from the library replaces the queue; "play next" / "add to queue" don't
- **Picks up where you left off**: the queue, current track and position are restored on
  launch (paused, unless "keep playing on launch" is on)
- **Synchronized lyrics** from `.lrc` files next to the track (or in a `lyrics/` folder),
  embedded synced lyrics, or a file you pick by hand; automatic encoding detection (UTF-8,
  UTF-16, GB18030, Shift-JIS). Click a line to jump there; scrolling pauses the auto-follow.
  Translations sharing a timestamp are shown under the original. Adjustable size, and a
  per-track timing offset on top of the file's `[offset:]`
- **Album art and tags**: embedded cover, title, artist and album, or a sidecar image
- **3-band equalizer** (bass / mid / treble) applied live, with dB readouts, presets, reset
  and an on/off switch for quick comparison
- **Playback**: repeat off / all / one, shuffle that plays every track once per round (with
  back/forward history), gapless playback between tracks of the same format, ReplayGain
  (per track or album, with pre-amp), and a choice of output device
- **Recoverable errors**: skipped tracks are reported, and a lost or missing audio device shows
  a reconnect button instead of failing silently
- **System integration**: media keys and the system media controls (Windows SMTC, macOS Now
  Playing, Linux MPRIS), system tray, and "Open with Mi" in the file manager — files are
  handed to the running player
- **Frameless translucent window** with day/night themes, adjustable material strength, pin-on-top,
  edge snapping in mini mode, an optional corner kitten, and a "reduce motion" switch that
  stops decorative animation
- **Chinese and English** interface

Supported formats: MP3, FLAC, WAV, AAC/M4A (AAC and ALAC), Ogg Vorbis.

### Keyboard shortcuts

| Key | Action |
| --- | --- |
| `Space` | Play / pause |
| `←` / `→` | Previous / next track |
| `Shift` + `←` / `→` | Back / forward 5 seconds |
| `↑` / `↓` | Volume ±5 |
| `M` | Mute |
| `L` | Play queue |
| `B` | Library |
| `Y` | Lyrics |
| `E` | Equalizer |
| `R` / `S` | Cycle repeat mode / shuffle |
| `F` | Favorite the current track |
| `Ctrl` + `F` (`⌘F`) | Search the queue or library |
| `Delete` | Remove the selected queue rows |

## Build

Requires a recent stable Rust toolchain (edition 2024).

```bash
cargo run --release
```

Platform prerequisites:

- **Windows**: Visual Studio Build Tools (MSVC) and the Windows SDK. Release builds compile
  shaders with the SDK's `fxc.exe`; if it is not found automatically, set `GPUI_FXC_PATH`
  to its full path.
- **Linux**: `clang pkg-config libssl-dev libasound2-dev libxkbcommon-dev libxkbcommon-x11-dev
  libwayland-dev libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype6-dev libvulkan-dev libdbus-1-dev`
- **macOS**: full Xcode with the Metal compiler (`xcrun --find metal` and `xcrun --find metallib`).
  The release workflow uses Xcode 16.4 and a macOS 11.0 deployment target.

Pinning, island positioning and edge snapping use Win32 APIs; on other platforms those
features are unavailable and closing the window minimizes it instead of hiding it to the tray.

The executable is `target/release/Mi.exe` on Windows and `target/release/Mi` on Linux/macOS.

To build the Windows installer locally, install [Inno Setup 6](https://jrsoftware.org/isinfo.php),
then run from the repository root:

```powershell
cargo build --release --locked
.github/scripts/build-installer.ps1 -Version 1.1.1
.github/scripts/package-windows.ps1 -Version 1.1.1
```

The installer script verifies the name and version embedded in `Mi.exe`. Pass `-BinaryPath`
when using a binary built with an explicit Rust target, or `-CompilerPath` for an Inno Setup
installation outside the default directory. Output packages go to `dist/`.

To package a Linux or macOS build locally, use the matching target:

```bash
# Linux x64; use x86_64-apple-darwin or aarch64-apple-darwin on the corresponding Mac.
cargo build --release --locked --target x86_64-unknown-linux-gnu
bash .github/scripts/package-unix.sh 1.1.1 x86_64-unknown-linux-gnu
```

On macOS, set `MACOSX_DEPLOYMENT_TARGET=11.0` before building and packaging.

## Install

Release packages are published at [Mi Releases](https://github.com/Sqhh99/Mi/releases).
Each package includes a matching `.sha256` checksum file.

| Platform | Package | Usage |
| --- | --- | --- |
| Windows x64 | `Mi-<version>-windows-x64-setup.exe` | Install for the current user, with no administrator rights; offers an optional desktop shortcut and "Open with" entries for audio files |
| Windows x64 | `Mi-<version>-windows-x64-portable.zip` | Extract and launch `Mi.exe` |
| Linux x64 | `Mi-<version>-linux-x64.tar.gz` | Extract and run `./Mi` inside the extracted folder |
| macOS Intel | `Mi-<version>-macos-x64.app.zip` | Extract and drag `Mi.app` into Applications |
| macOS Apple Silicon | `Mi-<version>-macos-arm64.app.zip` | Extract and drag `Mi.app` into Applications |

Linux releases are built on Ubuntu 22.04 and use system libraries rather than bundling them.
They require a graphical X11 or Wayland session, a working Vulkan driver, ALSA, Fontconfig,
FreeType, XCB, xkbcommon and D-Bus. A desktop portal is needed for the file picker, and a
StatusNotifier-compatible desktop or extension is needed for the tray icon. Install the
corresponding runtime packages for your distribution; `ldd ./Mi` identifies missing linked
libraries. The tarball is not an AppImage or a distribution-specific installer. To get a
launcher entry and "Open with" support, copy `mi.desktop` to `~/.local/share/applications/`,
`mi.png` to `~/.local/share/icons/`, and put `Mi` on your `PATH`.

Windows releases are not code-signed. macOS bundles use ad-hoc signing without Apple
notarization; macOS may require allowing the app in System Settings > Privacy & Security
after the first launch attempt. The deployment target is not a guarantee that all older
macOS versions have been tested.

## Release

The workspace version in `Cargo.toml` is the single source of truth:

1. Update the numeric `x.y.z` version, refresh `Cargo.lock`, and commit the changes.
2. Push a matching tag, for example `v1.1.1`.

`.github/workflows/release.yml` checks the tag before building Windows/Linux x64 and
macOS Intel/Apple Silicon packages. All four builds and workspace tests must succeed
before a single GitHub Release is created or updated with all five packages and their
SHA-256 checksums. A missing package or invalid checksum prevents publication.

You can also run the workflow manually: select a branch to download build artifacts
without publishing, or select a matching version tag to publish its release.

## Project layout

```
crates/player-core/   Platform-independent logic and unit tests:
                      play queue (shuffle/repeat/edits), library index and
                      groupings, LRC parsing, EQ filters and presets, ReplayGain,
                      settings, strings (zh/en), geometry and easing
src/
  audio/              Audio thread (rodio): equalizer, gapless source, devices
  media/              Tag, cover art, thumbnail and lyrics reading (lofty)
  model/              PlayerModel, LibraryModel and SettingsModel, shared by all views
  platform/           Win32 window helpers, tray, media controls, single instance
  ui/                 Theme, widgets and views (full / mini / island, queue, library,
                      lyrics, settings and EQ overlays)
  app.rs              Wiring: window, tray, media keys, persistence
resources/            App icon and Lucide SVG icons
installer/            Inno Setup script for the Windows installer
.github/scripts/      Local and CI packaging scripts
```

Settings (and the library index, `library.json`) are stored as JSON in the user config
directory:

- **Windows**: `%APPDATA%\sqhh99\Mi\config\settings.json`
- **Linux**: `$XDG_CONFIG_HOME/mi/settings.json`, or `~/.config/mi/settings.json`
- **macOS**: `~/Library/Application Support/com.sqhh99.Mi/settings.json`

Set `MI_CONFIG_DIR` to use another directory instead, e.g. to try a build without touching
your own queue and library; such a run is its own instance and doesn't hand over to an
already running Mi.

Mi starts with new settings and does not import the former MusicPlayer configuration.
Old configuration files are retained. The per-user installer does not migrate a former
all-users installation; manage that installation separately.

```bash
cargo test --workspace   # all tests
cargo test -p player-core   # core logic only, no GPUI build needed
```

## Tech stack

- **UI**: [GPUI](https://crates.io/crates/gpui)
- **Audio**: [rodio](https://crates.io/crates/rodio) + [symphonia](https://crates.io/crates/symphonia)
- **Metadata**: [lofty](https://crates.io/crates/lofty)
- **System media controls**: [souvlaki](https://crates.io/crates/souvlaki)
- **Icons**: [Lucide](https://lucide.dev)
