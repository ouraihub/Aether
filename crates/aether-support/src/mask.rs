//! Masking and secret detection for display and logs (docs/10 §3, docs/04 §2.1).

use url::Url;

/// Query parameter names whose values are treated as credentials (compared lower-case).
const SECRET_PARAMS: &[&str] = &[
    "key",
    "api_key",
    "apikey",
    "token",
    "access_token",
    "ak",
    "sk",
    "secret",
    "client_secret",
    "password",
    "signature",
];

/// Prefixes of well-known credential formats. A token must also be long enough
/// to be a key (`MIN_TOKEN_LEN`), so ordinary words that share a prefix pass.
const SECRET_PREFIXES: &[&str] = &[
    "sk-",
    "ghp_",
    "gho_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "AKIA",
    "AIza",
    "glpat-",
];
const MIN_TOKEN_LEN: usize = 20;
const MASK: &str = "***";

/// `sk-abcdefghijkl` → `sk-a*******ijkl`. Values of 8 characters or fewer are fully masked.
#[must_use]
pub fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len().max(4));
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}{}{tail}", "*".repeat(chars.len() - 8))
}

/// Removes userinfo and masks credential-looking query values. Unparseable input is
/// returned with every secret-looking token masked, never verbatim.
#[must_use]
pub fn mask_url(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw) else {
        return redact_secrets(raw);
    };
    // Setting userinfo can only fail for cannot-be-a-base URLs, which carry none.
    let _ = url.set_username("");
    let _ = url.set_password(None);
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(name, value)| {
            let secret = SECRET_PARAMS.contains(&name.to_ascii_lowercase().as_str())
                || is_secret_token(&value);
            (
                name.into_owned(),
                if secret {
                    MASK.to_owned()
                } else {
                    value.into_owned()
                },
            )
        })
        .collect();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(pairs);
    }
    url.into()
}

/// Location of the first credential-looking token: (1-based line, byte offset in the line).
/// Detection is a heuristic aid; it cannot promise to find every secret.
#[must_use]
pub fn find_secret(text: &str) -> Option<(usize, usize)> {
    for (index, line) in text.lines().enumerate() {
        if line.contains("-----BEGIN") && line.contains("PRIVATE KEY") {
            return Some((index + 1, line.find("-----BEGIN").unwrap_or(0)));
        }
        if let Some(offset) = bearer_offset(line) {
            return Some((index + 1, offset));
        }
        let mut offset = 0;
        for token in line.split(|c: char| !is_token_char(c)) {
            if is_secret_token(token) {
                return Some((index + 1, offset));
            }
            offset += token.len() + 1;
        }
    }
    None
}

/// Whether `text` contains a credential-looking token anywhere.
#[must_use]
pub fn looks_like_secret(text: &str) -> bool {
    find_secret(text).is_some()
}

/// Replaces each known plaintext secret with `***`. Empty entries are ignored.
#[must_use]
pub fn redact(text: &str, known: &[&str]) -> String {
    known
        .iter()
        .map(|secret| secret.trim())
        .filter(|secret| !secret.is_empty())
        .fold(text.to_owned(), |acc, secret| acc.replace(secret, MASK))
}

/// Replaces every credential-looking token (and bearer values) with `***`.
#[must_use]
pub fn redact_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut token = String::new();
    // Last completed token, to recognise the value in "Bearer <value>".
    let mut previous_was_bearer = false;
    let mut emit = |token: &mut String, out: &mut String| {
        if token.is_empty() {
            return;
        }
        let secret =
            is_secret_token(token) || (previous_was_bearer && token.len() >= MIN_TOKEN_LEN);
        out.push_str(if secret { MASK } else { token.as_str() });
        previous_was_bearer = token.eq_ignore_ascii_case("bearer");
        token.clear();
    };
    for c in text.chars() {
        if is_token_char(c) {
            token.push(c);
        } else {
            emit(&mut token, &mut out);
            out.push(c);
        }
    }
    emit(&mut token, &mut out);
    out
}

fn bearer_offset(line: &str) -> Option<usize> {
    let lower = line.to_ascii_lowercase();
    let at = lower.find("bearer ")?;
    let value: String = line[at + 7..]
        .chars()
        .take_while(|c| is_token_char(*c))
        .collect();
    (value.len() >= MIN_TOKEN_LEN).then_some(at)
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '+' | '=')
}

fn is_secret_token(token: &str) -> bool {
    token.len() >= MIN_TOKEN_LEN
        && SECRET_PREFIXES
            .iter()
            .any(|prefix| token.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "sk-abcdefghijklmnopqrstuvwx";

    #[test]
    fn mask_secret_keeps_only_four_chars_each_side() {
        assert_eq!(mask_secret("sk-abcdefghijkl"), "sk-a*******ijkl");
        assert_eq!(mask_secret("short"), "*****");
        assert_eq!(
            mask_secret("ab"),
            "****",
            "length of very short secrets is not revealed"
        );
        assert!(!mask_secret("sk-0123456789secret-tail").contains("secret"));
    }

    #[test]
    fn mask_url_drops_userinfo_and_masks_credential_params() {
        let masked = mask_url("https://user:pw@mcp.example.com/mcp?ak=abc123&page=2");
        assert_eq!(masked, "https://mcp.example.com/mcp?ak=***&page=2");
        let masked = mask_url(&format!("https://x.example/v1?q={KEY}"));
        assert!(!masked.contains(KEY), "{masked}");
        assert!(Url::parse(&masked).is_ok(), "masked URL stays parseable");
        assert_eq!(
            mask_url("https://api.example.com/v1"),
            "https://api.example.com/v1"
        );
    }

    #[test]
    fn find_secret_reports_line_and_ignores_ordinary_text() {
        let text = format!("# Title\nuse the key {KEY} here\n");
        assert_eq!(find_secret(&text), Some((2, 12)));
        assert!(looks_like_secret(
            "Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123"
        ));
        assert!(looks_like_secret("-----BEGIN RSA PRIVATE KEY-----"));
        assert!(!looks_like_secret(
            "sk-short and a skill-based plan with ghp_ text"
        ));
        assert!(!looks_like_secret("Bearer tokens are explained below"));
    }

    #[test]
    fn redaction_removes_known_and_pattern_secrets() {
        assert_eq!(
            redact("key=s3cr3t-value end", &["s3cr3t-value", ""]),
            "key=*** end"
        );
        let body =
            format!("invalid key {KEY}; header Bearer abcdefghijklmnopqrstuvwxyz0123 rejected");
        let out = redact_secrets(&body);
        assert_eq!(out, "invalid key ***; header Bearer *** rejected");
    }
}
