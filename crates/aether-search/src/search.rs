//! Independent search execution from an explicit configuration snapshot.
use crate::{
    SearchCap, SearchResult,
    execution::{self, Output, ResolvedConfig},
    policy,
};
use aether_support::KitError;

pub(crate) fn validate_query(query: &str) -> Result<&str, KitError> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > 4096 {
        return Err(KitError::bad_request("query", "invalid"));
    }
    Ok(query)
}

/// Searches once with the supplied key order, without configuration storage.
/// # Errors
/// Rejects invalid input/configuration and reports network, deadline or
/// upstream errors.
pub async fn search(
    config: ResolvedConfig,
    query: &str,
) -> Result<SearchResult, KitError> {
    let query = validate_query(query)?;
    let Output::Hits(mut hits) =
        execution::execute(&config, SearchCap::Search, query).await?
    else {
        return Err(KitError::internal(std::io::Error::other(
            "unexpected search output",
        )));
    };
    policy::process_hits(&mut hits, &config.preferences);
    Ok(SearchResult {
        query: query.into(),
        provider: config.provider,
        hits,
    })
}
