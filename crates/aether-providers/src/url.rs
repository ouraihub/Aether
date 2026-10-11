//! Base URL normalisation (docs/11 §4, pitfall 3).

/// Strips trailing `/` and appends `/v1` unless already there. Idempotent.
#[must_use]
pub fn api_base(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/v1")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_v1_once() {
        for (input, expected) in [
            ("https://api.example.com", "https://api.example.com/v1"),
            ("https://api.example.com/", "https://api.example.com/v1"),
            ("https://api.example.com/v1", "https://api.example.com/v1"),
            ("https://api.example.com/v1/", "https://api.example.com/v1"),
            (
                "https://x.example.com/api/v1",
                "https://x.example.com/api/v1",
            ),
        ] {
            assert_eq!(api_base(input), expected, "{input}");
            assert_eq!(api_base(&api_base(input)), expected, "idempotent {input}");
        }
    }
}
