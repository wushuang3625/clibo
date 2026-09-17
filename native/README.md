# Clibo Native

This file is included as 使用说明.md in the portable export.

## Build

The repository uses Rust 1.95.0, pinned by rust-toolchain.toml. Windows requires Visual Studio C++ Build Tools; macOS requires Xcode Command Line Tools.

~~~powershell
cargo run --manifest-path native/Cargo.toml
cargo fmt --check --manifest-path native/Cargo.toml
cargo test --locked --manifest-path native/Cargo.toml
cargo clippy --locked --manifest-path native/Cargo.toml -- -D warnings
cargo build --release --locked --manifest-path native/Cargo.toml
~~~

For a Windows portable export, run:

~~~powershell
npm run native:build
~~~

The exported files are written to output/native/:

~~~text
Clibo.exe
使用说明.md
SHA256.txt
~~~

## Data location

- Windows: %LOCALAPPDATA%\local.clibo.native\history.db
- macOS: ~/Library/Application Support/local.clibo.native/history.db

Set CLIBO_DATA_DIR to use an isolated data directory for tests or demos. Clipboard content and settings are stored in the local encrypted database; Clibo does not upload clipboard contents.

## Platform status

The downloadable release targets Windows x64. macOS platform code and CI checks are included, but .app packaging, signing, and real-device validation are not complete.

## Launcher

Chinese result names support full pinyin and initials: search `weixin`, `wei xin`, or `wx` for 微信; `app wx` limits results to applications. Matching is case-insensitive. Pinyin is generated locally using each character's default pronunciation; alternate polyphonic readings are not currently expanded. Application and file results display Windows shell icons, with enlarged icons on hover.

Press Alt+S for Clibo's native floating launcher. Search apps, files and browser bookmarks; use `app`, `file`, `bm`, `sys`, `g`, `b`, `bd`, or `= expression` prefixes. Arrow keys / Tab select, Enter opens, Ctrl+Enter opens the containing folder, and Esc hides the window. Use the gear button to manage custom entries and indexed folders. Change the hotkey in Clibo preferences if another app already uses Alt+S.

File indexing defaults to Desktop, Documents and Downloads, with bounded background scans and F5 refresh. Everything integration requires an existing Everything installation and the official es.exe on PATH. Browser bookmark discovery supports Chrome, Edge, Brave and Firefox on Windows. Shutdown and restart require confirmation in the UI. This is a native implementation inspired by search launchers; it does not bundle Flow Launcher or implement its plugin ecosystem.
