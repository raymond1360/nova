//! Version-bound handoff registration, not an executor or an authorization service.
pub mod contract;
pub mod policy;
pub mod store;
#[cfg(test)]
mod tests;

pub type Result<T> = std::result::Result<T, &'static str>;
