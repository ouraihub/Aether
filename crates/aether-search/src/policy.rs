//! Pure preference validation, URL exclusions and Unicode result processing.
use crate::{Compression, SearchHit, SearchPreferences};
use aether_support::KitError;

fn normalized_rule(rule: &str) -> Result<(String, bool), KitError> {
    let invalid = || KitError::bad_request("blacklist", "invalid_rule");
    if rule.is_empty()
        || rule.chars().count() > 256
        || rule.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(invalid());
    }
    let (scheme, rest) = match rule.split_once("://") {
        Some((scheme, rest))
            if matches!(
                scheme.to_ascii_lowercase().as_str(),
                "http" | "https" | "*"
            ) =>
        {
            (Some(scheme.to_ascii_lowercase()), rest)
        }
        Some(_) => return Err(invalid()),
        None => (None, rule),
    };
    let end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..end];
    if scheme.is_none() && end != rest.len() {
        return Err(invalid());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port))
            if scheme.is_some()
                && port.parse::<u16>().is_ok_and(|p| p != 0) =>
        {
            (host, Some(port))
        }
        Some(_) => return Err(invalid()),
        None => (authority, None),
    };
    let host = host.trim_end_matches('.');
    if host.len() > 253
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| {
                        b.is_ascii_alphanumeric()
                            || matches!(b, b'-' | b'*' | b'?')
                    })
        })
    {
        return Err(invalid());
    }
    let host = host.to_ascii_lowercase();
    if let Some(scheme) = scheme {
        if rest[end..].contains(['\\', '[', ']', '{', '}', '^', '|', '#']) {
            return Err(invalid());
        }
        Ok((
            format!(
                "{scheme}://{host}{}{}",
                port.map(|p| format!(":{p}")).unwrap_or_default(),
                if end == rest.len() { "/" } else { &rest[end..] }
            ),
            true,
        ))
    } else {
        Ok((host, false))
    }
}
/// Validates the entire preference document without modifying it.
/// # Errors
/// Rejects invalid bounds and blacklist rules.
pub fn validate_preferences(prefs: &SearchPreferences) -> Result<(), KitError> {
    if !(1..=50).contains(&prefs.max_results) {
        return Err(KitError::bad_request("maxResults", "out_of_range"));
    }
    if !(100..=20000).contains(&prefs.cutoff_limit) {
        return Err(KitError::bad_request("cutoffLimit", "out_of_range"));
    }
    if prefs.blacklist.len() > 32 {
        return Err(KitError::bad_request("blacklist", "too_many"));
    }
    for rule in &prefs.blacklist {
        normalized_rule(rule)?;
    }
    Ok(())
}
fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut i, mut j, mut star, mut retry) = (0, 0, None, 0);
    while j < t.len() {
        if i < p.len() && (p[i] == b'?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star = Some(i);
            i += 1;
            retry = j;
        } else if let Some(s) = star {
            retry += 1;
            j = retry;
            i = s + 1;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == b'*' {
        i += 1;
    }
    i == p.len()
}
fn blocked_url(url: &reqwest::Url, prefs: &SearchPreferences) -> bool {
    let host = url.host_str().unwrap_or_default().trim_end_matches('.');
    let mut normalized = url.clone();
    let _ = normalized.set_host(Some(host));
    prefs.blacklist.iter().any(|raw| {
        let Ok((rule, full)) = normalized_rule(raw) else {
            return true;
        };
        if full {
            glob_match(&rule, normalized.as_str())
        } else if rule.contains(['*', '?']) {
            glob_match(&rule, host)
        } else {
            host == rule
                || host.strip_suffix(&rule).is_some_and(|s| s.ends_with('.'))
        }
    })
}
/// Rejects a URL that matches a configured exclusion rule.
/// # Errors
/// Reports malformed URLs or a blacklisted source.
pub fn check_blacklist(
    url: &str,
    prefs: &SearchPreferences,
) -> Result<(), KitError> {
    let url = reqwest::Url::parse(url)
        .map_err(|_| KitError::bad_request("url", "invalid"))?;
    if blocked_url(&url, prefs) {
        return Err(KitError::bad_request("url", "blacklisted"));
    }
    Ok(())
}
/// Applies optional Unicode-scalar truncation in place.
pub fn cutoff(text: &mut String, prefs: &SearchPreferences) {
    if prefs.compression == Compression::Cutoff
        && let Some((end, _)) = text.char_indices().nth(prefs.cutoff_limit)
    {
        text.truncate(end);
    }
}
/// Removes invalid/excluded sources, then truncates summaries and result count.
pub fn process_hits(hits: &mut Vec<SearchHit>, prefs: &SearchPreferences) {
    hits.retain(|hit| {
        reqwest::Url::parse(&hit.url).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && !blocked_url(&url, prefs)
        })
    });
    hits.truncate(usize::from(prefs.max_results));
    for hit in hits {
        cutoff(&mut hit.snippet, prefs);
    }
}
