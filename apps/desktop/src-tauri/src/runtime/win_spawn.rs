//! Race-free Windows process creation: create suspended, assign to a Job
//! Object, *then* resume — so the target's own code never runs before it is
//! contained. See `job_object.rs` for the Job Object itself and
//! `process.rs` for how the two are wired together.
//!
//! The previous implementation used `std::process::Command::spawn()`
//! followed by `AssignProcessToJobObject`, which let the child run —
//! including spawning its own descendants — for a window before joining
//! the job. `std::process::Child` exposes no way to create a process
//! suspended-with-a-resumable-thread-handle (there is no public API for the
//! initial thread), so closing that race requires calling `CreateProcessW`
//! directly with `CREATE_SUSPENDED` and keeping the returned thread handle
//! ourselves, rather than guessing at a thread to resume via process-wide
//! thread enumeration (fragile, and explicitly not an acceptable substitute
//! per this remediation's own requirements).
//!
//! Still exposes a `program: &str` + `args: &[String]` interface — never a
//! shell command string. Argument quoting follows the documented MSVC
//! C runtime / `CommandLineToArgvW` rules (the same rules `std::process`
//! itself implements internally on Windows), and the executable is
//! resolved against `PATH` only (never the current directory), so a
//! same-named executable planted in a project directory Nova opens cannot
//! shadow the trusted one on `PATH`.

use std::ffi::OsStr;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, ResumeThread, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED, EXTENDED_STARTUPINFO_PRESENT,
    INFINITE, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

const HANDLE_FLAG_INHERIT: u32 = 1;
const WAIT_OBJECT_0: u32 = 0;

fn to_wide_null(text: &str) -> Vec<u16> {
    OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Reject `program`, `cwd`, and every element of `args` if any contains an
/// embedded NUL (`\0`/U+0000) — checked once, before any PATH resolution
/// or command-line construction, so no downstream code path can forget it
/// or accidentally see a silently-reshaped string instead.
///
/// Win32 wide strings are implicitly NUL-terminated: [`to_wide_null`]
/// appends its own terminator at the *end* of whatever it is given. An
/// embedded NUL earlier in the string would make `CreateProcessW` see a
/// *shorter* string than Nova intended — anything after the embedded NUL,
/// including content the caller believed was still part of the same
/// argument, would be silently dropped rather than rejected. A process
/// safety boundary must never let that happen invisibly: this is Nova's
/// own explicit refusal, not something Win32 discovers and reports for us.
fn validate_process_request_strings(program: &str, args: &[String], cwd: &Path) -> io::Result<()> {
    if program.as_bytes().contains(&0) {
        return Err(nul_rejection_error("可执行程序路径"));
    }
    if cwd.as_os_str().encode_wide().any(|unit| unit == 0) {
        return Err(nul_rejection_error("工作目录"));
    }
    for arg in args {
        if arg.as_bytes().contains(&0) {
            return Err(nul_rejection_error("命令行参数"));
        }
    }
    Ok(())
}

/// Builds an `InvalidInput` error that names *which kind* of string was
/// rejected but never echoes its actual (possibly sensitive) content —
/// `classify_spawn_error` in `process.rs` maps `InvalidInput` to
/// `ExecutionErrorKind::StartFailed`, and the message text below is all
/// that ever reaches `ExecutionResult` (itself still passed through
/// `redact_sensitive_text` as defense in depth).
fn nul_rejection_error(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{what}包含 Windows 无法表示的 NUL 字符，已拒绝启动"),
    )
}

