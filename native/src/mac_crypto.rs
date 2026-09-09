use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::RngCore;
use std::sync::Mutex;

static KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);
fn key(create: bool) -> Result<[u8; 32], String> {
    let mut cached = KEY.lock().map_err(|_| "加密状态不可用")?;
    if let Some(key) = *cached {
        return Ok(key);
    }
    let bytes = match security_framework::passwords::get_generic_password(
        "local.clibo.native",
        "history-aes256-v1",
    ) {
        Ok(bytes) => bytes,
        Err(e) if e.code() == -25300 && create => {
            let mut bytes = vec![0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut bytes);
            security_framework::passwords::set_generic_password(
                "local.clibo.native",
                "history-aes256-v1",
                &bytes,
            )
            .map_err(|e| format!("无法保存钥匙串密钥：{e}"))?;
            bytes
        }
        Err(e) => return Err(format!("无法读取历史密钥，请解锁原账户钥匙串：{e}")),
    };
    let key: [u8; 32] = bytes.try_into().map_err(|_| "钥匙串密钥长度无效")?;
    *cached = Some(key);
    Ok(key)
}
pub fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new_from_slice(&key(true)?).map_err(|_| "加密初始化失败")?;
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let encrypted = cipher
        .encrypt(Nonce::from_slice(&nonce), data)
        .map_err(|_| "历史加密失败")?;
    let mut bytes = b"CLIBO_MAC_1".to_vec();
    bytes.extend_from_slice(&nonce);
    bytes.extend(encrypted);
    Ok(bytes)
}
pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 39 || !data.starts_with(b"CLIBO_MAC_1") {
        return Err("历史格式不兼容；Windows DPAPI 数据不能直接在 Mac 解密".into());
    }
    let cipher = Aes256Gcm::new_from_slice(&key(false)?).map_err(|_| "解密初始化失败")?;
    cipher
        .decrypt(Nonce::from_slice(&data[11..23]), &data[23..])
        .map_err(|_| "历史解密失败或数据已损坏".into())
}
