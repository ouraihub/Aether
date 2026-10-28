//! Effective action resolution, capability snapshots and pure reservation validation.

#[cfg(any(feature = "runs", test))]
pub(crate) mod history;
#[cfg(any(feature = "runs", test))]
pub(crate) use history::trim_history;
#[cfg(feature = "runs")]
mod runs;
#[cfg(feature = "runs")]
pub(crate) use runs::*;
