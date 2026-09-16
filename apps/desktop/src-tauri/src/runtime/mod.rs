//! Phase 1 runtime foundation: environment/CLI detection and the safe
//! process-execution boundary that Nova's future Codex/Claude/Git
//! invocations must all go through.

pub mod ai_adapter;
pub mod detect;
pub mod detection_slot;
#[cfg(windows)]
pub mod job_object;
pub mod path_guard;
pub mod process;
pub mod types;
pub mod verdict;
#[cfg(windows)]
pub mod win_spawn;