/// Quote one argument following the MSVC C runtime / `CommandLineToArgvW`
/// rules, so the receiving program's own argv parser reconstructs exactly
/// this string as one argument — the same rules `std::process::Command`
/// itself implements internally on Windows.
fn quote_windows_arg(arg: &str, out: &mut String) {
    let needs_quotes = arg.is_empty()
        || arg
            .chars()
            .any(|c| c == ' ' || c == '\t' || c == '\n' || c == '\x0B' || c == '"');
    if !needs_quotes {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let chars: Vec<char> = arg.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let mut backslashes = 0usize;
        while i < chars.len() && chars[i] == '\\' {
            backslashes += 1;
            i += 1;
        }
        if i == chars.len() {
            // Trailing backslashes right before the closing quote: each
            // must become two, so the closing quote is not escaped.
            out.extend(std::iter::repeat_n('\\', backslashes * 2));
            break;
        } else if chars[i] == '"' {
            // Backslashes immediately before a literal quote: each becomes
            // two, plus one more backslash to escape the quote itself.
            out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
            out.push('"');
            i += 1;
        } else {
            // Backslashes not before a quote are literal.
            out.extend(std::iter::repeat_n('\\', backslashes));
            out.push(chars[i]);
            i += 1;
        }
    }
    out.push('"');
}

/// Build the full `lpCommandLine` string for `program` + `args`, quoting
/// each token independently. `program` is quoted the same way as any other
/// argument (it becomes `argv[0]` as the launched program sees it); the
/// actual executable that runs is controlled separately via
/// `lpApplicationName` (see [`resolve_executable`]), not by parsing this
/// string.
fn build_command_line(program: &str, args: &[String]) -> String {
    let mut out = String::new();
    quote_windows_arg(program, &mut out);
    for arg in args {
        out.push(' ');
        quote_windows_arg(arg, &mut out);
    }
    out
}

