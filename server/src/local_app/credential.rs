//! Reads the signed-in Cursor account credential stored by the local Cursor app.
//!
//! The desktop app keeps `cursorAuth/accessToken` in its VS Code style
//! `state.vscdb`, encrypted with the Chromium safe-storage scheme. Reading it
//! never modifies Cursor state; it only lets the overview import account usage.
use std::path::PathBuf;

#[cfg(target_os = "macos")]
use std::process::Command;

use aes::Aes128;
use base64::{engine::general_purpose::STANDARD, Engine};
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use sqlx::{Connection, Row, SqliteConnection};

use crate::{Error, Result};

const PREFIX: &str = "cursor-safe-storage:v1:";
const ACCESS_TOKEN_KEY: &str = "cursorAuth/accessToken";
const CHROMIUM_SALT: &[u8] = b"saltysalt";
const CHROMIUM_ITERATIONS: u32 = 1003;
const CHROMIUM_IV: [u8; 16] = [0x20; 16];
#[cfg(target_os = "macos")]
const MACOS_KEYCHAIN_SERVICE: &str = "Cursor Safe Storage";
#[cfg(target_os = "macos")]
const MACOS_KEYCHAIN_ACCOUNT: &str = "Cursor";

type Aes128CbcDecryptor = cbc::Decryptor<Aes128>;

pub(super) fn state_db_path() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| Error::Config("cannot resolve user home directory".into()))?;
    match std::env::consts::OS {
        "macos" => {
            Ok(home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb"))
        }
        "windows" => Ok(std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Roaming"))
            .join("Cursor/User/globalStorage/state.vscdb")),
        "linux" => Ok(std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("Cursor/User/globalStorage/state.vscdb")),
        platform => Err(Error::Config(format!(
            "Cursor account access is unsupported on {platform}"
        ))),
    }
}

/// Returns the access token of the account Cursor is currently signed in with.
pub async fn access_token() -> Result<String> {
    let path = state_db_path()?;
    if !path.exists() {
        return Err(Error::Config(
            "Cursor has not stored any account data on this machine".into(),
        ));
    }
    let stored = read_stored_token(&path).await?;
    let secret = safe_storage_secret()?;
    decrypt_token(&stored, &secret)
}

async fn read_stored_token(path: &PathBuf) -> Result<String> {
    // Read-only on purpose: opening Cursor's database read-write would take a
    // write lock while Cursor itself is running.
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true);
    let mut connection = SqliteConnection::connect_with(&options).await?;
    let stored = sqlx::query("SELECT CAST(value AS TEXT) AS value FROM ItemTable WHERE key = ?")
        .bind(ACCESS_TOKEN_KEY)
        .fetch_optional(&mut connection)
        .await?;
    connection.close().await.ok();
    let stored: String = stored
        .ok_or_else(|| Error::Config("Cursor has no signed-in account on this machine".into()))?
        .try_get("value")?;
    if stored.trim().is_empty() {
        return Err(Error::Config(
            "Cursor has no signed-in account on this machine".into(),
        ));
    }
    Ok(stored)
}

fn decrypt_token(stored: &str, secret: &str) -> Result<String> {
    let encoded = stored.trim().strip_prefix(PREFIX).ok_or_else(|| {
        Error::Config("Cursor account token is not protected by safe storage".into())
    })?;
    let blob = STANDARD
        .decode(encoded)
        .map_err(|_| Error::Config("Cursor account token is malformed".into()))?;
    let (prefix, payload) = blob
        .split_at_checked(3)
        .ok_or_else(|| Error::Config("Cursor account token is truncated".into()))?;
    if prefix != b"v10" {
        return Err(Error::Config(
            "Cursor account token uses an unsupported safe-storage revision".into(),
        ));
    }
    let key = derive_key(secret);
    let mut buffer = payload.to_vec();
    let plain = Aes128CbcDecryptor::new(&key.into(), &CHROMIUM_IV.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .map_err(|_| {
            Error::Config(
                "cannot decrypt the Cursor account token; unlock the system keychain and retry"
                    .into(),
            )
        })?;
    String::from_utf8(plain.to_vec())
        .map_err(|_| Error::Config("Cursor account token is not valid UTF-8".into()))
}

fn derive_key(secret: &str) -> [u8; 16] {
    let mut key = [0_u8; 16];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(
        secret.as_bytes(),
        CHROMIUM_SALT,
        CHROMIUM_ITERATIONS,
        &mut key,
    );
    key
}

#[cfg(target_os = "macos")]
fn safe_storage_secret() -> Result<String> {
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            MACOS_KEYCHAIN_SERVICE,
            "-a",
            MACOS_KEYCHAIN_ACCOUNT,
            "-w",
        ])
        .output()
        .map_err(|error| Error::Config(format!("cannot run the keychain helper: {error}")))?;
    if !output.status.success() {
        return Err(Error::Config(
            "cannot read the Cursor safe-storage key from the system keychain; Cursor must be signed in on this machine"
                .into(),
        ));
    }
    let secret = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if secret.is_empty() {
        return Err(Error::Config(
            "the system keychain returned an empty Cursor safe-storage key".into(),
        ));
    }
    Ok(secret)
}

#[cfg(not(target_os = "macos"))]
fn safe_storage_secret() -> Result<String> {
    Err(Error::Config(
        "importing Cursor account usage is currently supported on macOS only".into(),
    ))
}

#[cfg(test)]
mod tests {
    use aes::cipher::{BlockEncryptMut, KeyIvInit};
    use base64::{engine::general_purpose::STANDARD, Engine};

    use super::*;

    type Aes128CbcEncryptor = cbc::Encryptor<Aes128>;

    fn encrypt(token: &str, secret: &str) -> String {
        let key = derive_key(secret);
        let mut buffer = token.as_bytes().to_vec();
        let block_len = buffer.len().div_ceil(16) * 16;
        buffer.resize(block_len, 0);
        let ciphertext = Aes128CbcEncryptor::new(&key.into(), &CHROMIUM_IV.into())
            .encrypt_padded_mut::<Pkcs7>(&mut buffer, token.len())
            .unwrap()
            .to_vec();
        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(&ciphertext);
        format!("{PREFIX}{}", STANDARD.encode(blob))
    }

    #[test]
    fn round_trips_a_safe_storage_token() {
        let stored = encrypt("header.payload.signature", "keychain-secret");
        assert_eq!(
            decrypt_token(&stored, "keychain-secret").unwrap(),
            "header.payload.signature"
        );
    }

    #[test]
    fn rejects_a_token_encrypted_with_another_key() {
        let stored = encrypt("header.payload.signature", "keychain-secret");
        assert!(decrypt_token(&stored, "another-secret").is_err());
    }

    #[test]
    fn rejects_unprotected_tokens() {
        assert!(decrypt_token("plain-text-token", "keychain-secret").is_err());
    }
}
