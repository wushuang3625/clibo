//! Windows integration stays local: shell opening, known folders, packaged
//! (Microsoft Store) app enumeration / activation and icon extraction.
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use windows_sys::core::{GUID, PWSTR};
use windows_sys::Win32::{
    Foundation::SIZE,
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

/// Owns a live COM interface pointer and calls the `Release` slot captured from
/// its own vtable on drop. Wrapping a pointer immediately after acquisition means
/// early returns (and panics) can no longer leak the interface.
///
/// Safety contract for every construction site in this module: `raw` must be a
/// non-null, live COM interface pointer whose vtable begins with `IUnknown`,
/// `release` must be that vtable's `Release` slot, and each pointer is wrapped
/// exactly once (never also released by hand).
struct ComObject {
    raw: *mut c_void,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}
impl Drop for ComObject {
    fn drop(&mut self) {
        // SAFETY: upheld by the contract above: one guard per pointer.
        unsafe { (self.release)(self.raw) };
    }
}

/// A wide string the shell allocated in COM task memory; freed on drop so no
/// exit path between allocation and use can leak it.
struct CoStr(PWSTR);
impl CoStr {
    fn new(raw: PWSTR) -> Option<Self> {
        (!raw.is_null()).then_some(Self(raw))
    }
    fn to_string_lossy(&self) -> String {
        // SAFETY: COM contract guarantees a valid NUL-terminated wide string.
        unsafe { utf16_to_string(self.0) }
    }
}
impl Drop for CoStr {
    fn drop(&mut self) {
        // SAFETY: the pointer came from a successful shell call and lives in task memory.
        unsafe { windows_sys::Win32::System::Com::CoTaskMemFree(self.0.cast()) };
    }
}

pub fn known_folders() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for folder in [FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads] {
        unsafe {
            let mut ptr = std::ptr::null_mut();
            if SHGetKnownFolderPath(&folder, 0, std::ptr::null_mut(), &mut ptr) >= 0 {
                // CoStr 负责释放任务内存；提前返回或后续的 panic 都不会泄漏。
                if let Some(path) = CoStr::new(ptr) {
                    roots.push(PathBuf::from(path.to_string_lossy()));
                }
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

/// Packaged (Microsoft Store) apps never ship Start Menu shortcuts; they only exist in the
/// virtual `shell:AppsFolder` namespace with parsing names like `PackageFamily!ApplicationId`.
/// The COM interfaces needed to enumerate it are declared by hand because windows-sys ships
/// the functions but not these vtables.
const IID_SHELL_ITEM: GUID = GUID::from_u128(0x43826D1E_E718_42EE_BC55_A1E261C37BFE);
const IID_ENUM_SHELL_ITEMS: GUID = GUID::from_u128(0x70629033_E363_4A28_A567_0DB78006E6D7);
const IID_SHELL_ITEM_IMAGE_FACTORY: GUID = GUID::from_u128(0xBCC18B79_BA16_442F_80C4_8A59C30C2438);

#[repr(C)]
struct UnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}
#[repr(C)]
struct ShellItemVtbl {
    unknown: UnknownVtbl,
    bind_to_handler: unsafe extern "system" fn(
        *mut c_void,
        *const c_void,
        *const GUID,
        *const GUID,
        *mut *mut c_void,
    ) -> i32,
    get_parent: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32,
    get_display_name: unsafe extern "system" fn(*mut c_void, SIGDN, *mut PWSTR) -> i32,
    get_attributes: unsafe extern "system" fn(*mut c_void, u32, *mut u32) -> i32,
    compare: unsafe extern "system" fn(*mut c_void, *const c_void, u32, *mut i32) -> i32,
}
#[repr(C)]
struct ShellItem {
    vtbl: &'static ShellItemVtbl,
}
impl ShellItem {
    /// # Safety
    /// `raw` must be null or a live `IShellItem` interface pointer. The returned
    /// reference borrows the vtable and must not outlive the underlying object;
    /// pair it with [`ShellItem::guard`] which owns the release.
    unsafe fn from_raw(raw: *mut c_void) -> Option<&'static Self> {
        (!raw.is_null()).then(|| &*(raw as *const Self))
    }
    /// # Safety
    /// `self` must refer to a live `IShellItem`; the returned buffer is task memory
    /// and is freed by `CoStr` immediately after conversion.
    unsafe fn display_name(&self, kind: SIGDN) -> Option<String> {
        let mut name: PWSTR = std::ptr::null_mut();
        if (self.vtbl.get_display_name)(self as *const Self as *mut c_void, kind, &mut name) < 0 {
            return None;
        }
        CoStr::new(name).map(|s| s.to_string_lossy())
    }
    /// # Safety
    /// `self` must refer to a live `IShellItem` obtained from the shell exactly
    /// once; the guard becomes the single owner that releases it on drop.
    unsafe fn guard(&self) -> ComObject {
        ComObject {
            raw: self as *const Self as *mut c_void,
            release: self.vtbl.unknown.release,
        }
    }
}
#[repr(C)]
struct EnumShellItemsVtbl {
    unknown: UnknownVtbl,
    next: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void, *mut u32) -> i32,
    skip: unsafe extern "system" fn(*mut c_void, u32) -> i32,
    reset: unsafe extern "system" fn(*mut c_void) -> i32,
    clone: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32,
}
#[repr(C)]
struct EnumShellItems {
    vtbl: &'static EnumShellItemsVtbl,
}
#[repr(C)]
struct ImageFactoryVtbl {
    unknown: UnknownVtbl,
    get_image: unsafe extern "system" fn(*mut c_void, SIZE, SIIGBF, *mut HBITMAP) -> i32,
}
#[repr(C)]
struct ImageFactory {
    vtbl: &'static ImageFactoryVtbl,
}

/// Converts a NUL-terminated wide string from the shell into an owned `String`.
///
/// # Safety
/// `wide` must point to a readable, NUL-terminated UTF-16 buffer.
unsafe fn utf16_to_string(wide: PWSTR) -> String {
    let mut len = 0usize;
    while *wide.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(wide, len))
}

