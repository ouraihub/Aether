//! Opaque at-rest encryption for credentials (docs/10 §2, decision D3).
//!
//! A sealed BLOB is one format byte followed by a platform payload:
//! `0x01` = Windows DPAPI `CryptProtectData` output (current-user scope).

use zeroize::Zeroizing;

use crate::KitError;

const KEY_UNREADABLE: &str = "key_unreadable";

/// Encrypts `plain` into an opaque BLOB for storage.
///
/// # Errors
/// `unsupported` on platforms without a secret store yet; `internal` if the OS call fails.
/// Callers fail the whole save so the stored ciphertext stays unchanged.
pub fn seal(plain: &str) -> Result<Vec<u8>, KitError> {
    #[cfg(windows)]
    return dpapi::protect(plain.as_bytes());
    #[cfg(not(windows))]
    {
        let _ = plain;
        Err(KitError::unsupported("secret_store"))
    }
}

/// Decrypts a BLOB produced by [`seal`].
///
/// # Errors
/// `needs_config{reason:"key_unreadable"}` for an unknown or foreign-platform marker, tampered
/// data, or a BLOB sealed by another Windows user; the UI asks the user to re-enter the key.
pub fn open(sealed: &[u8]) -> Result<Zeroizing<String>, KitError> {
    match sealed.split_first() {
        #[cfg(windows)]
        Some((&dpapi::MARKER, payload)) => dpapi::unprotect(payload),
        _ => Err(KitError::needs_config(KEY_UNREADABLE)),
    }
}

/// Removes platform-side leftovers of a BLOB; call before overwriting or deleting it.
/// DPAPI keeps nothing outside the BLOB, so this is a no-op on Windows.
///
/// # Errors
/// Never on Windows.
pub fn forget(_sealed: &[u8]) -> Result<(), KitError> {
    Ok(())
}

/// Whether [`seal`] works on this platform.
#[must_use]
pub const fn available() -> bool {
    cfg!(windows)
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

    pub(super) fn unprotect(payload: &[u8]) -> Result<Zeroizing<String>, KitError> {
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
        take(out, |bytes| {
            std::str::from_utf8(bytes).map(|plain| Zeroizing::new(plain.to_owned()))
        })
        .map_err(|e| KitError::needs_config(KEY_UNREADABLE).with_cause(e))
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
        // SAFETY: `pbData` was LocalAlloc'ed by DPAPI and is freed exactly once, after its last use.
        unsafe { LocalFree(Some(HLOCAL(out.pbData.cast()))) };
        value
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
    fn tampered_byte_is_unreadable() {
        let sealed = seal("sk-test-1234567890").unwrap();
        for index in [1, sealed.len() / 2, sealed.len() - 1] {
            let mut tampered = sealed.clone();
            tampered[index] ^= 0x01;
            assert_unreadable(open(&tampered));
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn seal_is_unsupported_off_windows() {
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
