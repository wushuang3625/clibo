//! Windows integration stays local: shell opening, known folders and icon extraction.
use std::path::{Path, PathBuf};
use windows_sys::Win32::{
    Graphics::Gdi::*,
    UI::{Shell::*, WindowsAndMessaging::*},
};

pub struct ShellSession(bool);
impl ShellSession {
    pub fn new() -> Self {
        Self(unsafe { windows_sys::Win32::System::Com::CoInitializeEx(std::ptr::null(), 2) } >= 0)
    }
}
impl Drop for ShellSession {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                windows_sys::Win32::System::Com::CoUninitialize();
            }
        }
    }
}

pub fn known_folders() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for folder in [FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads] {
        unsafe {
            let mut ptr = std::ptr::null_mut();
            if SHGetKnownFolderPath(&folder, 0, std::ptr::null_mut(), &mut ptr) >= 0
                && !ptr.is_null()
            {
                let mut len = 0;
                while *ptr.add(len) != 0 {
                    len += 1;
                }
                roots.push(PathBuf::from(String::from_utf16_lossy(
                    std::slice::from_raw_parts(ptr, len),
                )));
                windows_sys::Win32::System::Com::CoTaskMemFree(ptr.cast());
            }
        }
    }
    roots
}

pub fn open(target: &str, admin: bool) -> Result<(), String> {
    if target.contains('\0') {
        return Err("目标包含无效字符".into());
    }
    let target: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
    let verb: Vec<u16> = if admin { "runas" } else { "open" }
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result <= 32 {
        Err(format!(
            "无法打开该入口（系统错误 {result}），请检查目标或默认应用"
        ))
    } else {
        Ok(())
    }
}

pub fn icon(path: &Path) -> Option<Vec<u8>> {
    let path: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        // Shell icon lookup does not reliably accept mixed path separators.
        .replace('/', "\\")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        let mut info: SHFILEINFOW = std::mem::zeroed();
        if SHGetFileInfoW(
            path.as_ptr(),
            0,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON,
        ) == 0
            || info.hIcon.is_null()
        {
            return None;
        }
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            DestroyIcon(info.hIcon);
            return None;
        }
        let mut bitmap: BITMAPINFO = std::mem::zeroed();
        bitmap.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bitmap.bmiHeader.biWidth = 32;
        bitmap.bmiHeader.biHeight = -32;
        bitmap.bmiHeader.biPlanes = 1;
        bitmap.bmiHeader.biBitCount = 32;
        let mut pixels = std::ptr::null_mut();
        let dib = CreateDIBSection(
            dc,
            &bitmap,
            DIB_RGB_COLORS,
            &mut pixels,
            std::ptr::null_mut(),
            0,
        );
        if dib.is_null() || pixels.is_null() {
            DeleteDC(dc);
            DestroyIcon(info.hIcon);
            return None;
        }
        let old = SelectObject(dc, dib);
        std::ptr::write_bytes(pixels.cast::<u8>(), 0, 32 * 32 * 4);
        let ok = DrawIconEx(
            dc,
            0,
            0,
            info.hIcon,
            32,
            32,
            0,
            std::ptr::null_mut(),
            DI_NORMAL,
        ) != 0;
        let mut rgba = std::slice::from_raw_parts(pixels.cast::<u8>(), 32 * 32 * 4).to_vec();
        // Older icons have no alpha channel; retain their non-black painted pixels.
        let has_alpha = rgba.chunks_exact(4).any(|p| p[3] != 0);
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            if !has_alpha && pixel[..3].iter().any(|v| *v != 0) {
                pixel[3] = 255;
            }
        }
        SelectObject(dc, old);
        DeleteObject(dib);
        DeleteDC(dc);
        DestroyIcon(info.hIcon);
        ok.then_some(rgba)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_lookup_accepts_forward_slashes_from_catalog_paths() {
        let _session = super::ShellSession::new();
        let exe = std::env::current_exe().unwrap();
        let native = super::icon(&exe).expect("test executable has a shell icon");
        let forward = exe.to_string_lossy().replace('\\', "/");
        assert_eq!(super::icon(std::path::Path::new(&forward)), Some(native));
    }
}