/// Turns a raw AppsFolder `(display name, parsing name)` pair into a launcher
/// entry. Desktop shortcuts and grouping folders are skipped because their
/// parsing names carry no `!` (`PackageFamily!ApplicationId`). Kept pure so it
/// is unit-testable without creating any shell objects.
fn packaged_entry(name: &str, aumid: &str) -> Option<super::Entry> {
    let valid = !name.is_empty()
        && aumid.contains('!')
        && !aumid.chars().any(|c| c.is_whitespace() || c.is_control());
    if !valid {
        return None;
    }
    Some(super::Entry {
        name: name.into(),
        target: format!("shell:AppsFolder\\{aumid}"),
    })
}

/// Enumerates packaged apps in `shell:AppsFolder`, skipping desktop shortcuts and
/// grouping folders whose parsing names carry no `!`.
pub fn store_apps() -> Vec<super::Entry> {
    let _session = ShellSession::new();
    let mut apps = Vec::new();
    // SAFETY: COM is initialized on this thread by `_session`; every interface
    // pointer below is null-checked and wrapped in a `ComObject` guard exactly
    // once, so all exit paths release in reverse acquisition order.
    unsafe {
        let folder: Vec<u16> = "shell:AppsFolder\0".encode_utf16().collect();
        let mut root: *mut c_void = std::ptr::null_mut();
        if SHCreateItemFromParsingName(
            folder.as_ptr(),
            std::ptr::null_mut(),
            &IID_SHELL_ITEM,
            &mut root,
        ) < 0
        {
            return apps;
        }
        if let Some(item) = ShellItem::from_raw(root) {
            let _root = item.guard();
            let mut enumerator: *mut c_void = std::ptr::null_mut();
            let bound = (item.vtbl.bind_to_handler)(
                root,
                std::ptr::null(),
                &BHID_EnumItems,
                &IID_ENUM_SHELL_ITEMS,
                &mut enumerator,
            ) >= 0
                && !enumerator.is_null();
            if bound {
                let items = &*(enumerator as *const EnumShellItems);
                let _enumerator = ComObject {
                    raw: enumerator,
                    release: items.vtbl.unknown.release,
                };
                loop {
                    let mut child: *mut c_void = std::ptr::null_mut();
                    let mut fetched: u32 = 0;
                    if (items.vtbl.next)(enumerator, 1, &mut child, &mut fetched) < 0
                        || fetched == 0
                        || child.is_null()
                    {
                        break;
                    }
                    if let Some(child_item) = ShellItem::from_raw(child) {
                        let _child = child_item.guard();
                        let name = child_item.display_name(SIGDN_NORMALDISPLAY);
                        let aumid = child_item.display_name(SIGDN_PARENTRELATIVEPARSING);
                        if let (Some(name), Some(aumid)) = (name, aumid) {
                            apps.extend(packaged_entry(&name, &aumid));
                        }
                    }
                }
            }
        }
    }
    apps
}

