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
