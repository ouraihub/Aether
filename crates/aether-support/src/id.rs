//! Time-ordered ids (docs/10 §4).

use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::KitError;

/// Crockford base32, lower case: no i, l, o, u.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// A 26-character id: 10 characters of millisecond time followed by 16 random
/// characters, so ids sort by creation time.
///
/// # Errors
/// `internal` when the OS random source fails.
pub fn new_id() -> Result<String, KitError> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random)
        .map_err(|error| KitError::internal(RandomError(error.to_string())))?;
    Ok(encode(now_millis(), &random))
}

/// An 8-character id used to correlate a user-visible error with its log line.
/// Falls back to time when the random source fails: it only has to be distinct, not secret.
#[must_use]
pub fn short_id() -> String {
    let mut random = [0_u8; 8];
    if getrandom::fill(&mut random).is_err() {
        random = now_millis().to_le_bytes();
    }
    random
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(byte & 31)]))
        .collect()
}

/// Whether `value` has the shape produced by [`new_id`].
#[must_use]
pub fn is_valid_id(value: &str) -> bool {
    value.len() == 26 && value.bytes().all(|byte| ALPHABET.contains(&byte))
}

fn encode(millis: u64, random: &[u8; 16]) -> String {
    let mut out = String::with_capacity(26);
    for shift in (0..10).rev() {
        let index = (millis >> (shift * 5)) & 31;
        out.push(char::from(ALPHABET[usize::try_from(index).unwrap_or(0)]));
    }
    out.extend(
        random
            .iter()
            .map(|byte| char::from(ALPHABET[usize::from(byte & 31)])),
    );
    out
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[derive(Debug, thiserror::Error)]
#[error("OS random source failed: {0}")]
struct RandomError(String);

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]
    use super::*;

    #[test]
    fn encodes_time_first_so_ids_sort_by_time() {
        let zero = [0_u8; 16];
        assert_eq!(encode(1, &zero), "00000000010000000000000000");
        assert!(encode(1_700_000_000_000, &zero) < encode(1_700_000_000_001, &zero));
        // Random bytes never change the time prefix ordering.
        assert!(encode(5, &[31; 16]) < encode(6, &zero));
    }

    #[test]
    fn generated_ids_are_valid_and_distinct() {
        let a = new_id().unwrap();
        let b = new_id().unwrap();
        assert!(is_valid_id(&a) && is_valid_id(&b));
        assert_ne!(a, b);
        assert!(
            !is_valid_id("0000000001000000000000000O"),
            "upper case and 'o' are not in the alphabet"
        );
        assert!(!is_valid_id("0000000001"));
        assert_eq!(short_id().len(), 8);
    }
}
