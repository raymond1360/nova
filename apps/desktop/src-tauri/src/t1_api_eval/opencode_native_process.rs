//! NB2 private process boundary. No live entry is wired. Native code is compiled only.
//! NB4 must verify managedConfigDir and active-account org configuration: both can
//! merge AFTER the environment overlay. This module does not prepare or certify config.
use super::{
    opencode_nonce::{Control, Model, RunNonce, RunSecrets},
    Result,
};
use std::time::{Duration, Instant};

pub const MAX_ENV_UNITS: usize = 32768;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchRole {
    GitPreflight,
    OpenCode,
}
/// Millisecond offsets on ONE caller-owned monotonic clock. Never reset per API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeadlinePlan {
    pub start: u64,
    pub chain: u64,
    pub cleanup_end: u64,
    pub reader_end: u64,
}
impl DeadlinePlan {
    pub fn git(start: u64, snapshot_end: u64, target_end: u64, global_end: u64) -> Result<Self> {
        Self::make(
            start,
            start
                .checked_add(5000)
                .ok_or("deadline overflow")?
                .min(snapshot_end)
                .min(target_end)
                .min(global_end),
        )
    }
    pub fn opencode(
        start: u64,
        startup_end: u64,
        target_end: u64,
        global_end: u64,
    ) -> Result<Self> {
        Self::make(start, startup_end.min(target_end).min(global_end))
    }
    fn make(start: u64, outer: u64) -> Result<Self> {
        let cleanup_end = outer.checked_sub(1000).ok_or("no cleanup reserve")?;
        let chain = start
            .checked_add(5000)
            .ok_or("deadline overflow")?
            .min(cleanup_end.checked_sub(1000).ok_or("no reader reserve")?);
        if chain <= start {
            return Err("insufficient reserved budget".into());
        }
        Ok(Self {
            start,
            chain,
            cleanup_end,
            reader_end: outer,
        })
    }
    pub fn cleanup_at(self, now: u64) -> u64 {
        now.saturating_add(1000).min(self.cleanup_end)
    }
    pub fn instants(self, origin: Instant) -> Result<(Instant, Instant, Instant)> {
        Ok((
            origin
                .checked_add(Duration::from_millis(self.chain))
                .ok_or("deadline range")?,
            origin
                .checked_add(Duration::from_millis(self.cleanup_end))
                .ok_or("deadline range")?,
            origin
                .checked_add(Duration::from_millis(self.reader_end))
                .ok_or("deadline range")?,
        ))
    }
}
/// No Debug/Serialize/Clone: a buffer may contain credentials. Capacity is fixed before copying.
pub struct EnvironmentBlock {
    units: Vec<u16>,
}
impl Drop for EnvironmentBlock {
    fn drop(&mut self) {
        for x in &mut self.units {
            unsafe { std::ptr::write_volatile(x, 0) }
        }
    }
}
fn name_key(s: &str) -> Result<Vec<u8>> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err("environment name rejected".into());
    }
    Ok(s.bytes().map(|b| b.to_ascii_uppercase()).collect())
}
impl EnvironmentBlock {
    /// Inputs are only the already closed fixed-key adapter, not CLI/env inheritance.
    pub(super) fn encode<'a>(
        mut values: Vec<(&'a str, &'a str)>,
        secrets: &RunSecrets,
        port: u16,
        role: LaunchRole,
    ) -> Result<Self> {
        let secret_names = [
            "OPENCODE_SERVER_USERNAME",
            "OPENCODE_SERVER_PASSWORD",
            "OPENCODE_CONFIG_CONTENT",
        ];
        for (k, v) in &values {
            name_key(k)?;
            if v.contains('\0') || secret_names.iter().any(|x| k.eq_ignore_ascii_case(x)) {
                return Err("environment value/credential override".into());
            }
        }
        let overlay_head=format!("{{\"provider\":{{\"nova\":{{\"options\":{{\"baseURL\":\"http://127.0.0.1:{port}/v1\",\"apiKey\":\"");
        let overlay_tail = "\"}}}}";
        if role == LaunchRole::OpenCode && port == 0 {
            return Err("model port missing".into());
        }
        enum Val<'a> {
            Plain(&'a str),
            Control,
            Overlay,
        }
        let mut rows = Vec::new();
        for (k, v) in values.drain(..) {
            rows.push((name_key(k)?, k, Val::Plain(v)));
        }
        if role == LaunchRole::OpenCode {
            rows.push((
                name_key(secret_names[0])?,
                secret_names[0],
                Val::Plain("opencode"),
            ));
            rows.push((name_key(secret_names[1])?, secret_names[1], Val::Control));
            rows.push((name_key(secret_names[2])?, secret_names[2], Val::Overlay));
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        if rows.windows(2).any(|w| w[0].0 == w[1].0) {
            return Err("duplicate environment name".into());
        }
        let mut total = 1usize;
        for (_, k, v) in &rows {
            let n = match v {
                Val::Plain(v) => v.encode_utf16().count(),
                Val::Control => 64,
                Val::Overlay => overlay_head.encode_utf16().count() + 64 + overlay_tail.len(),
            };
            total = total
                .checked_add(k.len() + 2)
                .and_then(|n0| n0.checked_add(n))
                .ok_or("environment overflow")?;
        }
        if rows.is_empty() {
            total = 2
        }
        if total > MAX_ENV_UNITS {
            return Err("environment bound".into());
        }
        let mut out = Self {
            units: Vec::with_capacity(total),
        };
        for (_, k, v) in rows {
            out.units.extend(k.encode_utf16());
            out.units.push(b'=' as u16);
            match v {
                Val::Plain(v) => out.units.extend(v.encode_utf16()),
                Val::Control => secrets
                    .control
                    .with_bytes(|b| out.units.extend(b.iter().map(|x| *x as u16))),
                Val::Overlay => {
                    out.units.extend(overlay_head.encode_utf16());
                    secrets
                        .model
                        .with_bytes(|b| out.units.extend(b.iter().map(|x| *x as u16)));
                    out.units.extend(overlay_tail.encode_utf16());
                }
            }
            out.units.push(0);
        }
        out.units.push(0);
        if out.units.len() == 1 {
            out.units.push(0)
        }
        if out.units.len() != total {
            return Err("environment size invariant".into());
        }
        Ok(out)
    }
    #[cfg(test)]
    pub(super) fn units(&self) -> &[u16] {
        &self.units
    }
}
/// An ordinary nonsecret base is built from empty, never env::vars().
pub fn git_environment() -> Result<EnvironmentBlock> {
    // No nonce is required or generated for Git; this is intentionally separate.
    let env = super::opencode_probe::minimal_environment();
    let mut names: Vec<_> = env.iter().collect();
    names.sort_by_key(|(k, _)| k.to_ascii_uppercase());
    let total = names
        .iter()
        .try_fold(1usize, |n, (k, v)| -> Result<usize> {
            name_key(k)?;
            if v.contains('\0') {
                return Err("environment NUL".into());
            }
            n.checked_add(k.encode_utf16().count() + v.encode_utf16().count() + 2)
                .ok_or("environment bound".into())
        })?;
    if total > MAX_ENV_UNITS {
        return Err("environment bound".into());
    }
    let mut b = EnvironmentBlock {
        units: Vec::with_capacity(total),
    };
    for (k, v) in names {
        b.units.extend(k.encode_utf16());
        b.units.push(61);
        b.units.extend(v.encode_utf16());
        b.units.push(0)
    }
    b.units.push(0);
    Ok(b)
}
pub fn opencode_environment(secrets: &RunSecrets, port: u16) -> Result<EnvironmentBlock> {
    secrets.distinct()?;
    secrets
        .model
        .with_bytes(RunNonce::<Model>::validate_external)?;
    secrets
        .control
        .with_bytes(RunNonce::<Control>::validate_external)?;
    let env = super::opencode_probe::minimal_environment();
    EnvironmentBlock::encode(
        env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect(),
        secrets,
        port,
        LaunchRole::OpenCode,
    )
}
pub(super) fn quote_arg(s: &str) -> Result<String> {
    if s.contains('\0') || s.contains(['\r', '\n']) {
        return Err("command argument rejected".into());
    }
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in s.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        if c == '"' {
            out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
            out.push('"')
        } else {
            out.extend(std::iter::repeat_n('\\', slashes));
            out.push(c)
        }
        slashes = 0;
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    Ok(out)
}
#[derive(Clone)]
pub struct FrozenImage {
    pub role: LaunchRole,
    pub path: String,
    pub sha256: String,
}
impl FrozenImage {
    pub fn validate(&self) -> Result<()> {
        let expected = match self.role {
            LaunchRole::GitPreflight => r"D:\NovaM0BInput\git\cmd\git.exe",
            LaunchRole::OpenCode => r"D:\NovaM0BInput\opencode.exe",
        };
        if self.path != expected
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("fixed frozen image required".into());
        }
        Ok(())
    }
}
/// ResumeThread returns the PREVIOUS suspension count. Only one releases our suspension.
pub(super) fn validate_resume_count(previous: u32) -> Result<()> {
    if previous != 1 {
        return Err("resume previous suspension count UNKNOWN".into());
    }
    Ok(())
}

