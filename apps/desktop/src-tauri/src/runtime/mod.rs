//! Phase 1 runtime foundation: environment/CLI detection and the safe
//! process-execution boundary that Nova's future Codex/Claude/Git
//! invocations must all go through.

pub mod detect;
pub mod path_guard;
pub mod process;
pub mod types;
