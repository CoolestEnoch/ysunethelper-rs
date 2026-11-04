use aes::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use anyhow::{Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::{RngCore, rngs::OsRng};
const CHARS: &[u8] = b"ABCDEFGHJKMNPQRSTWXYZabcdefhijkmnprstwxyz2345678";
fn random_text(n: usize) -> Result<Vec<u8>> {
    let mut result = Vec::with_capacity(n);
    while result.len() < n {
        let mut byte = [0];
        OsRng.try_fill_bytes(&mut byte)?;
        if (byte[0] as usize) < 256 - 256 % CHARS.len() {
            result.push(CHARS[byte[0] as usize % CHARS.len()]);
        }
    }
    Ok(result)
}
pub fn encrypt(password: &str, salt: &str) -> Result<String> {
    encrypt_with(password, salt, &random_text(64)?, &random_text(16)?)
}
pub fn encrypt_with(password: &str, salt: &str, prefix: &[u8], iv: &[u8]) -> Result<String> {
    let mut data = prefix.to_vec();
    data.extend_from_slice(password.as_bytes());
    macro_rules! enc {
        ($t:ty) => {
            cbc::Encryptor::<$t>::new_from_slices(salt.as_bytes(), iv)
                .map_err(|_| anyhow::anyhow!("无效的 AES key/IV"))?
                .encrypt_padded_vec_mut::<Pkcs7>(&data)
        };
    }
    let cipher = match salt.len() {
        16 => enc!(aes::Aes128),
        24 => enc!(aes::Aes192),
        32 => enc!(aes::Aes256),
        _ => bail!("pwdEncryptSalt 长度必须为 16/24/32 字节"),
    };
    Ok(STANDARD.encode(cipher))
}
