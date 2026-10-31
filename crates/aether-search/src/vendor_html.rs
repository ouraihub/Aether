use super::*;

pub(crate) fn page(
    provider: &str,
    value: &Value,
    url: &str,
) -> Result<SearchPage, KitError> {
    let data = if provider == "querit" {
        value
            .pointer("/results")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .or_else(|| {
                value
                    .pointer("/data")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
            })
            .unwrap_or(value)
    } else {
        value.get("data").unwrap_or(value)
    };
    let meta = data.get("metadata").unwrap_or(data);
    let title = field(meta, &["title"]);
    let title = if title.is_empty() {
        field(data, &["title"])
    } else {
        title
    };
    let content = field(data, &["markdown", "content", "text"]);
    if content.trim().is_empty() {
        return Err(upstream("webpage has no content"));
    }
    let source = field(meta, &["sourceURL", "sourceUrl", "url", "link"]);
    let source = if source.is_empty() {
        field(data, &["url", "link"])
    } else {
        source
    };
    let source = if source.is_empty() { url } else { &source };
    Ok(SearchPage {
        title: clip(if title.is_empty() { source } else { &title }, 500),
        url: source.to_owned(),
        content: clip(&content, 24000),
    })
}

pub(crate) fn html(body: &str, url: &str) -> Result<SearchPage, KitError> {
    let lower = body.to_ascii_lowercase();
    let title = lower
        .find("<title")
        .and_then(|i| lower[i..].find('>').map(|e| i + e + 1))
        .and_then(|i| {
            lower[i..]
                .find("</title>")
                .map(|e| body[i..i + e].trim().to_owned())
        })
        .unwrap_or_else(|| url.to_owned());
    let mut text = String::new();
    let mut at = 0;
    while at < body.len() {
        if body.as_bytes()[at] == b'<' {
            let end = body[at..]
                .find('>')
                .map(|n| at + n + 1)
                .unwrap_or(body.len());
            let tag = lower[at..end].trim_start_matches('<').trim();
            let noise = ["script", "style", "noscript", "head"]
                .into_iter()
                .find(|name| {
                    tag.starts_with(name)
                        && tag.as_bytes().get(name.len()).is_none_or(|b| {
                            b.is_ascii_whitespace() || *b == b'>'
                        })
                });
            if let Some(name) = noise
                && let Some(close) = lower[end..].find(&format!("</{name}>"))
            {
                at = end + close + name.len() + 3;
                continue;
            }
            at = end;
            text.push(' ');
        } else {
            let Some(ch) = body[at..].chars().next() else {
                break;
            };
            text.push(ch);
            at += ch.len_utf8();
        }
    }
    let content = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    if content.is_empty() {
        return Err(upstream("webpage has no content"));
    }
    Ok(SearchPage {
        title: clip(&title, 500),
        url: url.to_owned(),
        content: clip(&content, 24000),
    })
}