/// Resolve `program` to a full executable path using only `PATH` (and
/// `PATHEXT`) — deliberately never the current/working directory — unless
/// `program` already contains a path separator or is absolute, in which
/// case it is used as given. This mirrors the intent of `SearchPathW`'s
/// "safe process search mode" without depending on that mode being enabled
/// system-wide: a same-named executable planted in a directory Nova opens
/// (e.g. a malicious `git.exe` in a project root) must never be resolved
/// ahead of the trusted `git.exe` actually on `PATH`.
pub fn resolve_executable(program: &str) -> io::Result<PathBuf> {
    let candidate = Path::new(program);
    if program.contains('\\') || program.contains('/') || candidate.is_absolute() {
        return Ok(candidate.to_path_buf());
    }

    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let extensions: Vec<String> = pathext
        .split(';')
        .filter(|ext| !ext.is_empty())
        .map(|ext| ext.to_string())
        .collect();
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let verbatim = dir.join(program);
        if verbatim.is_file() {
            return Ok(verbatim);
        }
        for ext in &extensions {
            let with_ext = dir.join(format!("{program}{ext}"));
            if with_ext.is_file() {
                return Ok(with_ext);
            }
        }
    }
    Err(io::Error::from(io::ErrorKind::NotFound))
}

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn is_valid(&self) -> bool {
        !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if self.is_valid() {
            // SAFETY: `self.0` is a handle owned exclusively by this
            // struct and not accessed after this call.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

fn open_nul_inheritable() -> io::Result<OwnedHandle> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let name = to_wide_null("NUL");
    // SAFETY: `name` is a valid null-terminated wide string; `sa` is a
    // correctly sized, stack-local `SECURITY_ATTRIBUTES` alive for the
    // duration of this call.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &sa,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(OwnedHandle(handle))
}

struct InheritablePipe {
    /// Parent-side, non-inheritable.
    read: OwnedHandle,
    /// Child-side, inheritable — passed to `CreateProcessW` and closed in
    /// the parent right after the child is created.
    write: OwnedHandle,
}

fn create_inheritable_pipe() -> io::Result<InheritablePipe> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let mut read_handle: HANDLE = null_mut();
    let mut write_handle: HANDLE = null_mut();
    // SAFETY: both out-pointers are valid stack locals; `sa` is a
    // correctly sized, stack-local `SECURITY_ATTRIBUTES`.
    let ok = unsafe { CreatePipe(&mut read_handle, &mut write_handle, &sa, 0) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let read = OwnedHandle(read_handle);
    let write = OwnedHandle(write_handle);
    // The parent's read end must not be inherited by the child — only the
    // write end (already inheritable from `CreatePipe`'s security
    // attributes) is meant to cross into the child.
    // SAFETY: `read.0` is the just-created, still-valid read handle.
    let ok = unsafe {
        windows_sys::Win32::Foundation::SetHandleInformation(read.0, HANDLE_FLAG_INHERIT, 0)
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(InheritablePipe { read, write })
}

/// The mirror image of [`create_inheritable_pipe`]: the *read* end
/// (child-side, for stdin) is inheritable and the *write* end (parent-side)
/// is not. Used only when a caller actually wants to write content to the
/// child's stdin — the far more common `stdin: None` case keeps using
/// [`open_nul_inheritable`], unchanged.
fn create_inheritable_pipe_for_child_stdin() -> io::Result<InheritablePipe> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let mut read_handle: HANDLE = null_mut();
    let mut write_handle: HANDLE = null_mut();
    // SAFETY: both out-pointers are valid stack locals; `sa` is a
    // correctly sized, stack-local `SECURITY_ATTRIBUTES`.
    let ok = unsafe { CreatePipe(&mut read_handle, &mut write_handle, &sa, 0) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let read = OwnedHandle(read_handle);
    let write = OwnedHandle(write_handle);
    // The parent's write end must not be inherited by the child — only the
    // read end (already inheritable from `CreatePipe`'s security
    // attributes) is meant to cross into the child.
    // SAFETY: `write.0` is the just-created, still-valid write handle.
    let ok = unsafe {
        windows_sys::Win32::Foundation::SetHandleInformation(write.0, HANDLE_FLAG_INHERIT, 0)
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(InheritablePipe { read, write })
}

/// A process created suspended and (once [`resume`](Self::resume) is
/// called after Job assignment) running. Owns the process and thread
/// handles plus the parent-side ends of the stdout/stderr/stdin pipes.
pub struct SuspendedChild {
    process: OwnedHandle,
    thread: OwnedHandle,
    /// `Option`, mirroring `std::process::Child::stdout`, so a caller can
    /// `.take()` it into a reader thread while continuing to use the rest
    /// of `SuspendedChild` (`try_wait`/`wait`/`terminate`) by reference —
    /// a plain non-`Option` field would make that a partial move error.
    pub stdout: Option<std::fs::File>,
    pub stderr: Option<std::fs::File>,
    /// Parent-side write end of the child's stdin pipe. `None` when
    /// `spawn_suspended` was called with `stdin: None` (the child's stdin is
    /// then connected to `NUL`, exactly as before this field existed).
    /// `.take()`-able for the same reason as `stdout`/`stderr`, and must be
    /// dropped (closing the pipe, signaling EOF to the child) once all
    /// intended input has been written.
    pub stdin: Option<std::fs::File>,
}

/// A `WaitForSingleObject`/`GetExitCodeProcess`-derived status, shaped like
/// the subset of `std::process::ExitStatus` the rest of `process.rs` uses.
#[derive(Debug, Clone, Copy)]
pub struct WinExitStatus {
    code: u32,
}

impl WinExitStatus {
    pub fn success(&self) -> bool {
        self.code == 0
    }

    pub fn code(&self) -> Option<i32> {
        Some(self.code as i32)
    }
}

impl SuspendedChild {
    /// The raw process handle, for assigning this (still-suspended)
    /// process to a containment Job Object before it is resumed.
    pub fn process_handle(&self) -> HANDLE {
        self.process.0
    }

    /// Resume the initial (and, since the process was created suspended and
    /// has not run any code yet, only) thread. Must only be called after
    /// the process has been assigned to its containment Job Object.
    pub fn resume(&self) -> io::Result<()> {
        // SAFETY: `self.thread.0` is the valid, still-open thread handle
        // `CreateProcessW` returned for this process's initial thread.
        let previous_suspend_count = unsafe { ResumeThread(self.thread.0) };
        if previous_suspend_count == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Forcefully terminate the process. Valid to call whether or not it
    /// has been resumed yet — used both by the normal timeout/cancel path
    /// (via the Job Object, not this) and by the failure-cleanup path when
    /// containment could not be established at all, matching the original
    /// `child.kill()` semantics for that case.
    pub fn terminate(&self) -> io::Result<()> {
        // SAFETY: `self.process.0` is a valid, still-open process handle.
        let ok = unsafe { TerminateProcess(self.process.0, 1) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Non-blocking exit check.
    pub fn try_wait(&self) -> io::Result<Option<WinExitStatus>> {
        // SAFETY: `self.process.0` is a valid, still-open process handle.
        match unsafe { WaitForSingleObject(self.process.0, 0) } {
            WAIT_OBJECT_0 => Ok(Some(self.exit_status()?)),
            0x0000_0102 /* WAIT_TIMEOUT */ => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }

    /// Block until the process exits, then return its status. Used after
    /// the Job Object has been terminated, to reap Nova's own handle to the
    /// direct child (mirroring `Child::wait()` after `Child::kill()`).
    pub fn wait(&self) -> io::Result<WinExitStatus> {
        // SAFETY: `self.process.0` is a valid, still-open process handle.
        let result = unsafe { WaitForSingleObject(self.process.0, INFINITE) };
        if result != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        self.exit_status()
    }

    fn exit_status(&self) -> io::Result<WinExitStatus> {
        let mut code: u32 = 0;
        // SAFETY: `self.process.0` is valid; `code` is a valid out-pointer.
        let ok = unsafe { GetExitCodeProcess(self.process.0, &mut code) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(WinExitStatus { code })
    }
}

/// Create `program` (resolved via [`resolve_executable`]) with `args`,
/// suspended (`CREATE_SUSPENDED`) with its stdout/stderr connected to fresh
/// pipes and its stdin connected to `NUL`. The returned [`SuspendedChild`]'s
/// initial thread has *not* run any of the target's code yet — the caller
/// must assign it to a containment Job Object and only then call
/// [`SuspendedChild::resume`].
///
/// Handle-inheritance is restricted explicitly via
/// `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` to exactly the three handles the
/// child needs (stdout write end, stderr write end, stdin/NUL) — the same
/// mechanism `std::process::Command` itself uses internally on Windows —
/// so no other handle in the Nova process can leak into the child even if
/// `bInheritHandles` is `TRUE`.
pub fn spawn_suspended(
    program: &str,
    args: &[String],
    cwd: &Path,
    with_stdin_pipe: bool,
) -> io::Result<SuspendedChild> {
    validate_process_request_strings(program, args, cwd)?;

    let executable = resolve_executable(program)?;
    if !executable.is_file() {
        return Err(io::Error::from(io::ErrorKind::NotFound));
    }

    let stdout_pipe = create_inheritable_pipe()?;
    let stderr_pipe = create_inheritable_pipe()?;
    // Two disjoint shapes, matching the two fields this can populate:
    // `with_stdin_pipe == false` connects the child's stdin to `NUL` (the
    // original, unchanged behavior for every caller that never writes
    // anything to a child) via a single "handle" branch; `true` creates a
    // real pipe and keeps *both* the inheritable child-side read handle
    // (for `CreateProcessW`) and the parent-side write handle (returned to
    // the caller as `SuspendedChild::stdin`).
    enum StdinSetup {
        Nul(OwnedHandle),
        Pipe(InheritablePipe),
    }
    let stdin_setup = if with_stdin_pipe {
        StdinSetup::Pipe(create_inheritable_pipe_for_child_stdin()?)
    } else {
        StdinSetup::Nul(open_nul_inheritable()?)
    };
    let stdin_child_handle = match &stdin_setup {
        StdinSetup::Nul(handle) => handle.0,
        StdinSetup::Pipe(pipe) => pipe.read.0,
    };

    let app_name = to_wide_null(&executable.to_string_lossy());
    let mut command_line = to_wide_null(&build_command_line(&executable.to_string_lossy(), args));
    let cwd_wide = to_wide_null(&cwd.to_string_lossy());

    // Build the attribute list restricting inherited handles to exactly the
    // three the child needs.
    let mut inherit_handles: [HANDLE; 3] =
        [stdin_child_handle, stdout_pipe.write.0, stderr_pipe.write.0];
    let mut attribute_list_size: usize = 0;
    // SAFETY: a null attribute-list pointer with count 1 is the documented
    // way to query the required buffer size.
    unsafe {
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut attribute_list_size);
    }
    let mut attribute_list_buffer = vec![0u8; attribute_list_size];
    let attribute_list = attribute_list_buffer.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
    // SAFETY: `attribute_list_buffer` is sized exactly as requested above
    // and kept alive for the rest of this function.
    let ok = unsafe {
        InitializeProcThreadAttributeList(attribute_list, 1, 0, &mut attribute_list_size)
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `attribute_list` was just initialized; `inherit_handles` is
    // a stack array of valid, still-open, inheritable handles kept alive
    // until after `CreateProcessW` returns below.
    let ok = unsafe {
        UpdateProcThreadAttribute(
            attribute_list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            inherit_handles.as_mut_ptr() as *mut core::ffi::c_void,
            size_of::<[HANDLE; 3]>(),
            null_mut(),
            null(),
        )
    };
    if ok == 0 {
        let error = io::Error::last_os_error();
        // SAFETY: `attribute_list` was successfully initialized above.
        unsafe {
            DeleteProcThreadAttributeList(attribute_list);
        }
        return Err(error);
    }

    let mut startup_info_ex: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup_info_ex.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup_info_ex.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup_info_ex.StartupInfo.hStdInput = stdin_child_handle;
    startup_info_ex.StartupInfo.hStdOutput = stdout_pipe.write.0;
    startup_info_ex.StartupInfo.hStdError = stderr_pipe.write.0;
    startup_info_ex.lpAttributeList = attribute_list;

    let mut process_information: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    // SAFETY: all pointer arguments are valid for the duration of this
    // call: `app_name`/`command_line`/`cwd_wide` are null-terminated wide
    // buffers kept alive on the stack; `command_line` is mutable as
    // `CreateProcessW` requires; `startup_info_ex` and
    // `process_information` are valid, correctly sized stack locals.
    let ok = unsafe {
        CreateProcessW(
            app_name.as_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1, // bInheritHandles: required for the attribute-list handle whitelist to take effect.
            CREATE_SUSPENDED | CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
            null(),
            cwd_wide.as_ptr(),
            &startup_info_ex.StartupInfo,
            &mut process_information,
        )
    };
    let create_process_error = if ok == 0 {
        Some(io::Error::last_os_error())
    } else {
        None
    };

    // SAFETY: `attribute_list` was successfully initialized above and is
    // no longer needed after `CreateProcessW` returns either way.
    unsafe {
        DeleteProcThreadAttributeList(attribute_list);
    }

    if let Some(error) = create_process_error {
        return Err(error);
    }

    // The child now owns its own copies of the inheritable handles; the
    // parent's copies of the *write*/*stdin-read* ends must be closed so the
    // parent is not left holding a pipe-write reference that would keep
    // the read end from ever seeing EOF once the child exits (the pipe
    // read loop blocks until every write-end handle, including any the
    // parent still held, is closed). The child's stdin *read* end is the
    // symmetric case: the parent's copy of it must also be closed (it only
    // ever needed to exist long enough for `CreateProcessW` to inherit it).
    drop(stdout_pipe.write);
    drop(stderr_pipe.write);
    let stdin_write_file = match stdin_setup {
        StdinSetup::Nul(handle) => {
            drop(handle);
            None
        }
        StdinSetup::Pipe(pipe) => {
            drop(pipe.read);
            // SAFETY: `pipe.write` was created as a valid, non-inheritable
            // handle above; wrapping it in `std::fs::File` transfers
            // ownership to the caller, who writes stdin content to it and
            // drops it (closing the pipe, signaling EOF to the child) once
            // done.
            let file = unsafe { std::fs::File::from_raw_handle(pipe.write.0 as _) };
            std::mem::forget(pipe.write);
            Some(file)
        }
    };

    // SAFETY: pipe read ends were created as valid, non-inheritable
    // handles above; wrapping them in `std::fs::File` transfers ownership
    // (this function no longer closes them directly — `File`'s `Drop`
    // does) and gives the rest of `process.rs` the same `Read` interface
    // it already used for `std::process::ChildStdout`/`ChildStderr`.
    let stdout = unsafe { std::fs::File::from_raw_handle(stdout_pipe.read.0 as _) };
    std::mem::forget(stdout_pipe.read);
    let stderr = unsafe { std::fs::File::from_raw_handle(stderr_pipe.read.0 as _) };
    std::mem::forget(stderr_pipe.read);

    Ok(SuspendedChild {
        process: OwnedHandle(process_information.hProcess),
        thread: OwnedHandle(process_information.hThread),
        stdout: Some(stdout),
        stderr: Some(stderr),
        stdin: stdin_write_file,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_plain_argument_without_modification() {
        let mut out = String::new();
        quote_windows_arg("plain", &mut out);
        assert_eq!(out, "plain");
    }

    #[test]
    fn quotes_empty_argument_as_empty_quoted_string() {
        let mut out = String::new();
        quote_windows_arg("", &mut out);
        assert_eq!(out, "\"\"");
    }

    #[test]
    fn quotes_argument_with_spaces() {
        let mut out = String::new();
        quote_windows_arg("has space", &mut out);
        assert_eq!(out, "\"has space\"");
    }

    #[test]
    fn escapes_embedded_quote() {
        let mut out = String::new();
        quote_windows_arg("say \"hi\"", &mut out);
        assert_eq!(out, "\"say \\\"hi\\\"\"");
    }

    #[test]
    fn preserves_literal_backslashes_not_before_a_quote() {
        let mut out = String::new();
        quote_windows_arg(r"C:\no\spaces\here", &mut out);
        // No spaces/quotes: passed through unmodified, backslashes not doubled.
        assert_eq!(out, r"C:\no\spaces\here");
    }

    #[test]
    fn doubles_trailing_backslash_before_closing_quote() {
        let mut out = String::new();
        quote_windows_arg(r"C:\has space\", &mut out);
        assert_eq!(out, "\"C:\\has space\\\\\"");
    }

    #[test]
    fn doubles_backslashes_immediately_preceding_an_embedded_quote() {
        let mut out = String::new();
        quote_windows_arg(r#"a\"b"#, &mut out);
        // One backslash before a quote -> two backslashes + escaped quote.
        assert_eq!(out, "\"a\\\\\\\"b\"");
    }

    #[test]
    fn build_command_line_joins_quoted_tokens_with_single_spaces() {
        let line = build_command_line("prog", &["a b".to_string(), "c".to_string()]);
        assert_eq!(line, "prog \"a b\" c");
    }

    #[test]
    fn resolve_executable_finds_git_on_path() {
        let resolved = resolve_executable("git").expect("git should resolve via PATH");
        assert!(resolved.is_file());
        assert!(resolved
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_lowercase()
            .starts_with("git"));
    }

    #[test]
    fn resolve_executable_fails_for_unknown_name() {
        let name = format!("nova-definitely-nonexistent-{}", uuid::Uuid::new_v4());
        assert!(resolve_executable(&name).is_err());
    }

    #[test]
    fn resolve_executable_never_finds_a_planted_file_outside_path() {
        // Plant a "trusted-looking" executable in an arbitrary temp
        // directory that is (almost certainly) not on PATH, and prove
        // resolution does not find it by bare name — it only ever
        // considers PATH directories. Deliberately does *not* mutate the
        // process's current directory (unlike a naive version of this
        // test would), since `std::env::set_current_dir` is process-wide
        // state that other tests running concurrently could observe.
        let dir =
            std::env::temp_dir().join(format!("nova-win-spawn-plant-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("nova-fake-plant-{}", uuid::Uuid::new_v4());
        let fake = dir.join(format!("{name}.exe"));
        std::fs::write(&fake, b"not a real exe").unwrap();
        let path_var = std::env::var_os("PATH").unwrap_or_default();
        let already_on_path = std::env::split_paths(&path_var).any(|p| p == dir);
        assert!(
            !already_on_path,
            "test invariant broken: temp plant directory must not already be on PATH"
        );

        let result = resolve_executable(&name);

        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            result.is_err(),
            "a file outside every PATH directory must never be resolved by bare name"
        );
    }

    // --- Embedded-NUL rejection --------------------------------------------

    fn ok_args() -> Vec<String> {
        vec!["--version".to_string()]
    }

    fn ok_cwd() -> std::path::PathBuf {
        std::env::temp_dir()
    }

    #[test]
    fn rejects_program_with_embedded_nul() {
        let program = "git\0evil";
        let err = validate_process_request_strings(program, &ok_args(), &ok_cwd()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn rejects_cwd_with_embedded_nul() {
        let mut cwd = ok_cwd();
        cwd.push("bad\0dir");
        let err = validate_process_request_strings("git", &ok_args(), &cwd).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn rejects_first_arg_with_embedded_nul() {
        let args = vec!["bad\0first".to_string(), "safe".to_string()];
        let err = validate_process_request_strings("git", &args, &ok_cwd()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn rejects_middle_arg_with_embedded_nul() {
        let args = vec![
            "safe-one".to_string(),
            "bad\0middle".to_string(),
            "safe-two".to_string(),
        ];
        let err = validate_process_request_strings("git", &args, &ok_cwd()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn rejects_last_arg_with_embedded_nul() {
        let args = vec!["safe".to_string(), "bad\0last".to_string()];
        let err = validate_process_request_strings("git", &args, &ok_cwd()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn rejects_nul_even_when_surrounded_by_shell_metacharacters() {
        // Proves the NUL check runs independently of (and before) any
        // quoting/escaping logic — a NUL wrapped in shell-looking noise is
        // still rejected, not "cleaned up" and allowed through.
        let args = vec!["&& calc.exe \0 | rm -rf / ;".to_string()];
        let err = validate_process_request_strings("git", &args, &ok_cwd()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn nul_rejection_error_message_does_not_leak_the_offending_value() {
        let fake_secret = ["sk", "totally-fake-nul-adjacent-secret"].join("-");
        let args = vec![format!("{fake_secret}\0trailing")];
        let err = validate_process_request_strings("git", &args, &ok_cwd()).unwrap_err();
        let message = err.to_string();
        assert!(
            !message.contains(&fake_secret),
            "error message must not echo the offending argument's content: {message:?}"
        );
        assert!(
            message.contains("NUL"),
            "error message should explain what was rejected"
        );
    }

    #[test]
    fn does_not_reject_ordinary_unicode_and_chinese_text() {
        let args = vec!["项目 目录 with spaces 中文".to_string(), "café".to_string()];
        assert!(validate_process_request_strings("git", &args, &ok_cwd()).is_ok());
    }

    #[test]
    fn does_not_reject_a_normal_empty_string_argument() {
        let args = vec![String::new(), "safe".to_string()];
        assert!(validate_process_request_strings("git", &args, &ok_cwd()).is_ok());
    }

    #[test]
    fn spawn_suspended_rejects_embedded_nul_before_touching_the_filesystem() {
        // End-to-end: `spawn_suspended` itself (not just the standalone
        // validator) must refuse a NUL-containing argument, and must do so
        // before `resolve_executable`/`CreateProcessW` run at all — proven
        // here by pointing `program` at a deliberately nonexistent name: if
        // validation ran *after* executable resolution, this would fail
        // with `NotFound` instead of `InvalidInput`.
        let program = format!("nova-nonexistent-{}", uuid::Uuid::new_v4());
        let args = vec!["bad\0arg".to_string()];
        let err = match spawn_suspended(&program, &args, &ok_cwd(), false) {
            Err(error) => error,
            Ok(_) => panic!("expected spawn_suspended to reject the NUL-containing argument"),
        };
        assert_eq!(
            err.kind(),
            io::ErrorKind::InvalidInput,
            "expected InvalidInput (validation-before-resolution), got {err:?}"
        );
    }
}