/// Activates a packaged app. Plain ShellExecute cannot launch `shell:AppsFolder` items from
/// elevated contexts, so the request is delegated to the running shell via explorer.exe.
pub fn activate_app(target: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let spawned = std::process::Command::new("explorer.exe")
        .arg(target)
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|e| format!("无法启动该应用：{e}"))?;
    // explorer hands the request to the running shell and exits immediately,
    // reporting failures through its own UI rather than the exit code.
    drop(spawned);
    Ok(())
}

/// Extracts a 32x32 icon for an AppsFolder entry through IShellItemImageFactory.
pub fn apps_folder_icon(target: &str) -> Option<Vec<u8>> {
    let _session = ShellSession::new();
    unsafe {
        let path: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
        let mut factory: *mut c_void = std::ptr::null_mut();
        if SHCreateItemFromParsingName(
            path.as_ptr(),
            std::ptr::null_mut(),
            &IID_SHELL_ITEM_IMAGE_FACTORY,
            &mut factory,
        ) < 0
            || factory.is_null()
        {
            return None;
        }
        let image_factory = &*(factory as *const ImageFactory);
        let _factory = ComObject {
            raw: factory,
            release: image_factory.vtbl.unknown.release,
        };
        let mut bitmap: HBITMAP = std::ptr::null_mut();
        let fetched = (image_factory.vtbl.get_image)(
            factory,
            SIZE { cx: 32, cy: 32 },
            SIIGBF_ICONONLY,
            &mut bitmap,
        ) >= 0
            && !bitmap.is_null();
        if !fetched {
            return None;
        }
        let rgba = bitmap_rgba(bitmap);
        DeleteObject(bitmap);
        rgba
    }
}

unsafe fn bitmap_rgba(bitmap: HBITMAP) -> Option<Vec<u8>> {
    let dc = CreateCompatibleDC(std::ptr::null_mut());
    if dc.is_null() {
        return None;
    }
    let mut info: BITMAPINFO = std::mem::zeroed();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    if GetDIBits(
        dc,
        bitmap,
        0,
        0,
        std::ptr::null_mut(),
        &mut info,
        DIB_RGB_COLORS,
    ) == 0
    {
        DeleteDC(dc);
        return None;
    }
    let width = info.bmiHeader.biWidth.unsigned_abs() as usize;
    let height = info.bmiHeader.biHeight.unsigned_abs() as usize;
    if width == 0 || height == 0 || width * height > 4096 * 4096 {
        DeleteDC(dc);
        return None;
    }
    info.bmiHeader.biHeight = -(height as i32);
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    info.bmiHeader.biSizeImage = (width * height * 4) as u32;
    let mut rgba = vec![0u8; width * height * 4];
    let lines = GetDIBits(
        dc,
        bitmap,
        0,
        height as u32,
        rgba.as_mut_ptr().cast(),
        &mut info,
        DIB_RGB_COLORS,
    );
    DeleteDC(dc);
    if lines as usize != height {
        return None;
    }
    bgra_to_rgba(&mut rgba);
    if width == 32 && height == 32 {
        // Match icon(): keep legacy icons without alpha data opaque.
        ensure_opaque_alpha(&mut rgba);
        return Some(rgba);
    }
    // GetImage may return a different size; resample to the 32x32 the UI expects.
    let image = image::RgbaImage::from_raw(width as u32, height as u32, rgba)?;
    Some(image::imageops::resize(&image, 32, 32, image::imageops::FilterType::Nearest).into_raw())
}

/// In-place BGRA → RGBA channel swap; both shell icon paths produce BGRA bytes.
fn bgra_to_rgba(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
}

