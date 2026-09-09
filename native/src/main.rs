#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backend;
mod calculator;
#[cfg(windows)]
mod crypto;
#[cfg(target_os = "macos")]
#[path = "mac_crypto.rs"]
mod crypto;
mod instance;
mod model;
#[cfg(windows)]
mod platform;
#[cfg(target_os = "macos")]
#[path = "mac_platform.rs"]
mod platform;
mod preview;
mod store;
mod text_tools;
mod ui;
mod visibility;

fn main() {
    if let Err(error) = ui::run() {
        eprintln!("Clibo: {error}");
        // A GUI-subsystem release still leaves a diagnosable startup error.
        if let Ok(path) = backend::data_dir() {
            let _ = std::fs::create_dir_all(&path);
            let _ = std::fs::write(path.join("startup-error.log"), &error);
        }
        std::process::exit(1);
    }
}
