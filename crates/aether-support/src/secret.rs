//! Native at-rest protection for credentials (docs/10 §2, decision D3).
//!
//! `0x01` preserves Windows DPAPI; `0x02` (macOS) / `0x03` (Linux) prefix an
//! AES-256-GCM nonce and ciphertext. The key exists only in the native credential store.

use zeroize::Zeroizing;

use crate::KitError;

const KEY_UNREADABLE: &str = "key_unreadable";

/// Encrypts UTF-8 credentials into an opaque BLOB for storage.
///
/// # Errors
/// The same errors as [`protect`]. A failed save must leave the stored BLOB unchanged.
pub fn seal(plain: &str) -> Result<Vec<u8>, KitError> {
    protect(plain.as_bytes())
}

/// Decrypts UTF-8 credentials produced by [`seal`].
///
/// # Errors
/// The same errors as [`unprotect`], plus `needs_config{key_unreadable}` for invalid UTF-8.
pub fn open(sealed: &[u8]) -> Result<Zeroizing<String>, KitError> {
    let mut plain = unprotect(sealed)?;
    if std::str::from_utf8(&plain).is_err() {
        return Err(KitError::needs_config(KEY_UNREADABLE));
    }
    String::from_utf8(std::mem::take(&mut *plain))
        .map(Zeroizing::new)
        .map_err(|_| KitError::needs_config(KEY_UNREADABLE))
}

/// Encrypts arbitrary credential bytes using the current user's native store.
///
/// # Errors
/// `needs_config{secret_store_unavailable}` for unavailable/locked native storage or
/// failed randomness; `needs_config{key_unreadable}` for a malformed stored key;
/// `unsupported{secret_store}` on other platforms; `internal` for a failed Windows call.
/// There is no plaintext or file-key fallback.
pub fn protect(plain: &[u8]) -> Result<Vec<u8>, KitError> {
    #[cfg(windows)]
    return dpapi::protect(plain);
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    return native::protect(plain);
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = plain;
        Err(KitError::unsupported("secret_store"))
    }
}

/// Decrypts bytes produced by [`protect`], wiping the returned buffer on drop.
///
/// # Errors
/// `needs_config{key_unreadable}` for missing keys, wrong/foreign markers, tampering,
/// or another Windows user's BLOB; `needs_config{secret_store_unavailable}` for
/// unavailable/locked native storage. Native errors never include credential bytes.
pub fn unprotect(sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, KitError> {
    match sealed.split_first() {
        #[cfg(windows)]
        Some((&dpapi::MARKER, payload)) => dpapi::unprotect(payload),
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        Some((&native::MARKER, _)) => native::unprotect(sealed),
        _ => Err(KitError::needs_config(KEY_UNREADABLE)),
    }
}

/// Releases per-BLOB platform resources; none of the supported formats have any.
/// The shared native key must outlive every BLOB, including database backups.
///
/// # Errors
/// Never fails. Removing a BLOB does not delete the user's shared native key.
pub fn forget(_sealed: &[u8]) -> Result<(), KitError> {
    Ok(())
}

/// Whether this target implements native protection (not a runtime service probe).
#[must_use]
pub const fn available() -> bool {
    cfg!(any(windows, target_os = "macos", target_os = "linux"))
}

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "DPAPI FFI is the only allowed unsafe code in the workspace (docs/04 §10)"
)]
mod dpapi {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    use windows::core::PCWSTR;
    use zeroize::{Zeroize, Zeroizing};

    use super::KEY_UNREADABLE;
    use crate::KitError;

    pub(super) const MARKER: u8 = 0x01;

    pub(super) fn protect(plain: &[u8]) -> Result<Vec<u8>, KitError> {
        let input = blob(plain)?;
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: `input` points into `plain`, which outlives the call and is only read;
        // the optional pointers are null; on success DPAPI LocalAlloc's `out`, freed by `take`.
        unsafe {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        }
        .map_err(KitError::internal)?;
        Ok(take(out, |bytes| {
            let mut sealed = Vec::with_capacity(1 + bytes.len());
            sealed.push(MARKER);
            sealed.extend_from_slice(bytes);
            sealed
        }))
    }

    pub(super) fn unprotect(payload: &[u8]) -> Result<Zeroizing<Vec<u8>>, KitError> {
        let unreadable = |cause| KitError::needs_config(KEY_UNREADABLE).with_cause(cause);
        let input = blob(payload)?;
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: same contract as in `protect`; the description out-pointer is not requested.
        unsafe {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        }
        .map_err(unreadable)?;
        Ok(take(out, |bytes| Zeroizing::new(bytes.to_vec())))
    }

