//! Independent webpage reading from an explicit configuration snapshot.
use crate::{
    SearchCap, SearchPage,
    execution::{self, Output, ResolvedConfig},
    policy,
};
use aether_support::{KitError, net};

pub(crate) fn validate_url<'a>(
    url: &'a str,
    policy: &net::AddressPolicy,
) -> Result<&'a str, KitError> {
    if url.len() > 8192 {
        return Err(KitError::bad_request("url", "too_long"));
    }
    let url = url.trim();
    net::check_url(url, policy)?;
    Ok(url)
}

/// Reads one page with the supplied key order, without configuration storage.
/// # Errors
/// Rejects invalid input/configuration, unsafe sources and network/upstream failures.
pub async fn fetch(config: ResolvedConfig, url: &str) -> Result<SearchPage, KitError> {
    let url = validate_url(url, &config.address_policy)?;
    let Output::Page(mut page) = execution::execute(&config, SearchCap::Fetch, url).await? else {
        return Err(KitError::internal(std::io::Error::other(
            "unexpected fetch output",
        )));
    };
    net::check_url(&page.url, &config.address_policy)?;
    policy::check_blacklist(&page.url, &config.preferences)?;
    policy::cutoff(&mut page.content, &config.preferences);
    Ok(page)
}
