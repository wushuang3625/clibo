use std::ptr;
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    },
};

// DPAPI's CurrentUser scope protects bytes at rest. It is not a sandbox against
// programs running under the same Windows account. No plaintext fallback exists.
pub fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    transform(data, true)
}
pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    transform(data, false)
}
fn transform(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    let len = u32::try_from(data.len()).map_err(|_| "内容过大，无法加密".to_string())?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: len,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    // SAFETY: input remains alive throughout the call. DPAPI allocates output;
    // it is copied before being released using LocalFree on every success path.
    unsafe {
        let ok = if encrypt {
            CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(if encrypt {
                "Windows 数据加密失败"
            } else {
                "无法解密历史，请使用原 Windows 账户打开"
            }
            .into());
        }
        let bytes = if output.cbData == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
        };
        LocalFree(output.pbData.cast());
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_roundtrip_and_tamper_rejection() {
        let raw = "Clibo 测试：不能明文落盘".as_bytes();
        let mut sealed = protect(raw).unwrap();
        assert!(!sealed.windows(raw.len()).any(|w| w == raw));
        assert_eq!(unprotect(&sealed).unwrap(), raw);
        let last = sealed.len() - 1;
        sealed[last] ^= 0x5a;
        assert!(unprotect(&sealed).is_err());
    }
}
