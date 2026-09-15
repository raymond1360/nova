//! Windows Job Object wrapper providing real process-tree containment.
//!
//! `std::process::Child::kill()` only terminates the one direct child.
//! Real Git/Codex/Claude invocations can spawn further child processes of
//! their own; killing only the direct child can leave those descendants
//! running after Nova reports `Cancelled`/`TimedOut`. A Windows Job Object
//! with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` fixes this: every process a
//! job member spawns automatically becomes a member of the same job too
//! (unless it explicitly opts out with `CREATE_BREAKAWAY_FROM_JOB`, which
//! Nova never requests), so terminating the job — or the job handle being
//! closed because Nova exited/panicked — takes the whole tree down.
//!
//! Windows-only by construction (`#[cfg(windows)]`); see `process.rs` for
//! the non-Windows fallback, which keeps single-child `kill()` semantics
//! rather than claiming a containment guarantee it cannot provide.

use std::io;
use std::mem::size_of;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// An owned Windows Job Object handle with kill-on-close semantics already
/// configured. Dropping this value closes the handle, which — per
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` — terminates every process still
/// assigned to it, so an unexpected Nova exit cannot leave the job's
/// processes running.
pub struct JobObject {
    handle: HANDLE,
}

// SAFETY: a Win32 HANDLE is an opaque kernel object reference. It is safe
// to move between threads: every method here only reads `self.handle` and
// passes it to a thread-safe Win32 API; no interior mutation occurs.
unsafe impl Send for JobObject {}

impl JobObject {
    /// Create a new, unnamed job object configured so that closing its
    /// handle (including on process exit/panic) terminates every process
    /// still assigned to it.
    pub fn create() -> io::Result<Self> {
        // SAFETY: both arguments may legally be null (default security
        // attributes, unnamed job object) per the documented contract of
        // `CreateJobObjectW`.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = JobObject { handle };

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        // SAFETY: `job.handle` is the handle just created above and is
        // still valid; `info` is a correctly sized, zero-initialized
        // struct with only the documented `LimitFlags` field populated.
        let ok = unsafe {
            SetInformationJobObject(
                job.handle,
                JobObjectExtendedLimitInformation,
                &info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const core::ffi::c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            // `job` still gets dropped normally here, which closes the
            // handle (CloseHandle on a job object with no assigned
            // processes yet is always safe and does not terminate
            // anything).
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Assign the process behind `process_handle` to this job. Every
    /// process it spawns afterward automatically becomes a member of the
    /// same job too. Takes a raw `HANDLE` (rather than a `std::process`
    /// type) so this works uniformly for a suspended
    /// [`super::win_spawn::SuspendedChild`] — the whole point of this
    /// remediation is assigning *before* the process has run any code, so
    /// there is no `std::process::Child` in that path at all.
    pub fn assign_handle(&self, process_handle: HANDLE) -> io::Result<()> {
        // SAFETY: `self.handle` is a valid job object handle; the caller
        // guarantees `process_handle` is a valid, still-open process
        // handle for the duration of this call.
        let ok = unsafe { AssignProcessToJobObject(self.handle, process_handle) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Terminate every process currently assigned to this job — the whole
    /// tree, not just one direct child.
    pub fn terminate(&self) -> io::Result<()> {
        // SAFETY: `self.handle` is a valid job object handle.
        let ok = unsafe { TerminateJobObject(self.handle, 1) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        // SAFETY: `self.handle` is a handle owned exclusively by this
        // struct and is not accessed after this call.
        unsafe {
            CloseHandle(self.handle);
        }
    }
}
