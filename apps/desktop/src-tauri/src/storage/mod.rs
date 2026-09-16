pub mod actions;
#[cfg(test)]
mod concurrency_tests;
pub mod evidence;
pub mod memory;
pub mod migrations;
pub mod projects;
pub mod recovery;
pub mod tasks;

pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub(crate) fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
