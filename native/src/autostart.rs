//! Per-user Windows login startup; no administrator privileges required.
use std::{os::windows::ffi::OsStrExt, path::Path, ptr};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
    System::Registry::*,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE: &str = "Clibo";

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn command(path: &Path) -> Vec<u16> {
    // Preserve Unicode paths, and quote paths containing spaces.
    std::iter::once(b'"' as u16)
        .chain(path.as_os_str().encode_wide())
        .chain("\" --background".encode_utf16())
        .chain(Some(0))
        .collect()
}

fn check(status: u32) -> Result<(), String> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!(
            "无法访问开机自启设置：{}",
            std::io::Error::from_raw_os_error(status as i32)
        ))
    }
}

pub fn enabled() -> Result<bool, String> {
    let mut size = 0;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            wide(VALUE).as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut size,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    check(status).map(|()| size > 2)
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    if enabled {
        let path = std::env::current_exe().map_err(|e| format!("无法获取程序路径：{e}"))?;
        let value = command(&path);
        let mut key = ptr::null_mut();
        check(unsafe { RegCreateKeyW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), &mut key) })?;
        let status = unsafe {
            RegSetKeyValueW(
                key,
                ptr::null(),
                wide(VALUE).as_ptr(),
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        unsafe {
            RegCloseKey(key);
        }
        check(status)
    } else {
        let status = unsafe {
            RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                wide(VALUE).as_ptr(),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            check(status)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_command_quotes_unicode_path_and_starts_in_background() {
        let value = command(Path::new(r"C:\我的工具\Clibo App\Clibo.exe"));
        assert_eq!(value.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&value[..value.len() - 1]).unwrap(),
            "\"C:\\我的工具\\Clibo App\\Clibo.exe\" --background"
        );
    }
}
