use super::*;

pub(crate) async fn fetch(
    policy: &net::AddressPolicy,
    runtime: &Attempt<'_>,
    url: &str,
    prefs: &crate::SearchPreferences,
) -> Result<SearchPage, KitError> {
    let target = net::check_url(url, policy)?;
    crate::policy::check_blacklist(target.as_str(), prefs)?;
    let client = net::client(policy.clone(), Duration::from_secs(10))?;
    let base = runtime.base.trim_end_matches('/');
    if runtime.provider == "fetch" {
        let mut current = target;
        for count in 0..=5 {
            let response = client
                .get(current.clone())
                .send()
                .await
                .map_err(network)?;
            if response.status().is_redirection() {
                if count == 5 {
                    return Err(upstream("too many redirects"));
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| upstream("redirect missing location"))?;
                let next = current.join(location).map_err(|_| {
                    KitError::bad_request("url", "invalid_redirect")
                })?;
                current = net::check_url(next.as_str(), policy)?;
                crate::policy::check_blacklist(current.as_str(), prefs)?;
                continue;
            }
            let body = read_response(response, None).await?;
            return vendor_html::html(&body, current.as_str());
        }
        return Err(upstream("too many redirects"));
    }
    net::check_target(url, policy).await?;
    let endpoint = match runtime.provider {
        "jina" => format!("{base}/{url}"),
        "querit" => format!("{base}/v1/contents"),
        "firecrawl" => format!("{base}/v2/scrape"),
        _ => return Err(KitError::bad_request("provider", "unsupported")),
    };
    let checked = net::check_url(&endpoint, policy)?;
    let req = match runtime.provider {
        "jina" => client
            .get(checked)
            .headers(auth(runtime, "bearer")?)
            .header("x-retain-images", "none"),
        "querit" => client
            .post(checked)
            .headers(auth(runtime, "bearer")?)
            .json(&json!({"urls":[url],"format":"markdown","extrasMeta":true})),
        "firecrawl" => client
            .post(checked)
            .headers(auth(runtime, "bearer")?)
            .json(&json!({"url":url,"formats":["markdown"]})),
        _ => return Err(KitError::bad_request("provider", "unsupported")),
    };
    let body = send(req, runtime).await?;
    let value = json_body(&body)?;
    provider_error(runtime.provider, &value)?;
    vendor_html::page(runtime.provider, &value, url)
}