/// Older shell icons carry no alpha at all; keep their painted (non-black)
/// pixels opaque instead of letting them become fully transparent.
fn ensure_opaque_alpha(pixels: &mut [u8]) {
    let has_alpha = pixels.chunks_exact(4).any(|p| p[3] != 0);
    if !has_alpha {
        for pixel in pixels.chunks_exact_mut(4) {
            if pixel[..3].iter().any(|v| *v != 0) {
                pixel[3] = 255;
            }
        }
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
        bgra_to_rgba(&mut rgba);
        ensure_opaque_alpha(&mut rgba);
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
    fn packaged_entry_filters_shortcuts_and_grouping_folders() {
        let good = super::packaged_entry("ChatGPT", "OpenAI.ChatGPT_2p2nqsd0c76g0!App").unwrap();
        assert_eq!(good.name, "ChatGPT");
        assert_eq!(
            good.target,
            "shell:AppsFolder\\OpenAI.ChatGPT_2p2nqsd0c76g0!App"
        );
        assert!(super::packaged_entry("", "pkg!app").is_none());
        // 分组文件夹与普通快捷方式的解析名不带 '!'。
        assert!(super::packaged_entry("记事本", "notepad.exe").is_none());
        assert!(super::packaged_entry("应用", "pkg!bad id").is_none());
        assert!(super::packaged_entry("应用", "pkg!bad\u{7}id").is_none());
    }

    #[test]
    fn pixel_helpers_swap_channels_and_force_legacy_alpha() {
        let mut pixels = vec![10u8, 20, 30, 40, 1, 2, 3, 4];
        super::bgra_to_rgba(&mut pixels);
        assert_eq!(pixels, [30, 20, 10, 40, 3, 2, 1, 4]);
        // 已经带 alpha 的图标不做任何改动。
        let before = pixels.clone();
        super::ensure_opaque_alpha(&mut pixels);
        assert_eq!(pixels, before);
        // 全零 alpha 的老图标：非黑像素补为不透明，纯黑像素保持透明。
        let mut legacy = vec![9u8, 8, 7, 0, 0, 0, 0, 0];
        super::ensure_opaque_alpha(&mut legacy);
        assert_eq!(legacy, [9, 8, 7, 255, 0, 0, 0, 0]);
        // 尾块不足 4 字节时不产生越界。
        let mut short = vec![1u8, 2];
        super::bgra_to_rgba(&mut short);
        super::ensure_opaque_alpha(&mut short);
        assert_eq!(short, [1, 2]);
    }

    #[test]
    fn com_iids_match_the_published_shell_guids() {
        // 手写 IID 易在转置时打错字节；与官方公布的字符串形式对照。
        fn text(guid: &super::GUID) -> String {
            format!(
                "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
                guid.data1,
                guid.data2,
                guid.data3,
                guid.data4[0],
                guid.data4[1],
                guid.data4[2],
                guid.data4[3],
                guid.data4[4],
                guid.data4[5],
                guid.data4[6],
                guid.data4[7],
            )
        }
        assert_eq!(
            text(&super::IID_SHELL_ITEM),
            "43826D1E-E718-42EE-BC55-A1E261C37BFE"
        );
        assert_eq!(
            text(&super::IID_ENUM_SHELL_ITEMS),
            "70629033-E363-4A28-A567-0DB78006E6D7"
        );
        assert_eq!(
            text(&super::IID_SHELL_ITEM_IMAGE_FACTORY),
            "BCC18B79-BA16-442F-80C4-8A59C30C2438"
        );
    }

    #[test]
    fn icon_lookup_accepts_forward_slashes_from_catalog_paths() {
        let _session = super::ShellSession::new();
        let exe = std::env::current_exe().unwrap();
        let native = super::icon(&exe).expect("test executable has a shell icon");
        let forward = exe.to_string_lossy().replace('\\', "/");
        assert_eq!(super::icon(std::path::Path::new(&forward)), Some(native));
    }
    #[test]
    fn store_apps_reference_packaged_namespace() {
        let apps = super::store_apps();
        for app in &apps {
            assert!(!app.name.trim().is_empty(), "{:?}", app.name);
            let aumid = app
                .target
                .strip_prefix("shell:AppsFolder\\")
                .unwrap_or_else(|| panic!("{}", app.target));
            assert!(aumid.contains('!'), "{}", app.target);
        }
    }
    #[test]
    fn apps_folder_icons_are_32px_rgba() {
        let _session = super::ShellSession::new();
        for app in super::store_apps().into_iter().take(5) {
            if let Some(rgba) = super::apps_folder_icon(&app.target) {
                assert_eq!(rgba.len(), 32 * 32 * 4, "{}", app.name);
            }
        }
    }
}
