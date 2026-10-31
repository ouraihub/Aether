//! Independent search settings and explicit, bounded search and webpage
//! execution.
pub mod dto;
pub mod execution;
pub mod fetch;
pub mod policy;
pub mod search;
mod store;
mod vendor;

pub use aether_support::net::AddressPolicy;
pub use dto::{
    CheckResult, Compression, SearchCap, SearchHit, SearchKey, SearchPage,
    SearchPreferences, SearchProvider, SearchResult, SearchSave, SearchSettings,
};
pub use store::Store;
