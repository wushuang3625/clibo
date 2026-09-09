# Clibo

[中文](README.md) · English

[![Native checks](https://github.com/wushuang3625/clibo/actions/workflows/native.yml/badge.svg)](https://github.com/wushuang3625/clibo/actions/workflows/native.yml)

Clibo is a native local clipboard manager built with Rust and egui. It is designed for a fast, lightweight, privacy-focused clipboard history experience on Windows.

Current version: **0.4.0 native preview**.

## Features

- Store text, links, and PNG / DIB image history.
- Chinese substring search, type filters, favorites, notes, and groups.
- Clipboard queues with reordering, sequential paste, merged paste, and recovery.
- Trim whitespace, merge blank lines, change case, and format JSON.
- Global shortcuts, system tray, pinned panels, focus-loss hiding, and single-instance behavior.
- A temporary calculator entered by typing = in the search box.
- Light / dark themes and image thumbnail previews.

## Download

Windows x64 users can download the [Clibo 0.4.0](https://github.com/wushuang3625/clibo/releases/tag/v0.4.0) portable release. Extract the archive and run Clibo.exe; Node, WebView, and other runtimes are not required.

## Quick start

1. Start Clibo and click the recording control to begin capture.
2. Copy text, links, or images in any application.
3. Press Ctrl + Shift + V to open Clibo.
4. Search and select an entry. Press Enter to try pasting, or Shift + Enter to copy only.
5. Change the panel and queue shortcuts in Preferences.

## Data and privacy

Default data locations:

- Windows: %LOCALAPPDATA%\local.clibo.native\history.db
- macOS: ~/Library/Application Support/local.clibo.native/history.db

The UI theme state is stored as ui-state.json in the same directory. Clipboard text, sources, images, and settings are stored in a local encrypted database: DPAPI on Windows, and AES-256-GCM with the system Keychain on macOS. Clibo does not upload clipboard contents.

For isolated tests or demos, set CLIBO_DATA_DIR to a separate directory:

~~~powershell
$env:CLIBO_DATA_DIR = "D:\CliboTestData"
~~~

## Build from source

Rust 1.95.0 is required and pinned by rust-toolchain.toml. Windows also requires Visual Studio C++ Build Tools; macOS requires Xcode Command Line Tools.

~~~powershell
cargo fmt --check --manifest-path native/Cargo.toml
cargo test --locked --manifest-path native/Cargo.toml
cargo clippy --locked --manifest-path native/Cargo.toml -- -D warnings
cargo build --release --locked --manifest-path native/Cargo.toml
~~~

Run the development build:

~~~powershell
cargo run --manifest-path native/Cargo.toml
~~~

Build and export portable files:

~~~powershell
npm run native:build
~~~

On Windows, the export directory is output/native/:

~~~text
Clibo.exe
使用说明.md
SHA256.txt
~~~

Node is only used by the export script; it is not required to run Clibo.

## Release

When releasing a new version, update the version in both package.json and native/Cargo.toml, then create and push a version tag:

~~~powershell
git tag -a vX.Y.Z -m "Clibo X.Y.Z"
git push origin vX.Y.Z
~~~

Pushing vX.Y.Z runs the checks, builds the Windows x64 portable package, generates a SHA256 file, and creates a GitHub Release. The workflow is [.github/workflows/release.yml](.github/workflows/release.yml).

## Platform status

- Windows x64: portable release available.
- macOS: native platform code and CI checks are present, but .app packaging, signing, and real-device validation are not complete.
- There is currently no installer, auto-updater, or code signing.

## Project layout

~~~text
native/                       Rust native application
scripts/export-native.mjs     Build artifact export script
.github/workflows/            CI and release workflows
~~~

## Contributing

Issues and pull requests are welcome. Before submitting a change, run formatting checks, tests, and Clippy, and include the operating system and reproduction steps when reporting a problem.

## License

This project is licensed under the [MIT License](LICENSE).
