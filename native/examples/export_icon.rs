//! Regenerate packaged icons from the original transparent artwork.
use image::{imageops::FilterType, ImageFormat};
use std::{error::Error, fs, io::Cursor, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons");
    let master = image::open(dir.join("clibo-master.png"))?.into_rgba8();
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let mut frames = Vec::new();
    for size in sizes {
        let resized = image::imageops::resize(&master, size, size, FilterType::Lanczos3);
        let mut bytes = Cursor::new(Vec::new());
        resized.write_to(&mut bytes, ImageFormat::Png)?;
        if size == 32 {
            fs::write(dir.join("clibo-tray.png"), bytes.get_ref())?;
        }
        if size == 256 {
            fs::write(dir.join("clibo.png"), bytes.get_ref())?;
        }
        frames.push(bytes.into_inner());
    }
    let mut ico = vec![0, 0, 1, 0, sizes.len() as u8, 0];
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (size, frame) in sizes.iter().zip(&frames) {
        ico.extend_from_slice(&[*size as u8, *size as u8, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += frame.len() as u32;
    }
    for frame in frames {
        ico.extend_from_slice(&frame);
    }
    fs::write(dir.join("clibo.ico"), ico)?;
    println!(
        "Exported 7 ICO sizes, window PNG and tray PNG from {}x{} artwork",
        master.width(),
        master.height()
    );
    Ok(())
}
