pub mod actions;
pub mod binary_trust;
#[cfg(test)]
mod concurrency_tests;
pub mod evidence;
pub mod memory;
pub mod migrations;
pub mod no_build_confirmation;
pub mod no_tests_confirmation;
pub mod projects;
pub mod recovery;
pub mod requirement_items;
pub mod reviews;
pub mod self_check;
pub mod tasks;

pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub(crate) fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub mod workspace;