    fn blob(data: &[u8]) -> Result<CRYPT_INTEGER_BLOB, KitError> {
        Ok(CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(data.len()).map_err(KitError::internal)?,
            pbData: data.as_ptr().cast_mut(),
        })
    }

    /// Hands DPAPI's output to `copy`, then wipes and frees it.
    fn take<T>(out: CRYPT_INTEGER_BLOB, copy: impl FnOnce(&[u8]) -> T) -> T {
        if out.pbData.is_null() {
            return copy(&[]);
        }
        // SAFETY: after a successful DPAPI call `pbData` points to `cbData` initialized bytes
        // that this process owns exclusively until `LocalFree`.
        let bytes = unsafe { std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize) };
        let value = copy(bytes);
        bytes.zeroize();
        // SAFETY: `pbData` was LocalAlloc'ed by DPAPI and is freed exactly once,
        // after its last use.
        unsafe { LocalFree(Some(HLOCAL(out.pbData.cast()))) };
        value
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod native {
    use std::fmt::Write;
    use std::fs::{self, DirBuilder, OpenOptions};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::path::Path;

    use aes_gcm::aead::{AeadInPlace, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce, Tag};
    use sha2::{Digest, Sha256};
    use zeroize::Zeroizing;

    use super::KEY_UNREADABLE;
    use crate::KitError;

    #[cfg(target_os = "macos")]
    pub(super) const MARKER: u8 = 0x02;
    #[cfg(target_os = "linux")]
    pub(super) const MARKER: u8 = 0x03;
    const SERVICE: &str = "com.ouraihub.aether.credentials.v1";
    const HEADER: usize = 13; // marker + 96-bit nonce
    const TAG: usize = 16;

    fn unavailable() -> KitError {
        KitError::needs_config("secret_store_unavailable")
    }

    fn unreadable() -> KitError {
        KitError::needs_config(KEY_UNREADABLE)
    }

    fn account(home: &Path) -> Result<String, KitError> {
        let mut account = String::with_capacity(64);
        for byte in Sha256::digest(home.as_os_str().as_bytes()) {
            write!(account, "{byte:02x}").map_err(|_| unavailable())?;
        }
        Ok(account)
    }

    fn master_key(create: bool) -> Result<Zeroizing<Vec<u8>>, KitError> {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(unavailable)?;
        let home = fs::canonicalize(home).map_err(|_| unavailable())?;
        let directory = home.join(".aether");
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)
            .map_err(|_| unavailable())?;
        // This file holds no secrets. Keep its inode: removing it would split concurrent locks.
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(directory.join("credential-v1.lock"))
            .map_err(|_| unavailable())?;
        // Serialize lookup + creation across threads, processes and host applications.
        lock.lock().map_err(|_| unavailable())?;
        // Use the compiled native backend directly, never an installable mock default.
        let credential = keyring::default::default_credential_builder()
            .build(None, SERVICE, &account(&home)?)
            .map_err(|_| unavailable())?;
        let entry = keyring::Entry::new_with_credential(credential);
        let key = match entry.get_secret() {
            Ok(key) => Zeroizing::new(key),
            Err(keyring::Error::NoEntry) if create => {
                let mut key = Zeroizing::new(vec![0_u8; 32]);
                getrandom::fill(&mut key).map_err(|_| unavailable())?;
                entry.set_secret(&key).map_err(|_| unavailable())?;
                key
            }
            Err(keyring::Error::NoEntry) => return Err(unreadable()),
            Err(_) => return Err(unavailable()),
        };
        if key.len() != 32 {
            return Err(unreadable());
        }
        Ok(key)
    }

    pub(super) fn protect(plain: &[u8]) -> Result<Vec<u8>, KitError> {
        let key = master_key(true)?;
        let mut nonce = [0_u8; 12];
        getrandom::fill(&mut nonce).map_err(|_| unavailable())?;
        encrypt(&key, nonce, plain)
    }

    pub(super) fn unprotect(sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, KitError> {
        // Reject malformed data before contacting the credential store.
        if sealed.len() < HEADER + TAG || sealed[0] != MARKER {
            return Err(unreadable());
        }
        decrypt(&master_key(false)?, sealed)
    }

    fn encrypt(key: &[u8], nonce: [u8; 12], plain: &[u8]) -> Result<Vec<u8>, KitError> {
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| unreadable())?;
        let mut sealed = Zeroizing::new(Vec::with_capacity(HEADER + plain.len() + TAG));
        sealed.push(MARKER);
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(plain);
        let tag = cipher
            .encrypt_in_place_detached(&Nonce::from(nonce), &[MARKER], &mut sealed[HEADER..])
            .map_err(|_| unavailable())?;
        sealed.extend_from_slice(&tag);
        Ok(std::mem::take(&mut *sealed))
    }

    fn decrypt(key: &[u8], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, KitError> {
        if sealed.len() < HEADER + TAG || sealed[0] != MARKER {
            return Err(unreadable());
        }
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| unreadable())?;
        let nonce = <[u8; 12]>::try_from(&sealed[1..HEADER]).map_err(|_| unreadable())?;
        let end = sealed.len() - TAG;
        let tag = <[u8; TAG]>::try_from(&sealed[end..]).map_err(|_| unreadable())?;
        let mut plain = Zeroizing::new(sealed[HEADER..end].to_vec());
        cipher
            .decrypt_in_place_detached(&Nonce::from(nonce), &[MARKER], &mut plain, &Tag::from(tag))
            .map_err(|_| unreadable())?;
        Ok(plain)
    }

    #[cfg(test)]
    mod tests {
        #![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]
        use super::*;

        #[test]
        fn deterministic_cipher_authenticates_every_byte_and_key() {
            let key = [0x42_u8; 32];
            let nonce = [0x24_u8; 12];
            for plain in [b"".as_slice(), b"\0\xffprivate credential\0".as_slice()] {
                let sealed = encrypt(&key, nonce, plain).unwrap();
                assert_eq!(sealed, encrypt(&key, nonce, plain).unwrap());
                assert_eq!(decrypt(&key, &sealed).unwrap().as_slice(), plain);
                assert_eq!(sealed.len(), HEADER + plain.len() + TAG);
                if !plain.is_empty() {
                    assert!(!sealed.windows(plain.len()).any(|bytes| bytes == plain));
                }
                assert_ne!(sealed, encrypt(&key, [0x25; 12], plain).unwrap());
                assert!(decrypt(&[0x43; 32], &sealed).is_err());
                for index in 0..sealed.len() {
                    let mut tampered = sealed.clone();
                    tampered[index] ^= 1;
                    let error = decrypt(&key, &tampered).unwrap_err();
                    assert_eq!(error.details().reason(), Some(KEY_UNREADABLE));
                    assert!(!format!("{error:?}").contains("private credential"));
                }
                for end in 0..sealed.len() {
                    assert!(decrypt(&key, &sealed[..end]).is_err());
                }
                let mut extended = sealed;
                extended.push(0);
                assert!(decrypt(&key, &extended).is_err());
            }
        }

        #[test]
        fn native_identity_contains_no_credential_and_is_home_scoped() {
            let first = account(Path::new("/users/one")).unwrap();
            assert_eq!(first.len(), 64);
            assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert_eq!(first, account(Path::new("/users/one")).unwrap());
            assert_ne!(first, account(Path::new("/users/two")).unwrap());
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]
    use super::*;
    use crate::KitErrorCode;

    fn assert_unreadable(result: Result<Zeroizing<String>, KitError>) {
        let error = result.err().unwrap();
        assert_eq!(error.code(), KitErrorCode::NeedsConfig);
        assert_eq!(error.details().reason(), Some(KEY_UNREADABLE));
    }

    #[cfg(windows)]
    #[test]
    fn roundtrips_and_hides_plaintext() {
        for plain in ["", "sk-ant-api03-密钥 with spaces"] {
            let sealed = seal(plain).unwrap();
            assert_eq!(sealed[0], 0x01);
            if !plain.is_empty() {
                assert!(!sealed.windows(plain.len()).any(|w| w == plain.as_bytes()));
            }
            assert_eq!(open(&sealed).unwrap().as_str(), plain);
        }
    }

    #[cfg(windows)]
    #[test]
    fn bytes_preserve_dpapi_marker_and_string_compatibility() {
        let bytes = b"\0\xffopaque credential\0";
        let sealed = protect(bytes).unwrap();
        assert_eq!(sealed[0], 0x01);
        assert_eq!(unprotect(&sealed).unwrap().as_slice(), bytes);
        assert_unreadable(open(&sealed));
        assert_eq!(
            unprotect(&seal("existing").unwrap()).unwrap().as_slice(),
            b"existing"
        );
        forget(&sealed).unwrap();
        assert_eq!(unprotect(&sealed).unwrap().as_slice(), bytes);
    }

    #[cfg(windows)]
    #[test]
    fn tampered_byte_is_unreadable() {
        let sealed = seal("sk-test-1234567890").unwrap();
        for index in [1, sealed.len() / 2, sealed.len() - 1] {
            let mut tampered = sealed.clone();
            tampered[index] ^= 0x01;
            assert_unreadable(open(&tampered));
        }
    }

    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    #[test]
    fn seal_is_unsupported_on_other_targets() {
        assert_eq!(seal("k").err().unwrap().code(), KitErrorCode::Unsupported);
        assert!(!available());
    }

    #[test]
    fn unknown_or_foreign_marker_is_unreadable() {
        assert_unreadable(open(&[]));
        assert_unreadable(open(&[0x00, 1, 2, 3]));
        assert_unreadable(open(&[0x02; 17]));
        assert_unreadable(open(&[0xFF]));
    }
}
