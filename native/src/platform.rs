use crate::model::{is_sensitive, Captured, Settings, MAX_IMAGE, MAX_TEXT};
use std::{
    mem, ptr,
    sync::{
        mpsc::{self, Receiver, SyncSender},
        OnceLock,
    },
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM},
    System::{DataExchange::*, LibraryLoader::GetModuleHandleW, Memory::*, Threading::*},
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
static NOTIFY: OnceLock<SyncSender<()>> = OnceLock::new();
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        if let Some(tx) = NOTIFY.get() {
            let _ = tx.try_send(());
        }
        return 0;
    }
    DefWindowProcW(hwnd, msg, w, l)
}
pub fn subscribe() -> Result<Receiver<()>, String> {
    let (tx, rx) = mpsc::sync_channel(1);
    let (ready_tx, ready_rx) = mpsc::channel();
    NOTIFY.set(tx).map_err(|_| "监听已经启动".to_string())?;
    std::thread::spawn(move || unsafe {
        let class = wide("Clibo.Clipboard.Listener");
        let instance = GetModuleHandleW(ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..mem::zeroed()
        };
        if RegisterClassW(&wc) == 0 {
            let _ = ready_tx.send(false);
            return;
        }
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        if hwnd.is_null() || AddClipboardFormatListener(hwnd) == 0 {
            let _ = ready_tx.send(false);
            return;
        }
        let _ = ready_tx.send(true);
        let mut msg: MSG = mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        RemoveClipboardFormatListener(hwnd);
        DestroyWindow(hwnd);
    });
    if ready_rx.recv_timeout(Duration::from_secs(3)) != Ok(true) {
        return Err("无法启动系统剪贴板监听，请重新打开 Clibo".into());
    }
    Ok(rx)
}
struct ClipboardLock;
impl ClipboardLock {
    fn acquire() -> Result<Self, String> {
        Self::with_owner(0)
    }
    fn with_owner(owner: usize) -> Result<Self, String> {
        for ms in [0, 8, 16, 32, 64] {
            if ms > 0 {
                std::thread::sleep(Duration::from_millis(ms));
            }
            if unsafe { OpenClipboard(owner as HWND) } != 0 {
                return Ok(Self);
            }
        }
        Err("剪贴板正被其他应用使用，本次内容未记录".into())
    }
}
impl Drop for ClipboardLock {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}
fn format(name: &str) -> u32 {
    unsafe { RegisterClipboardFormatW(wide(name).as_ptr()) }
}
unsafe fn format_zero(id: u32) -> bool {
    if IsClipboardFormatAvailable(id) == 0 {
        return false;
    }
    let h = GetClipboardData(id);
    if h.is_null() || GlobalSize(h) < 4 {
        return false;
    }
    let p = GlobalLock(h);
    if p.is_null() {
        return false;
    }
    let value = ptr::read_unaligned(p.cast::<u32>());
    GlobalUnlock(h);
    value == 0
}
pub fn read(settings: &Settings) -> Result<Option<Captured>, String> {
    let lock = ClipboardLock::acquire()?;
    let source = source_app(unsafe { GetClipboardOwner() });
    if settings
        .excluded_apps
        .iter()
        .any(|s| s.eq_ignore_ascii_case(&source))
    {
        return Ok(None);
    }
    unsafe {
        if IsClipboardFormatAvailable(format("Clibo.Internal")) != 0
            || IsClipboardFormatAvailable(format("ExcludeClipboardContentFromMonitorProcessing"))
                != 0
            || format_zero(format("CanIncludeInClipboardHistory"))
        {
            return Ok(None);
        }
        // Files and virtual files are deliberately outside the first release.
        if IsClipboardFormatAvailable(15) != 0 {
            return Ok(None);
        }
        if IsClipboardFormatAvailable(13) != 0 {
            let h = GetClipboardData(13);
            if h.is_null() {
                return Err("无法读取本次文字".into());
            }
            let len = GlobalSize(h);
            if len > 2 * MAX_TEXT + 2 {
                return Err("文字超过 1 MiB 限制，已跳过".into());
            }
            if len < 2 {
                return Ok(None);
            }
            let p = GlobalLock(h);
            if p.is_null() {
                return Err("无法锁定剪贴板内容".into());
            }
            let units = std::slice::from_raw_parts(p.cast::<u16>(), len / 2);
            let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
            let text = String::from_utf16_lossy(&units[..end]);
            GlobalUnlock(h);
            if text.is_empty() {
                return Ok(None);
            }
            if text.len() > MAX_TEXT {
                return Err("文字超过 1 MiB 限制，已跳过".into());
            }
            if settings.detect_sensitive && is_sensitive(&text) {
                return Err("检测到疑似密钥，已跳过记录".into());
            }
            return Ok(Some(Captured {
                text: Some(text),
                png: None,
                source,
            }));
        }
        if !settings.capture_images {
            return Ok(None);
        }
        // Copy a bounded snapshot while the original owner's privacy flags are
        // still protected by the same clipboard lock. Decode only after closing.
        let png_format = format("PNG");
        let image_format = if IsClipboardFormatAvailable(png_format) != 0 {
            png_format
        } else if IsClipboardFormatAvailable(17) != 0 {
            17
        } else if IsClipboardFormatAvailable(8) != 0 {
            8
        } else {
            return Ok(None);
        };
        let h = GetClipboardData(image_format);
        if h.is_null() {
            return Err("无法读取本次图片".into());
        }
        let len = GlobalSize(h);
        let is_png = image_format == png_format;
        if len == 0 || len > if is_png { MAX_IMAGE } else { 110 * 1024 * 1024 } {
            return Err("图片格式或大小不支持，已跳过".into());
        }
        let p = GlobalLock(h);
        if p.is_null() {
            return Err("图片读取失败".into());
        }
        let bytes = std::slice::from_raw_parts(p.cast::<u8>(), len).to_vec();
        GlobalUnlock(h);
        drop(lock);
        let png = normalize_image(&bytes, is_png)?;
        Ok(Some(Captured {
            text: None,
            png: Some(png),
            source,
        }))
    }
}
fn normalize_image(bytes: &[u8], png: bool) -> Result<Vec<u8>, String> {
    use image::ImageDecoder;
    fn decode(mut decoder: impl ImageDecoder) -> Result<image::DynamicImage, String> {
        let (w, h) = decoder.dimensions();
        if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 25_000_000 {
            return Err("图片超过 2500 万像素，已跳过".into());
        }
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(110 * 1024 * 1024);
        decoder.set_limits(limits).map_err(|_| "图片过大，已跳过")?;
        image::DynamicImage::from_decoder(decoder).map_err(|_| "图片数据不完整".into())
    }
    let cursor = std::io::Cursor::new(bytes);
    let img = if png {
        decode(image::codecs::png::PngDecoder::new(cursor).map_err(|_| "此 PNG 图片无法读取")?)?
    } else {
        decode(
            image::codecs::bmp::BmpDecoder::new_without_file_header(cursor)
                .map_err(|_| "此位图格式暂不支持")?,
        )?
    };
    let mut buffer = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buffer, image::ImageFormat::Png)
        .map_err(|_| "图片编码失败".to_string())?;
    let png = buffer.into_inner();
    if png.len() > MAX_IMAGE {
        return Err("图片超过 10 MiB 限制，已跳过".into());
    }
    Ok(png)
}
pub fn source_app(hwnd: HWND) -> String {
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return "未知来源".into();
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return "未知来源".into();
        }
        let mut buf = vec![0u16; 32768];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(process);
        if ok == 0 {
            return "未知来源".into();
        }
        String::from_utf16_lossy(&buf[..size as usize])
            .rsplit('\\')
            .next()
            .unwrap_or("未知来源")
            .to_string()
    }
}
// On success ownership of this allocation transfers to the clipboard manager.
unsafe fn put_bytes(id: u32, bytes: &[u8]) -> Result<(), String> {
    let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
    if h.is_null() {
        return Err("剪贴板内存分配失败".into());
    }
    let p = GlobalLock(h);
    if p.is_null() {
        windows_sys::Win32::Foundation::GlobalFree(h);
        return Err("剪贴板内存锁定失败".into());
    }
    ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast(), bytes.len());
    GlobalUnlock(h);
    if SetClipboardData(id, h).is_null() {
        windows_sys::Win32::Foundation::GlobalFree(h);
        return Err("无法写入系统剪贴板".into());
    }
    Ok(())
}
pub(crate) fn image_clipboard_bitmaps(png: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|_| "图片解码失败".to_string())?
        .into_rgba8();
    let (width, height) = (img.width(), img.height());
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 25_000_000 {
        return Err("图片尺寸超过限制".into());
    }
    let mut dib = vec![0u8; 124];
    for (offset, value) in [
        (0, 124u32),
        (4, width),
        (8, height),
        (16, 3),
        (20, width * height * 4),
        (40, 0x00ff0000),
        (44, 0x0000ff00),
        (48, 0x000000ff),
        (52, 0xff000000),
        (56, 0x73524742),
        (108, 4),
    ] {
        dib[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    dib[12..14].copy_from_slice(&1u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32u16.to_le_bytes());
    // Positive height / bottom-up rows also work with consumers which reject
    // top-down DIBV5 (notably older Word/WordPad clipboard implementations).
    for row in img.rows().rev() {
        for p in row {
            dib.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    // Explicit CF_DIB fallback: no dependence on Windows synthesizing a
    // legacy header from DIBV5. Alpha-less consumers receive a white matte;
    // PNG and DIBV5 retain the original alpha channel.
    let stride = (width as usize * 3 + 3) & !3;
    let mut legacy = vec![0u8; 40 + stride * height as usize];
    for (offset, value) in [
        (0, 40u32),
        (4, width),
        (8, height),
        (20, (stride * height as usize) as u32),
    ] {
        legacy[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    legacy[12..14].copy_from_slice(&1u16.to_le_bytes());
    legacy[14..16].copy_from_slice(&24u16.to_le_bytes());
    for (y, row) in img.rows().rev().enumerate() {
        for (x, p) in row.enumerate() {
            let alpha = u32::from(p[3]);
            let flatten = |c: u8| ((u32::from(c) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
            let offset = 40 + y * stride + x * 3;
            legacy[offset..offset + 3].copy_from_slice(&[
                flatten(p[2]),
                flatten(p[1]),
                flatten(p[0]),
            ]);
        }
    }
    Ok((dib, legacy))
}
pub fn write(text: Option<&str>, png: Option<&[u8]>, owner: usize) -> Result<(), String> {
    if let Some(png) = png {
        let (dib, legacy) = image_clipboard_bitmaps(png)?;
        let _lock = ClipboardLock::with_owner(owner)?;
        unsafe {
            if EmptyClipboard() == 0 {
                return Err("无法更新剪贴板".into());
            }
            put_bytes(format("Clibo.Internal"), &1u32.to_le_bytes())?;
            put_bytes(format("CanUploadToCloudClipboard"), &0u32.to_le_bytes())?;
            // Enumerating consumers should see the lossless PNG first.
            put_bytes(format("PNG"), png)?;
            put_bytes(17, &dib)?;
            put_bytes(8, &legacy)?;
        }
    } else if let Some(text) = text {
        let data: Vec<u8> = wide(text).into_iter().flat_map(u16::to_le_bytes).collect();
        let _lock = ClipboardLock::with_owner(owner)?;
        unsafe {
            if EmptyClipboard() == 0 {
                return Err("无法更新剪贴板".into());
            }
            put_bytes(format("Clibo.Internal"), &1u32.to_le_bytes())?;
            put_bytes(format("CanUploadToCloudClipboard"), &0u32.to_le_bytes())?;
            put_bytes(13, &data)?;
        }
    } else {
        return Err("没有可复制的内容".into());
    }
    Ok(())
}
pub fn foreground() -> usize {
    unsafe { GetForegroundWindow() as usize }
}
pub fn valid_target(target: usize, own: usize) -> bool {
    target != 0 && target != own && unsafe { IsWindow(target as HWND) } != 0
}
pub fn activate(target: usize) -> bool {
    unsafe { SetForegroundWindow(target as HWND) != 0 }
}
pub fn paste(target: usize) -> Result<(), String> {
    // Wait for the selection Enter / mouse click to finish. Never release keys
    // physically held by the user or inject into a different foreground window.
    for _ in 0..30 {
        let busy = [
            VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN, VK_RETURN, VK_LBUTTON,
        ]
        .iter()
        .any(|k| unsafe { GetAsyncKeyState(*k as i32) } < 0);
        if !busy {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if foreground() != target {
        return Err("已复制；原窗口未获得焦点，请手动粘贴".into());
    }
    if [
        VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN, VK_RETURN, VK_LBUTTON,
    ]
    .iter()
    .any(|k| unsafe { GetAsyncKeyState(*k as i32) } < 0)
    {
        return Err("已复制；请松开快捷键后手动粘贴".into());
    }
    let key = |vk, flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [
        key(VK_CONTROL, 0),
        key(0x56, 0),
        key(0x56, KEYEVENTF_KEYUP),
        key(VK_CONTROL, KEYEVENTF_KEYUP),
    ];
    if unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            mem::size_of::<INPUT>() as i32,
        )
    } != inputs.len() as u32
    {
        return Err("已复制；目标可能有权限限制，请手动粘贴".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_paste_formats_preserve_orientation_color_and_alpha() {
        use image::ImageDecoder;
        let rgba = image::RgbaImage::from_raw(
            3,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 0, 255,
                255, 0, 255, 255,
            ],
        )
        .unwrap();
        let mut encoded = std::io::Cursor::new(Vec::new());
        rgba.write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let (v5, legacy) = image_clipboard_bitmaps(encoded.get_ref()).unwrap();
        assert_eq!(i32::from_le_bytes(v5[8..12].try_into().unwrap()), 2);
        assert_eq!(i32::from_le_bytes(legacy[8..12].try_into().unwrap()), 2);
        assert_eq!(legacy.len(), 40 + 12 * 2); // 24-bit odd width requires padding.
        let decode = |data: Vec<u8>| {
            // Give the decoder an explicit pixel offset. image 0.25.10's
            // headerless reader assumes another 12 mask bytes after V5, even
            // though our packed clipboard V5 already embeds its masks.
            let header_size = u32::from_le_bytes(data[0..4].try_into().unwrap());
            let mut file = vec![0u8; 14];
            file[0..2].copy_from_slice(b"BM");
            file[2..6].copy_from_slice(&((data.len() + 14) as u32).to_le_bytes());
            file[10..14].copy_from_slice(&(14 + header_size).to_le_bytes());
            file.extend_from_slice(&data);
            let decoder = image::codecs::bmp::BmpDecoder::new(std::io::Cursor::new(file)).unwrap();
            assert_eq!(decoder.dimensions(), (3, 2));
            image::DynamicImage::from_decoder(decoder)
                .unwrap()
                .to_rgba8()
        };
        assert_eq!(decode(v5), rgba);
        let rgb = decode(legacy);
        assert_eq!(rgb.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(rgb.get_pixel(1, 0).0, [127, 255, 127, 255]);
        assert_eq!(rgb.get_pixel(2, 0).0, [255, 255, 255, 255]);
        assert_eq!(rgb.get_pixel(0, 1).0, [0, 0, 255, 255]);
        assert!(image_clipboard_bitmaps(b"not a PNG").is_err());
    }
    #[test]
    fn dib_bottom_up_rows_and_png_roundtrip() {
        // Two 24-bit pixels on separate rows, each padded to four bytes.
        let mut dib = vec![0u8; 40];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&2i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[255, 0, 0, 0, 0, 0, 255, 0]);
        let png = normalize_image(&dib, false).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (1, 2));
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0]);
        assert_eq!(img.get_pixel(0, 1).0, [0, 0, 255]);
        assert_eq!(normalize_image(&png, true).unwrap(), png);
        dib[4..8].copy_from_slice(&100_000i32.to_le_bytes());
        dib[8..12].copy_from_slice(&100_000i32.to_le_bytes());
        assert!(normalize_image(&dib, false).is_err());
        assert!(normalize_image(&[0; 12], false).is_err());
        assert!(normalize_image(&[0; 12], true).is_err());
    }
}