/// Same Git scheduling skeleton is used by native IO and injected tests.
pub trait GitIo<P: super::opencode_probe::ProcessOps> {
    fn start(&mut self, p: &mut P) -> Result<()>;
    fn overflow(&self) -> bool;
    fn finish(&mut self, p: &mut P, deadline: Instant) -> Result<Vec<u8>>;
}
#[derive(Debug)]
pub struct GitStageReceipt {
    pub job: super::opencode_paths::JobStopReceipt,
    pub stdout: Vec<u8>,
    pub natural: bool,
}
#[derive(Debug)]
pub struct GitStageFailure {
    pub reason: String,
    pub launch: Option<super::opencode_probe::LaunchFailure>,
    pub stop: Option<super::opencode_paths::JobStopReceipt>,
    pub readers_confirmed: bool,
    // Original per-operation cutoffs. Retention must never grant a new cleanup/reader budget.
    pub cleanup_deadline: Option<Instant>,
    pub reader_deadline: Option<Instant>,
}
/// Failure ownership is returned, never implicitly detached by execute_git.
/// For native P this retains child/thread/pipe/image handles AND borrows the original Job.
/// Dropping it is only a last-resort termination request, never confirmation of reclamation.
pub struct GitResources<P, I> {
    pub process: P,
    pub io: I,
    cleanup_end: Instant,
    reader_end: Instant,
    confirmed_stop: Option<super::opencode_paths::JobStopReceipt>,
    confirmed_readers: bool,
}
pub struct OwnedGitFailure<P, I> {
    pub stage: GitStageFailure,
    pub resources: Option<GitResources<P, I>>,
}
impl<P, I> std::fmt::Debug for OwnedGitFailure<P, I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedGitFailure")
            .field("stage", &self.stage)
            .field("resources_retained", &self.resources.is_some())
            .finish()
    }
}
#[derive(Debug)]
pub struct GitReclaimReceipt {
    pub stop: Option<super::opencode_paths::JobStopReceipt>,
    pub readers_confirmed: bool,
    pub error: Option<String>,
}
impl GitReclaimReceipt {
    pub fn confirmed(&self) -> bool {
        self.error.is_none()
            && self.readers_confirmed
            && self.stop.as_ref().is_some_and(|r| r.validate().is_ok())
    }
}
impl<P: super::opencode_probe::ProcessOps, I: GitIo<P>> GitResources<P, I> {
    /// No new stage budget: both retries are capped by the ORIGINAL reserved deadlines.
    /// The owner remains held even on UNKNOWN, so callers may preserve responsibility.
    pub fn reclaim(&mut self) -> GitReclaimReceipt {
        let stop = if let Some(r) = &self.confirmed_stop {
            // Already proved before failure; not a new termination or a Drop observation.
            Ok(r.clone())
        } else if self.process.now() < self.cleanup_end {
            super::opencode_probe::confirm_stopped(&mut self.process, self.cleanup_end)
        } else {
            Err("Git retained cleanup deadline UNKNOWN".into())
        };
        if let Ok(r) = &stop {
            self.confirmed_stop = Some(r.clone());
        }
        let end = self.reader_end;
        let readers = if self.confirmed_readers {
            Ok(Vec::new()) // Preserve a completed join; no second reader operation.
        } else if self.process.now() < end {
            self.io.finish(&mut self.process, end)
        } else {
            Err("Git retained readers deadline UNKNOWN".into())
        };
        let readers_confirmed =
            self.confirmed_readers || (readers.is_ok() && self.process.now() < end);
        self.confirmed_readers = readers_confirmed;
        let error = stop
            .as_ref()
            .err()
            .cloned()
            .or_else(|| readers.err())
            .or_else(|| (!readers_confirmed).then(|| "Git retained readers late UNKNOWN".into()));
        GitReclaimReceipt {
            stop: stop.ok(),
            readers_confirmed,
            error,
        }
    }
}
pub fn supervise_owned_git<P: super::opencode_probe::ProcessOps, I: GitIo<P>>(
    process: P,
    io: I,
    origin: Instant,
    plan: DeadlinePlan,
) -> std::result::Result<(GitStageReceipt, GitResources<P, I>), OwnedGitFailure<P, I>> {
    // Even an invalid plan returns ownership; never lose resources on an error conversion.
    let cleanup_end = origin
        .checked_add(Duration::from_millis(plan.cleanup_end))
        .unwrap_or(origin);
    let reader_end = origin
        .checked_add(Duration::from_millis(plan.reader_end))
        .unwrap_or(origin);
    let mut resources = GitResources {
        process,
        io,
        cleanup_end,
        reader_end,
        confirmed_stop: None,
        confirmed_readers: false,
    };
    match supervise_git(&mut resources.process, &mut resources.io, origin, plan) {
        Ok(r) => {
            resources.confirmed_stop = Some(r.job.clone());
            resources.confirmed_readers = true;
            Ok((r, resources))
        }
        Err(stage) => {
            // Launch failure does not expose unused cleanup time: fail closed, hold ownership.
            resources.cleanup_end = stage
                .cleanup_deadline
                .unwrap_or_else(|| resources.process.now());
            resources.reader_end = stage
                .reader_deadline
                .unwrap_or_else(|| resources.process.now());
            resources.confirmed_stop = stage.stop.clone();
            resources.confirmed_readers = stage.readers_confirmed;
            Err(OwnedGitFailure {
                stage,
                resources: Some(resources),
            })
        }
    }
}
pub fn supervise_git<P: super::opencode_probe::ProcessOps, I: GitIo<P>>(
    p: &mut P,
    io: &mut I,
    origin: Instant,
    plan: DeadlinePlan,
) -> std::result::Result<GitStageReceipt, GitStageFailure> {
    use super::opencode_probe::{confirm_stopped, launch_verified_until};
    let (chain, cleanup_end, reader_end) = plan.instants(origin).map_err(|e| GitStageFailure {
        reason: e,
        launch: None,
        stop: None,
        readers_confirmed: false,
        cleanup_deadline: None,
        reader_deadline: None,
    })?;
    launch_verified_until(p, chain, cleanup_end).map_err(|e| GitStageFailure {
        reason: "Git launch refused".into(),
        launch: Some(e),
        stop: None,
        readers_confirmed: false,
        cleanup_deadline: None,
        reader_deadline: None,
    })?;
    let mut started = false;
    let mut natural = false;
    let operation = (|| -> Result<()> {
        io.start(p)?;
        started = true;
        loop {
            if p.now() >= chain || io.overflow() {
                return Err("Git deadline/output bound".into());
            }
            let exited = p.exited()?;
            let (flags, active) = p.flags_and_active()?;
            if p.now() >= chain {
                return Err("Git query late UNKNOWN".into());
            }
            if flags != 0x2000 {
                return Err("Git Job flags changed".into());
            }
            if exited && active == 0 {
                natural = true;
                return Ok(());
            }
            p.pause();
        }
    })();
    let cleanup_deadline = cleanup_end.min(p.now() + Duration::from_secs(1));
    let stopped = if natural {
        Ok(super::opencode_paths::JobStopReceipt {
            flags: Some(0x2000),
            active: Some(0),
            direct_exited: Some(true),
            error: None,
        })
    } else {
        confirm_stopped(p, cleanup_deadline)
    };
    // Always attempt bounded reader collection after a stop; never hide an earlier failure.
    let reader_deadline = reader_end.min(p.now() + Duration::from_secs(1));
    let output = if started && p.now() < reader_deadline {
        io.finish(p, reader_deadline)
    } else {
        Err("Git readers unstarted/UNKNOWN".into())
    };
    let readers_ok = output.is_ok() && p.now() < reader_deadline;
    if operation.is_err() || stopped.is_err() || !readers_ok {
        return Err(GitStageFailure {
            reason: operation
                .err()
                .or_else(|| stopped.as_ref().err().cloned())
                .or_else(|| output.as_ref().err().cloned())
                .unwrap_or("Git stage late UNKNOWN".into()),
            launch: None,
            stop: stopped.ok(),
            readers_confirmed: readers_ok,
            cleanup_deadline: Some(cleanup_deadline),
            reader_deadline: Some(reader_deadline),
        });
    }
    Ok(GitStageReceipt {
        job: stopped.unwrap(),
        stdout: output.unwrap(),
        natural,
    })
}
#[cfg(windows)]
pub mod native {
    use super::super::{
        opencode_paths::{ProcessIdentity, RootReservation},
        opencode_probe::{native::ProbeJob, ProcessOps},
    };
    use super::*;
    use std::{
        fs::File,
        io::Read,
        os::windows::io::FromRawHandle,
        path::Path,
        sync::{Mutex, MutexGuard},
    };
    use windows_sys::Win32::{
        Foundation::*,
        Security::*,
        Storage::FileSystem::*,
        System::{Pipes::*, Threading::*},
    };
    static SPAWN_SERIAL: Mutex<()> = Mutex::new(());
    pub struct Handle(pub HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }
    impl Handle {
        fn file(self) -> File {
            let h = self.0;
            std::mem::forget(self);
            unsafe { File::from_raw_handle(h as _) }
        }
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn err() -> String {
        "native API failed (details withheld)".into()
    }
    fn check(b: BOOL) -> Result<()> {
        if b == 0 {
            Err(err())
        } else {
            Ok(())
        }
    }
    pub fn parent_policy() -> Result<u32> {
        unsafe {
            // The locked payload is exactly its aligned u32 Flags union; retain all unknown bits.
            let mut flags = 0u32;
            check(GetProcessMitigationPolicy(
                GetCurrentProcess(),
                ProcessChildProcessPolicy,
                &mut flags as *mut _ as _,
                std::mem::size_of_val(&flags),
            ))?;
            Ok(flags)
        }
    }
    pub fn process_identity(h: HANDLE) -> Result<ProcessIdentity> {
        unsafe {
            let (mut c, mut e, mut k, mut u) = (
                std::mem::zeroed(),
                std::mem::zeroed(),
                std::mem::zeroed(),
                std::mem::zeroed(),
            );
            check(GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u))?;
            let v = ProcessIdentity {
                pid: GetProcessId(h),
                created: ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64,
            };
            v.validate()?;
            Ok(v)
        }
    }
    pub fn token_sid(h: HANDLE) -> Result<String> {
        unsafe {
            let mut token = std::ptr::null_mut();
            check(OpenProcessToken(h, TOKEN_QUERY, &mut token))?;
            let token = Handle(token);
            let mut n = 0;
            GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut n);
            if n == 0 || n > 65536 {
                return Err("token size".into());
            }
            let mut b = vec![0u64; (n as usize + 7) / 8];
            check(GetTokenInformation(
                token.0,
                TokenUser,
                b.as_mut_ptr() as _,
                n,
                &mut n,
            ))?;
            let user = &*(b.as_ptr() as *const TOKEN_USER);
            super::super::opencode_native_files::native::sid_string(user.User.Sid)
        }
    }
    fn verify_ordinary(h: HANDLE) -> Result<()> {
        unsafe {
            let mut t = std::ptr::null_mut();
            check(OpenProcessToken(h, TOKEN_QUERY, &mut t))?;
            let t = Handle(t);
            let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
            let mut app = 1u32;
            let mut n = 0;
            check(GetTokenInformation(
                t.0,
                TokenElevation,
                &mut elevation as *mut _ as _,
                std::mem::size_of_val(&elevation) as u32,
                &mut n,
            ))?;
            check(GetTokenInformation(
                t.0,
                TokenIsAppContainer,
                &mut app as *mut _ as _,
                std::mem::size_of_val(&app) as u32,
                &mut n,
            ))?;
            if elevation.TokenIsElevated != 0 || app != 0 || IsTokenRestricted(t.0) != 0 {
                return Err("ordinary non-elevated token required".into());
            }
            Ok(())
        }
    }
    fn pipe() -> Result<(Handle, Handle)> {
        unsafe {
            let mut sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: std::ptr::null_mut(),
                bInheritHandle: 1,
            };
            let (mut r, mut w) = (std::ptr::null_mut(), std::ptr::null_mut());
            check(CreatePipe(&mut r, &mut w, &mut sa, 0))?;
            let (r, w) = (Handle(r), Handle(w));
            check(SetHandleInformation(r.0, HANDLE_FLAG_INHERIT, 0))?;
            Ok((r, w))
        }
    }
    struct Attr {
        _buffer: Vec<usize>,
        ptr: LPPROC_THREAD_ATTRIBUTE_LIST,
    }
    impl Drop for Attr {
        fn drop(&mut self) {
            unsafe { DeleteProcThreadAttributeList(self.ptr) }
        }
    }
    pub struct NativeSuspendedChild {
        process: Handle,
        thread: Handle,
        pub stdout: Option<File>,
        pub stderr: Option<File>,
        pub identity: ProcessIdentity,
        pub thread_id: u32,
    }
    impl NativeSuspendedChild {
        fn spawn(
            image: &FrozenImage,
            env: &mut EnvironmentBlock,
            control_port: Option<u16>,
            deadline: Instant,
        ) -> Result<Self> {
            image.validate()?;
            let until = deadline.min(Instant::now() + Duration::from_secs(1));
            let _serial: MutexGuard<'_, ()> = loop {
                match SPAWN_SERIAL.try_lock() {
                    Ok(v) => break v,
                    Err(std::sync::TryLockError::Poisoned(_)) => {
                        return Err("spawn lock poisoned".into())
                    }
                    Err(_) => {
                        if Instant::now() >= until {
                            return Err("spawn serial deadline".into());
                        }
                        std::thread::yield_now()
                    }
                }
            };
            unsafe {
                let (out_r, out_w) = pipe()?;
                let (err_r, err_w) = pipe()?;
                let mut sa = SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: std::ptr::null_mut(),
                    bInheritHandle: 1,
                };
                let nul = Handle(CreateFileW(
                    wide("NUL").as_ptr(),
                    GENERIC_READ,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    &mut sa,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    std::ptr::null_mut(),
                ));
                if nul.0 == INVALID_HANDLE_VALUE {
                    return Err(err());
                }
                let args: Vec<String> = match image.role {
                    LaunchRole::GitPreflight => vec!["rev-parse".into(), "--show-toplevel".into()],
                    LaunchRole::OpenCode => vec![
                        "serve".into(),
                        "--hostname".into(),
                        "127.0.0.1".into(),
                        "--port".into(),
                        control_port
                            .filter(|p| *p != 0)
                            .ok_or("registered control port missing")?
                            .to_string(),
                    ],
                };
                if Instant::now() >= deadline {
                    return Err("spawn deadline before API".into());
                }
                let cmd = std::iter::once(image.path.as_str())
                    .chain(args.iter().map(String::as_str))
                    .map(quote_arg)
                    .collect::<Result<Vec<_>>>()?
                    .join(" ");
                Self::spawn_parts(
                    image,
                    env,
                    &cmd,
                    out_r,
                    out_w,
                    err_r,
                    err_w,
                    nul,
                    super::super::opencode_paths::ROOT,
                )
            }
        }
        unsafe fn spawn_parts(
            image: &FrozenImage,
            env: &mut EnvironmentBlock,
            command: &str,
            out_r: Handle,
            out_w: Handle,
            err_r: Handle,
            err_w: Handle,
            nul: Handle,
            cwd: &str,
        ) -> Result<Self> {
            let mut n = 0;
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut n);
            if n == 0 || n > 65536 {
                return Err("attribute size".into());
            }
            let mut buffer =
                vec![0usize; (n + std::mem::size_of::<usize>() - 1) / std::mem::size_of::<usize>()];
            let ptr = buffer.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
            check(InitializeProcThreadAttributeList(ptr, 1, 0, &mut n))?;
            let _attr = Attr {
                _buffer: buffer,
                ptr,
            };
            let mut hs = [nul.0, out_w.0, err_w.0];
            check(UpdateProcThreadAttribute(
                ptr,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                hs.as_mut_ptr() as _,
                std::mem::size_of_val(&hs),
                std::ptr::null_mut(),
                std::ptr::null(),
            ))?;
            let mut si: STARTUPINFOEXW = std::mem::zeroed();
            si.StartupInfo.cb = std::mem::size_of_val(&si) as u32;
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.StartupInfo.hStdInput = nul.0;
            si.StartupInfo.hStdOutput = out_w.0;
            si.StartupInfo.hStdError = err_w.0;
            si.lpAttributeList = ptr;
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let mut line = wide(command);
            check(CreateProcessW(
                wide(&image.path).as_ptr(),
                line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                CREATE_SUSPENDED
                    | CREATE_NO_WINDOW
                    | EXTENDED_STARTUPINFO_PRESENT
                    | CREATE_UNICODE_ENVIRONMENT,
                env.units.as_ptr() as _,
                wide(cwd).as_ptr(),
                &si.StartupInfo,
                &mut pi,
            ))?;
            let process = Handle(pi.hProcess);
            let thread = Handle(pi.hThread);
            // No fallible operations after successful creation; caller owns cleanup even when identity fails.
            let identity = process_identity(process.0).unwrap_or(ProcessIdentity {
                pid: pi.dwProcessId,
                created: 0,
            });
            drop(out_w);
            drop(err_w);
            drop(nul);
            Ok(Self {
                process,
                thread,
                stdout: Some(out_r.file()),
                stderr: Some(err_r.file()),
                identity,
                thread_id: pi.dwThreadId,
            })
        }
        pub fn exited(&self) -> Result<bool> {
            unsafe {
                match WaitForSingleObject(self.process.0, 0) {
                    WAIT_OBJECT_0 => Ok(true),
                    WAIT_TIMEOUT => Ok(false),
                    _ => Err(err()),
                }
            }
        }
        pub fn final_exit(&self) -> Result<Option<u32>> {
            if !self.exited()? {
                return Ok(None);
            }
            unsafe {
                let mut n = 0;
                check(GetExitCodeProcess(self.process.0, &mut n))?;
                Ok(Some(n))
            }
        }
    }
    pub struct NativeProcessOps<'a> {
        pub job: &'a ProbeJob,
        image: FrozenImage,
        env: Option<EnvironmentBlock>,
        child: Option<NativeSuspendedChild>,
        sid: String,
        image_guard: Option<File>,
        root: RootReservation,
        control_port: Option<u16>,
        chain_deadline: Instant,
    }
    impl<'a> NativeProcessOps<'a> {
        fn git(
            job: &'a ProbeJob,
            image: FrozenImage,
            sid: String,
            root: RootReservation,
            chain_deadline: Instant,
        ) -> Result<Self> {
            image.validate()?;
            if image.role != LaunchRole::GitPreflight {
                return Err("Git role".into());
            }
            if parent_policy()? != 0 || token_sid(unsafe { GetCurrentProcess() })? != sid {
                return Err("parent policy/SID rejected".into());
            }
            verify_ordinary(unsafe { GetCurrentProcess() })?;
            Ok(Self {
                job,
                image,
                env: Some(git_environment()?),
                child: None,
                sid,
                image_guard: None,
                root,
                control_port: None,
                chain_deadline,
            })
        }
        pub fn opencode(
            job: &'a ProbeJob,
            image: FrozenImage,
            sid: String,
            root: RootReservation,
            secrets: &RunSecrets,
            model_port: u16,
            control_port: u16,
            origin: Instant,
            startup_end: u64,
            target_end: u64,
            global_end: u64,
        ) -> Result<Self> {
            let start = Instant::now()
                .checked_duration_since(origin)
                .ok_or("clock origin")?
                .as_millis() as u64;
            let plan = DeadlinePlan::opencode(
                start,
                startup_end.min(start.checked_add(20000).ok_or("clock overflow")?),
                target_end,
                global_end,
            )?;
            let (chain_deadline, _, _) = plan.instants(origin)?;
            image.validate()?;
            if image.role != LaunchRole::OpenCode || control_port == 0 || control_port == model_port
            {
                return Err("registered roles/ports".into());
            }
            if parent_policy()? != 0 || token_sid(unsafe { GetCurrentProcess() })? != sid {
                return Err("parent policy/SID rejected".into());
            }
            verify_ordinary(unsafe { GetCurrentProcess() })?;
            Ok(Self {
                job,
                image,
                env: Some(opencode_environment(secrets, model_port)?),
                child: None,
                sid,
                image_guard: None,
                root,
                control_port: Some(control_port),
                chain_deadline,
            })
        }
        pub fn identity(&self) -> Option<&ProcessIdentity> {
            self.child.as_ref().map(|c| &c.identity)
        }
        pub fn take_output(&mut self) -> Result<(File, File)> {
            let c = self.child.as_mut().ok_or("no child")?;
            Ok((
                c.stdout.take().ok_or("stdout already taken")?,
                c.stderr.take().ok_or("stderr already taken")?,
            ))
        }
        pub fn final_exit(&self) -> Result<Option<u32>> {
            self.child.as_ref().ok_or("no child")?.final_exit()
        }
    }
    impl ProcessOps for NativeProcessOps<'_> {
        fn create_suspended(&mut self) -> Result<()> {
            if self.child.is_some() {
                return Err("create replay".into());
            }
            if parent_policy()? != 0 || token_sid(unsafe { GetCurrentProcess() })? != self.sid {
                return Err("parent policy/SID changed".into());
            }
            verify_ordinary(unsafe { GetCurrentProcess() })?;
            let g = super::super::opencode_native_files::native::guard_image(
                Path::new(&self.image.path),
                &self.image.sha256,
            )?;
            super::super::opencode_native_files::native::verify_root(&self.root)?;
            self.image_guard = Some(g);
            let mut env = self.env.take().ok_or("environment consumed")?;
            self.child = Some(NativeSuspendedChild::spawn(
                &self.image,
                &mut env,
                self.control_port,
                self.chain_deadline,
            )?);
            Ok(())
        }
        fn validate_identity(&mut self) -> Result<()> {
            let c = self.child.as_ref().ok_or("no process")?;
            c.identity.validate()?;
            if c.thread_id == 0
                || unsafe { GetThreadId(c.thread.0) } != c.thread_id
                || unsafe { GetProcessIdOfThread(c.thread.0) } != c.identity.pid
            {
                return Err("process/thread identity mismatch".into());
            }
            verify_ordinary(c.process.0)?;
            if process_identity(c.process.0)? != c.identity || token_sid(c.process.0)? != self.sid {
                return Err("process identity mismatch".into());
            }
            unsafe {
                let mut b = vec![0u16; 32768];
                let mut n = b.len() as u32;
                check(QueryFullProcessImageNameW(
                    c.process.0,
                    0,
                    b.as_mut_ptr(),
                    &mut n,
                ))?;
                if String::from_utf16(&b[..n as usize]).map_err(|_| "image UTF16")?
                    != self.image.path
                {
                    return Err("actual process image mismatch".into());
                }
            }
            super::super::opencode_native_files::native::verify_image_guard(
                self.image_guard.as_ref().ok_or("image guard missing")?,
                Path::new(&self.image.path),
            )?;
            super::super::opencode_native_files::native::verify_root(&self.root)?;
            Ok(())
        }
        fn assign(&mut self) -> Result<()> {
            self.job
                .assign(self.child.as_ref().ok_or("no process")?.process.0)
        }
        fn flags_and_active(&mut self) -> Result<(u32, u32)> {
            Ok((0x2000, self.job.query()?))
        }
        fn resume(&mut self) -> Result<()> {
            unsafe {
                validate_resume_count(ResumeThread(
                    self.child.as_ref().ok_or("no process")?.thread.0,
                ))?;
            }
            Ok(())
        }
        fn terminate_suspended(&mut self) -> Result<()> {
            unsafe {
                check(TerminateProcess(
                    self.child.as_ref().ok_or("no process")?.process.0,
                    1,
                ))
            }
        }
        fn terminate_job(&mut self) -> Result<()> {
            self.job.terminate()
        }
        fn exited(&mut self) -> Result<bool> {
            self.child.as_ref().ok_or("no process")?.exited()
        }
    }
    impl Drop for NativeProcessOps<'_> {
        fn drop(&mut self) {
            if self
                .child
                .as_ref()
                .is_some_and(|c| c.exited() != Ok(true) || self.job.query() != Ok(0))
            {
                let _ = self.job.terminate();
                if let Some(c) = &self.child {
                    unsafe {
                        TerminateProcess(c.process.0, 1);
                    }
                }
            } /* Fallback only, never a stop receipt. */
        }
    }
    #[derive(Clone, Debug)]
    pub struct GitImageReceipt {
        pub process: ProcessIdentity,
        pub images: Vec<(String, String)>,
        pub all_events_observed: Option<bool>,
    }
    pub struct GitExecution {
        pub preflight: super::super::opencode_paths::GitPreflight,
        pub process: ProcessIdentity,
        pub stdout_eof: bool,
        pub stderr_eof: bool,
    }
    impl GitExecution {
        /// NB4 supplies observer-derived closure facts. No closure exists in NB2 by default.
        pub fn bind_images(
            mut self,
            r: &GitImageReceipt,
        ) -> Result<super::super::opencode_paths::GitPreflight> {
            if r.process != self.process
                || r.all_events_observed != Some(true)
                || r.images.is_empty()
                || !self.stdout_eof
                || !self.stderr_eof
            {
                return Err("Git image/reader coverage UNKNOWN".into());
            }
            self.preflight.images = r.images.clone();
            self.preflight.manifest_complete = true;
            Ok(self.preflight)
        }
    }
    fn reader(
        mut file: File,
        limit: usize,
        overflow: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<OwnedReader> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let handle = std::thread::Builder::new()
            .name("nb2-bounded-reader".into())
            .spawn(move || {
                let mut all = Vec::new();
                let mut b = [0u8; 1024];
                let result = loop {
                    match file.read(&mut b) {
                        Ok(0) => break Ok(all),
                        Ok(n) => {
                            if all.len().checked_add(n).is_none_or(|n| n > limit) {
                                overflow.store(true, std::sync::atomic::Ordering::SeqCst);
                                break Err("Git output bound".into());
                            }
                            all.extend_from_slice(&b[..n]);
                        }
                        Err(_) => break Err("Git reader error".into()),
                    }
                };
                let _ = tx.send(result);
            })
            .map_err(|_| "reader start UNKNOWN")?;
        Ok(OwnedReader {
            rx,
            handle: Some(handle),
            result: None,
        })
    }
    struct OwnedReader {
        rx: std::sync::mpsc::Receiver<Result<Vec<u8>>>,
        handle: Option<std::thread::JoinHandle<()>>,
        result: Option<Result<Vec<u8>>>,
    }
    impl OwnedReader {
        fn finish(&mut self, deadline: Instant) -> Result<Vec<u8>> {
            if self.result.is_none() {
                // Timeout MUST NOT consume/drop the JoinHandle; owner retains it for follow-up.
                self.result = Some(
                    self.rx
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .map_err(|_| "reader EOF deadline UNKNOWN")?,
                );
            }
            if let Some(handle) = &self.handle {
                while !handle.is_finished() {
                    if Instant::now() >= deadline {
                        return Err("reader reclaim UNKNOWN".into());
                    }
                    std::thread::yield_now();
                }
                if self.handle.take().unwrap().join().is_err() {
                    self.result = Some(Err("reader panicked UNKNOWN".into()));
                }
            }
            if Instant::now() >= deadline {
                return Err("reader reclaim late UNKNOWN".into());
            }
            self.result.as_ref().ok_or("reader result UNKNOWN")?.clone()
        }
    }
    pub struct NativeGitIo {
        // Store each reader immediately: a failure starting stderr cannot detach stdout.
        readers: Vec<OwnedReader>,
        overflow: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    impl GitIo<NativeProcessOps<'_>> for NativeGitIo {
        fn start(&mut self, p: &mut NativeProcessOps<'_>) -> Result<()> {
            let (out, err) = p.take_output()?;
            self.readers.push(reader(out, 1024, self.overflow.clone())?);
            self.readers
                .push(reader(err, 32768, self.overflow.clone())?);
            Ok(())
        }
        fn overflow(&self) -> bool {
            self.overflow.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn finish(&mut self, _: &mut NativeProcessOps<'_>, deadline: Instant) -> Result<Vec<u8>> {
            let mut failure = (self.readers.len() != 2)
                .then(|| "Git reader start incomplete UNKNOWN".to_string());
            let mut output = Vec::new();
            // Try BOTH within one deadline; never move either reader out on a failure.
            for (i, r) in self.readers.iter_mut().enumerate() {
                match r.finish(deadline) {
                    Ok(b) if i == 0 => output = b,
                    Ok(_) => {}
                    Err(e) => {
                        if failure.is_none() {
                            failure = Some(e);
                        }
                    }
                }
            }
            if let Some(e) = failure {
                Err(e)
            } else {
                Ok(output)
            }
        }
    }
    pub fn execute_git<'a>(
        job: &'a ProbeJob,
        image: FrozenImage,
        sid: String,
        root: RootReservation,
        origin: Instant,
        plan: DeadlinePlan,
    ) -> std::result::Result<GitExecution, OwnedGitFailure<NativeProcessOps<'a>, NativeGitIo>> {
        let early = |reason| OwnedGitFailure {
            stage: GitStageFailure {
                reason,
                launch: None,
                stop: None,
                readers_confirmed: false,
                cleanup_deadline: None,
                reader_deadline: None,
            },
            resources: None,
        };
        let (chain, _, _) = plan.instants(origin).map_err(&early)?;
        let p =
            NativeProcessOps::git(job, image, sid.clone(), root.clone(), chain).map_err(early)?;
        let io = NativeGitIo {
            readers: Vec::new(),
            overflow: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let (receipt, resources) = supervise_owned_git(p, io, origin, plan)?;
        let facts = (|| -> Result<_> {
            let process = resources
                .process
                .identity()
                .ok_or("Git identity missing")?
                .clone();
            let exit = resources.process.final_exit()?;
            if Instant::now() >= resources.reader_end {
                return Err("final exit query late UNKNOWN".into());
            }
            Ok((process, exit))
        })();
        let (process, exit) = match facts {
            Ok(f) => f,
            Err(reason) => {
                return Err(OwnedGitFailure {
                    stage: GitStageFailure {
                        reason,
                        launch: None,
                        stop: Some(receipt.job),
                        readers_confirmed: true,
                        cleanup_deadline: None,
                        reader_deadline: None,
                    },
                    resources: Some(resources),
                })
            }
        };
        Ok(GitExecution {
            process,
            stdout_eof: true,
            stderr_eof: true,
            preflight: super::super::opencode_paths::GitPreflight {
                attempts: 1,
                exit,
                timed_out: !receipt.natural,
                job: receipt.job,
                stdout: receipt.stdout,
                owner_sid: sid,
                manifest_complete: false,
                images: vec![],
                root_identity: Some(root.identity),
            },
        })
    }

    #[cfg(test)]
    pub(crate) fn helper_fixture(child_wait: bool, env: EnvironmentBlock) -> Result<()> {
        // Only an ignored test calls this. Fixed own built helper, never a caller-selected executable.
        let exe = std::env::current_exe().map_err(|_| "test image")?;
        let helper = exe
            .parent()
            .and_then(|p| p.parent())
            .ok_or("test target layout")?
            .join("nova_t1_m0b_native_helper.exe");
        let cwd = std::env::var("NOVA_NB2_TEST_ROOT")
            .map_err(|_| "explicit own native test root required")?;
        let data = std::fs::read(&helper).map_err(|_| "build fixed helper first")?;
        let _guard = super::super::opencode_native_files::native::guard_image(
            &helper,
            &super::super::hash(&data),
        )?;
        let image = FrozenImage {
            role: LaunchRole::GitPreflight,
            path: helper.to_str().ok_or("helper path")?.into(),
            sha256: super::super::hash(&data),
        };
        let job = ProbeJob::new()?;
        let _serial = SPAWN_SERIAL.lock().map_err(|_| "test serial")?;
        let mode = if child_wait {
            "--child-wait"
        } else {
            "--env-contract"
        };
        let cmd = format!("{} {}", quote_arg(&image.path)?, quote_arg(mode)?);
        let (a, b) = pipe()?;
        let (c, d) = pipe()?;
        let mut env = env;
        let mut sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        let nul = unsafe {
            Handle(CreateFileW(
                wide("NUL").as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &mut sa,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            ))
        };
        if nul.0 == INVALID_HANDLE_VALUE {
            return Err("test NUL".into());
        }
        let mut child = unsafe {
            NativeSuspendedChild::spawn_parts(&image, &mut env, &cmd, a, b, c, d, nul, &cwd)?
        };
        job.assign(child.process.0)?;
        assert_eq!(job.query()?, 1);
        unsafe { assert_eq!(ResumeThread(child.thread.0), 1) }
        let overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut out = reader(child.stdout.take().unwrap(), 1024, overflow.clone())?;
        let mut err = reader(child.stderr.take().unwrap(), 1024, overflow)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        if child_wait {
            while job.query()? < 2 {
                if Instant::now() >= deadline {
                    return Err("delegated helper not observed".into());
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            job.terminate()?;
        }
        while !child.exited()? || job.query()? != 0 {
            if Instant::now() >= deadline {
                return Err("helper exit/Job UNKNOWN".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let reader_end = Instant::now() + Duration::from_secs(1);
        let bytes = out.finish(reader_end)?;
        let error = err.finish(reader_end)?;
        assert!(error.is_empty());
        if !child_wait {
            let j: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| "helper result")?;
            assert_eq!(j["schema"], "NB2_HELPER_V1");
            assert_eq!(j["git_secret_free"], true);
        }
        Ok(())
    }
}
