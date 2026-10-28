//! Native application services, independent of UI and transport adapters.

pub mod contract;
#[cfg(feature = "conversations")]
pub mod conversations;
#[cfg(feature = "runs")]
mod host;
#[cfg(feature = "conversations")]
mod prepare;
#[cfg(feature = "runs")]
mod service;
#[cfg(all(feature = "runs", feature = "skills"))]
mod skill_authoring;
#[cfg(feature = "runs")]
mod tools;

#[cfg(feature = "runs")]
pub use aether_chat::{EndStatus, RunEvent};
pub use aether_support::KitError;
#[cfg(feature = "runs")]
pub use aether_support::net::AddressPolicy;
pub use contract::*;
#[cfg(feature = "runs")]
pub use host::*;
#[cfg(feature = "runs")]
pub use service::{EventSink, RunConfig, RunContext, RunDependencies, RunOwner, RunService};
#[cfg(all(feature = "runs", feature = "skills"))]
pub use skill_authoring::SkillAuthoringService;

#[cfg(all(test, feature = "runs"))]
mod tests;
