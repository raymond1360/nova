//! Debug-only thin adapter: fixed local Git reads and four registration commands.
use nova_lib::scheduler::{
    contract::*,
    store::{self, Guard, Import, Store},
    Result,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

struct Git {
    exe: PathBuf,
    _exe_guard: Guard,
    temp: PathBuf,
    start: Instant,
    total: usize,
    failed: bool,
    cleanup_allowed: bool,
    temp_owner: Option<OwnedTemp>,
}
impl Git {
    fn new() -> Result<Self> {
        let exe = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
            .map(|p| p.join("git.exe"))
            .find(|p| p.is_file())
            .ok_or("GIT_MISSING")?;
        let exe_guard = Guard::tool(&exe)?;
        let exe = exe_guard.path.clone();
        let temp_owner = OwnedTemp::create()?;
        let temp = temp_owner.path.clone();
        Ok(Self {
            exe,
            _exe_guard: exe_guard,
            temp,
            start: Instant::now(),
            total: 0,
            failed: false,
            cleanup_allowed: true,
            temp_owner: Some(temp_owner),
        })
    }
    fn call(&mut self, m: &Manifest, args: &[String]) -> Result<Vec<u8>> {
        if self.failed {
            return Err("GIT_ADMISSION_STOPPED");
        }
        if let Err(e) = self._exe_guard.verify() {
            self.failed = true;
            return Err(e);
        }
        if self.start.elapsed() >= Duration::from_secs(30) {
            self.failed = true;
            return Err("GIT_TOTAL_DEADLINE");
        }
        let mut c = Command::new(&self.exe);
        c.args([
            "--no-pager",
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "core.pager=cat",
            "-c",
            "color.ui=false",
            "-c",
            "diff.external=",
        ])
        .arg("-c")
        .arg(format!(
            "core.hooksPath={}",
            self.temp.join("hooks").display()
        ))
        .arg("-c")
        .arg(format!("core.autocrlf={}", m.eol_policy))
        .arg("-C")
        .arg(&m.repo_path)
        .args(args)
        .env_clear()
        .env(
            "SystemRoot",
            std::env::var_os("SystemRoot").ok_or("SYSTEMROOT_MISSING")?,
        )
        .env(
            "PATH",
            format!(
                "{};{}\\System32",
                self.exe.parent().ok_or("GIT_MISSING")?.display(),
                std::env::var("SystemRoot").map_err(|_| "SYSTEMROOT_MISSING")?
            ),
        )
        .env("TEMP", &self.temp)
        .env("TMP", &self.temp)
        .env("HOME", &self.temp)
        .env("USERPROFILE", &self.temp)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", self.temp.join("empty-config"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x08000000);
        }
        let native = run_git(&c, self.start + Duration::from_secs(30));
        let (o, e) = match native {
            Ok(value) => value,
            Err(failure) => {
                self.failed = true;
                self.cleanup_allowed &= failure.lifecycle_known;
                return Err(failure.code);
            }
        };
        self.total = self
            .total
            .checked_add(o.len() + e.len())
            .ok_or("GIT_BOUND")?;
        if self.total > 8 * 1024 * 1024 {
            self.failed = true;
            return Err("GIT_BOUND");
        }
        Ok(o)
    }
    fn text(&mut self, m: &Manifest, args: &[&str]) -> Result<String> {
        let a = args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        String::from_utf8(self.call(m, &a)?)
            .map(|s| s.trim_end_matches(['\r', '\n']).to_string())
            .map_err(|_| "GIT_ENCODING")
    }
    fn attributes(&mut self, m: &Manifest, paths: &[String]) -> Result<Value> {
        if paths.is_empty() {
            return Ok(json!([]));
        }
        let mut result = vec![];
        for batch in paths.chunks(64) {
            let mut a = vec![
                "check-attr".into(),
                "-z".into(),
                "filter".into(),
                "text".into(),
                "eol".into(),
                "--".into(),
            ];
            a.extend_from_slice(batch);
            let bytes = self.call(m, &a)?;
            let fields = nul(&bytes)?;
            if fields.len() != batch.len() * 9 {
                return Err("ATTR_SCHEMA");
            }
            let mut seen = BTreeSet::new();
            let mut table: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
            for row in fields.chunks(3) {
                if !batch.contains(&row[0])
                    || !["filter", "text", "eol"].contains(&row[1].as_str())
                    || !seen.insert((row[0].clone(), row[1].clone()))
                {
                    return Err("ATTR_SCHEMA");
                }
                table
                    .entry(row[0].clone())
                    .or_default()
                    .insert(row[1].clone(), row[2].clone());
            }
            for path in batch {
                let attrs = table.get(path).ok_or("ATTR_SCHEMA")?;
                let f = attrs.get("filter").ok_or("ATTR_SCHEMA")?;
                let t = attrs.get("text").ok_or("ATTR_SCHEMA")?;
                let e = attrs.get("eol").ok_or("ATTR_SCHEMA")?;
                if !["unspecified", "unset"].contains(&f.as_str()) {
                    return Err("UNSUPPORTED_FILTER");
                }
                if !["unspecified", "unset", "set", "auto"].contains(&t.as_str())
                    || !["unspecified", "unset", "lf", "crlf"].contains(&e.as_str())
                    || (t == "unset" && ["lf", "crlf"].contains(&e.as_str()))
                {
                    return Err("UNSUPPORTED_ATTRIBUTES");
                }
                result.push(json!({"path":path,"filter":f,"text":t,"eol":e}));
            }
        }
        Ok(json!(result))
    }
    fn scope_clean(&mut self, m: &Manifest) -> Result<()> {
        // Only call after attributes for every exact scope path have been accepted.
        let mut a = vec![
            "status".into(),
            "--porcelain=v1".into(),
            "-z".into(),
            "--untracked-files=all".into(),
            "--ignore-submodules=all".into(),
            "--".into(),
        ];
        a.extend_from_slice(&m.allowed_paths);
        // Status can conservatively mark a size/stat-only CRLF change as ` M`
        // while Git's normalized content diff is empty. Never accept that marker
        // alone: index==HEAD was already proved and *both* content diffs below
        // must be empty. Untracked/staged/deleted/conflict entries still reject.
        for row in nul(&self.call(m, &a)?)? {
            if !row.starts_with(" M ")
                || !m
                    .allowed_paths
                    .iter()
                    .any(|p| row.get(3..) == Some(p.as_str()))
            {
                return Err("DIRTY_SCOPE");
            }
        }
        for cached in [false, true] {
            let mut a = vec!["diff".into()];
            if cached {
                a.push("--cached".into())
            }
            a.extend(
                [
                    "--raw",
                    "-z",
                    "--no-abbrev",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-renames",
                    "--ignore-submodules=all",
                ]
                .iter()
                .map(|s| s.to_string()),
            );
            if cached {
                a.push("HEAD".into())
            }
            a.push("--".into());
            a.extend_from_slice(&m.allowed_paths);
            if !self.call(m, &a)?.is_empty() {
                return Err("DIRTY_SCOPE");
            }
        }
        Ok(())
    }
    fn outside(&mut self, m: &Manifest) -> String {
        let attempt = (|| -> Result<String> {
            let all = self.call(
                m,
                &[
                    "ls-files".into(),
                    "-z".into(),
                    "--cached".into(),
                    "--others".into(),
                    "--exclude-standard".into(),
                ],
            )?;
            let names: BTreeSet<_> = nul(&all)?.into_iter().collect();
            if names.len() > 256 || !names.iter().all(|s| relative(s)) {
                return Err("NAME_BOUND_OR_UNSUPPORTED");
            }
            let paths = names.into_iter().collect::<Vec<_>>();
            // Do not compare any outside content until *all* such attributes have passed.
            self.attributes(m, &paths)?;
            for p in &paths {
                let abs = Path::new(&m.repo_path).join(p);
                if abs.exists() {
                    let g = Guard::open(&abs, true)?;
                    if !g.path.is_file() {
                        return Err("OUTSIDE_PATH_UNKNOWN");
                    }
                }
            }
            let mut dirty = false;
            for batch in paths.chunks(64) {
                let mut a = vec![
                    "status".into(),
                    "--porcelain=v1".into(),
                    "-z".into(),
                    "--untracked-files=all".into(),
                    "--ignore-submodules=all".into(),
                    "--".into(),
                ];
                a.extend_from_slice(batch);
                let status = nul(&self.call(m, &a)?)?;
                // Scope is already clean; any nonempty result is a bounded outside notice.
                for row in status {
                    let path = row.get(3..).ok_or("OUTSIDE_STATUS_UNKNOWN")?;
                    if row.as_bytes().get(2) != Some(&b' ') {
                        return Err("OUTSIDE_STATUS_UNKNOWN");
                    }
                    if !m.allowed_paths.iter().any(|p| p == path) {
                        dirty = true;
                    }
                }
            }
            Ok(if dirty {
                "OUTSIDE_SCOPE_DIRTY_NOTICE"
            } else {
                "NO_DIRTY_IN_ENUMERATED_PATHS_IGNORED_FILES_NOT_AUDITED"
            }
            .into())
        })();
        match attempt {
            Ok(s) => s,
            Err(e) => format!("UNKNOWN:{e}"),
        }
    }
    fn sample(&mut self, m: &Manifest, fixed_base: Option<(&str, &str)>) -> Result<Envelope> {
        let rg = Guard::open(Path::new(&m.repo_path), true)?;
        let root = self.text(m, &["rev-parse", "--show-toplevel"])?;
        let observed_root = Guard::open(Path::new(&root), true)?;
        if observed_root.identity != rg.identity {
            return Err("ROOT_MISMATCH");
        }
        if self.text(m, &["rev-parse", "--show-object-format"])? != "sha1"
            || self.text(m, &["rev-parse", "--is-shallow-repository"])? != "false"
        {
            return Err("GIT_REPOSITORY_UNSUPPORTED");
        }
        let git_dir = self.text(m, &["rev-parse", "--absolute-git-dir"])?;
        let gd = Guard::open(Path::new(&git_dir), true)?;
        // ls-files --stage does not expose assume-valid/skip-worktree. Refuse those
        // index forms rather than trusting a status that may intentionally skip content.
        index_flags(&store::read_file(&gd.path.join("index"), 1048576)?.0)?;
        let head = self.text(m, &["rev-parse", "--verify", "HEAD^{commit}"])?;
        let tip = self.text(m, &["rev-list", "--parents", "--max-count=1", &head])?;
        if tip.split_whitespace().collect::<Vec<_>>() == vec![head.as_str()] {
            return Err("ROOT_COMMIT_UNSUPPORTED");
        }
        let parent = self.text(m, &["rev-parse", "--verify", "HEAD^1^{commit}"])?;
        if !hex(&head, 40) || !hex(&parent, 40) {
            return Err("VERSION_UNKNOWN");
        }
        let (base, base_source) = if let Some((b, s)) = fixed_base {
            (b.to_string(), s.to_string())
        } else if m.baseline.mode == "PRE_WORK" {
            (head.clone(), "TOOL_GIT_READ".into())
        } else {
            (
                m.baseline.base_sha.clone().ok_or("BASELINE_REJECTED")?,
                "HUMAN_ATTESTED".into(),
            )
        };
        let obj = self.text(m, &["rev-parse", "--verify", &format!("{base}^{{commit}}")])?;
        if obj != base {
            return Err("VERSION_UNKNOWN");
        }
        let graph = self.text(
            m,
            &[
                "rev-list",
                "--parents",
                "--max-count=257",
                &head,
                &format!("^{base}"),
            ],
        )?;
        let ancestors = chain(&graph, &head, &base)?;
        let raw = self.call(
            m,
            &[
                "diff",
                "--raw",
                "-z",
                "--no-abbrev",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--ignore-submodules=none",
                &base,
                &head,
                "--",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        )?;
        range(&raw, &m.allowed_paths)?;
        let mut args = vec![
            "ls-tree".into(),
            "-r".into(),
            "-z".into(),
            "--full-tree".into(),
            head.clone(),
            "--".into(),
        ];
        args.extend_from_slice(&m.allowed_paths);
        let tree_raw = self.call(m, &args)?;
        let mut tree = BTreeMap::new();
        for rec in nul(&tree_raw)? {
            let (meta, path) = rec.split_once('\t').ok_or("TREE_SCHEMA")?;
            let pieces = meta.split(' ').collect::<Vec<_>>();
            if pieces.len() != 3
                || !["100644", "100755"].contains(&pieces[0])
                || pieces[1] != "blob"
                || !hex(pieces[2], 40)
                || !m.allowed_paths.contains(&path.to_string())
                || tree
                    .insert(
                        path.to_string(),
                        (pieces[0].to_string(), pieces[2].to_string()),
                    )
                    .is_some()
            {
                return Err("TREE_UNSUPPORTED");
            }
        }
        let mut args = vec![
            "ls-files".into(),
            "--stage".into(),
            "-z".into(),
            "--".into(),
        ];
        args.extend_from_slice(&m.allowed_paths);
        let index_raw = self.call(m, &args)?;
        let mut index = BTreeMap::new();
        for rec in nul(&index_raw)? {
            let (meta, path) = rec.split_once('\t').ok_or("INDEX_SCHEMA")?;
            let p = meta.split(' ').collect::<Vec<_>>();
            if p.len() != 3
                || p[2] != "0"
                || index
                    .insert(path.to_string(), (p[0].to_string(), p[1].to_string()))
                    .is_some()
            {
                return Err("INDEX_UNSUPPORTED");
            }
        }
        if tree != index {
            return Err("DIRTY_SCOPE");
        }
        let mut scope_guards = vec![];
        for path in &m.allowed_paths {
            let abs = Path::new(&m.repo_path).join(path);
            if tree.contains_key(path) {
                let guard = Guard::open(&abs, false)?;
                if !guard.path.is_file() {
                    return Err("SCOPE_PATH_UNSUPPORTED");
                }
                scope_guards.push(guard);
            } else {
                scope_guards.push(absent_ancestor(&abs, &rg.path)?);
                match fs::symlink_metadata(&abs) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    _ => return Err("DIRTY_SCOPE"),
                }
            }
        }
        let attrs = self.attributes(m, &m.allowed_paths)?;
        // This is the first worktree-content comparison in this sampling pass.
        self.scope_clean(m)?;
        let mut entries = vec![];
        let mut files = vec![];
        let mut size = 0usize;
        for path in &m.allowed_paths {
            let abs = Path::new(&m.repo_path).join(path);
            if let Some((mode, object)) = tree.get(path) {
                let (bytes, identity) = store::read_file(&abs, 8 * 1024 * 1024)?;
                size = size.checked_add(bytes.len()).ok_or("FILE_BOUND")?;
                if size > 128 * 1024 * 1024 {
                    return Err("FILE_BOUND");
                }
                entries.push(TreeEntry {
                    path: path.clone(),
                    mode: mode.clone(),
                    object: object.clone(),
                });
                files.push(FileFact {
                    path: path.clone(),
                    bytes: bytes.len() as u64,
                    sha256: hash(&bytes),
                    identity,
                });
            } else {
                if fs::symlink_metadata(&abs).is_ok() {
                    return Err("DIRTY_SCOPE");
                }
                let _ = absent_ancestor(&abs, &rg.path)?;
                entries.push(TreeEntry {
                    path: path.clone(),
                    mode: "ABSENT".into(),
                    object: "ABSENT".into(),
                });
            }
        }
        let outside = self.outside(m);
        // Config/attributes are re-read before any repeat content comparison.
        if attrs != self.attributes(m, &m.allowed_paths)? {
            return Err("ATTR_CHANGED");
        }
        self.scope_clean(m)?;
        if head != self.text(m, &["rev-parse", "--verify", "HEAD^{commit}"])? {
            return Err("FILE_CHANGED");
        }
        rg.verify()?;
        gd.verify()?;
        for g in scope_guards {
            g.verify()?;
        }
        let mut e = Envelope {
            head,
            parent,
            base,
            base_source,
            root_identity: rg.identity.clone(),
            git_dir: gd.path.to_string_lossy().into(),
            tree: entries,
            files,
            ancestors,
            attributes: attrs,
            outside_scope_notice: outside,
            observed_utc: chrono::Utc::now().to_rfc3339(),
            delivery_identity: String::new(),
            envelope_id: String::new(),
        };
        e.bind(m)?;
        Ok(e)
    }
}
// Cleanup has its own bounded grace after query admission has failed. No
// unconditional wait/join: unconfirmed termination or pipes remain UNKNOWN.
type Reader = std::thread::JoinHandle<std::io::Result<Vec<u8>>>;
struct RunFailure {
    code: &'static str,
    lifecycle_known: bool,
}
fn reader(mut file: std::fs::File, cap: usize, overflow: Arc<AtomicBool>) -> Reader {
    std::thread::spawn(move || {
        let mut data = vec![];
        let mut buf = [0u8; 4096];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if data.len() + n > cap {
                overflow.store(true, Ordering::Release);
                break;
            }
            data.extend_from_slice(&buf[..n]);
        }
        Ok(data)
    })
}
#[cfg(windows)]
fn run_git(c: &Command, total: Instant) -> std::result::Result<(Vec<u8>, Vec<u8>), RunFailure> {
    let deadline = std::cmp::min(total, Instant::now() + Duration::from_secs(5));
    let mut tree = process_job::Tree::spawn(c, false).map_err(|code| RunFailure {
        code,
        lifecycle_known: false,
    })?;
    let overflow = Arc::new(AtomicBool::new(false));
    let out = reader(
        tree.out.take().ok_or(RunFailure {
            code: "GIT_PIPE",
            lifecycle_known: false,
        })?,
        1048576,
        overflow.clone(),
    );
    let err = reader(
        tree.err.take().ok_or(RunFailure {
            code: "GIT_PIPE",
            lifecycle_known: false,
        })?,
        32768,
        overflow.clone(),
    );
    let (reason, code, stop) = process_job::supervise(&mut tree, deadline, &overflow);
    let streams = collect_readers(out, err, Duration::from_secs(1));
    let known = stop.confirmed() && streams.is_ok();
    if let Some(code) = reason {
        return Err(RunFailure {
            code,
            lifecycle_known: known,
        });
    }
    if !known {
        return Err(RunFailure {
            code: "GIT_LIFECYCLE_UNKNOWN",
            lifecycle_known: false,
        });
    }
    let data = streams.map_err(|code| RunFailure {
        code,
        lifecycle_known: known,
    })?;
    if code != Some(0) {
        return Err(RunFailure {
            code: "GIT_QUERY_FAILED",
            lifecycle_known: known,
        });
    }
    Ok(data)
}
#[cfg(not(windows))]
fn run_git(_: &Command, _: Instant) -> std::result::Result<(Vec<u8>, Vec<u8>), RunFailure> {
    Err(RunFailure {
        code: "WINDOWS_REQUIRED",
        lifecycle_known: true,
    })
}

fn absent_ancestor(abs: &Path, root: &Path) -> Result<Guard> {
    if !store::inside(abs, root) {
        return Err("PATH_REJECTED");
    }
    let remaining = abs
        .strip_prefix(root)
        .map_err(|_| "PATH_REJECTED")?
        .to_str()
        .ok_or("PATH_REJECTED")?
        .replace('\\', "/");
    if !relative(&remaining) {
        return Err("PATH_REJECTED");
    }
    let mut p = abs.parent().ok_or("PATH_REJECTED")?;
    loop {
        match fs::symlink_metadata(p) {
            Ok(_) => {
                let g = Guard::open(p, true)?;
                if !g.path.is_dir() || !store::inside(&g.path, root) {
                    return Err("ABSENT_ANCESTOR_REJECTED");
                }
                return Ok(g);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                p = p.parent().ok_or("PATH_REJECTED")?;
                if !store::inside(p, root) {
                    return Err("PATH_REJECTED");
                }
            }
            Err(_) => return Err("ABSENT_LOOKUP_REJECTED"),
        }
    }
}
fn collect_readers(out: Reader, err: Reader, grace: Duration) -> Result<(Vec<u8>, Vec<u8>)> {
    let start = Instant::now();
    while !out.is_finished() || !err.is_finished() {
        if start.elapsed() >= grace {
            return Err("GIT_STREAM_UNKNOWN");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok((
        out.join()
            .map_err(|_| "GIT_READ")?
            .map_err(|_| "GIT_READ")?,
        err.join()
            .map_err(|_| "GIT_READ")?
            .map_err(|_| "GIT_READ")?,
    ))
}
fn nul(b: &[u8]) -> Result<Vec<String>> {
    if b.is_empty() {
        return Ok(vec![]);
    }
    if b.last() != Some(&0) {
        return Err("NUL_SCHEMA");
    }
    b[..b.len() - 1]
        .split(|b| *b == 0)
        .map(|x| String::from_utf8(x.to_vec()).map_err(|_| "GIT_ENCODING"))
        .collect()
}
fn index_flags(b: &[u8]) -> Result<()> {
    if b.len() < 32 || &b[..4] != b"DIRC" {
        return Err("INDEX_UNSUPPORTED");
    }
    let number = |at: usize| -> Result<u32> {
        Ok(u32::from_be_bytes(
            b.get(at..at + 4)
                .ok_or("INDEX_UNSUPPORTED")?
                .try_into()
                .map_err(|_| "INDEX_UNSUPPORTED")?,
        ))
    };
    if ![2, 3].contains(&number(4)?) {
        return Err("INDEX_UNSUPPORTED");
    }
    let count = number(8)? as usize;
    let mut pos = 12usize;
    if count > 16384 {
        return Err("INDEX_BOUND");
    }
    for _ in 0..count {
        let flag_bytes = b.get(pos + 60..pos + 62).ok_or("INDEX_UNSUPPORTED")?;
        let flags = u16::from_be_bytes(flag_bytes.try_into().map_err(|_| "INDEX_UNSUPPORTED")?);
        if flags & 0xc000 != 0 || number(pos + 24)? & 0o170000 == 0o160000 {
            return Err("INDEX_FLAGS_UNSUPPORTED");
        }
        let end = b
            .get(pos + 62..)
            .ok_or("INDEX_UNSUPPORTED")?
            .iter()
            .position(|v| *v == 0)
            .ok_or("INDEX_UNSUPPORTED")?
            + pos
            + 62;
        pos = pos + ((end + 1 - pos + 7) / 8) * 8;
        if pos > b.len() - 20 {
            return Err("INDEX_UNSUPPORTED");
        }
    }
    Ok(())
}
fn chain(graph: &str, head: &str, base: &str) -> Result<Vec<String>> {
    if graph.lines().count() > 256 {
        return Err("GRAPH_BOUND");
    }
    let mut map = BTreeMap::new();
    for line in graph.lines() {
        let p = line.split_whitespace().collect::<Vec<_>>();
        if p.len() != 2 || !p.iter().all(|s| hex(s, 40)) || map.insert(p[0], p[1]).is_some() {
            return Err("MERGE_RANGE_UNSUPPORTED");
        }
    }
    let mut result = vec![head.to_string()];
    let mut cursor = head;
    while cursor != base {
        cursor = map.remove(cursor).ok_or("VERSION_UNKNOWN")?;
        result.push(cursor.to_string());
        if result.len() > 257 {
            return Err("GRAPH_BOUND");
        }
    }
    if !map.is_empty() {
        return Err("VERSION_UNKNOWN");
    }
    Ok(result)
}
fn range(raw: &[u8], allowed: &[String]) -> Result<()> {
    let rows = nul(raw)?;
    if rows.len() % 2 != 0 {
        return Err("DIFF_SCHEMA");
    }
    for pair in rows.chunks(2) {
        let p = pair[0].split(' ').collect::<Vec<_>>();
        if p.len() != 5
            || !p[0].starts_with(':')
            || !hex(p[2], 40)
            || !hex(p[3], 40)
            || !matches!(p[4], "A" | "M" | "D" | "T")
        {
            return Err("DIFF_SCHEMA");
        }
        if p[0] == ":160000" || p[1] == "160000" {
            return Err("GITLINK_UNSUPPORTED");
        }
        if !relative(&pair[1]) || !allowed.contains(&pair[1]) {
            return Err("SCOPE_EXCEEDED");
        }
    }
    Ok(())
}
fn sample_twice(g: &mut Git, m: &Manifest, base: Option<(&str, &str)>) -> Result<Envelope> {
    let one = g.sample(m, base)?;
    let two = g.sample(m, Some((&one.base, &one.base_source)))?;
    if one.delivery_identity != two.delivery_identity
        || one.files != two.files
        || one.attributes != two.attributes
        || one.git_dir != two.git_dir
    {
        return Err("FILE_CHANGED");
    }
    Ok(two)
}
fn command(args: &[String]) -> Result<Value> {
    if args.len() < 3 {
        return Err("ARGUMENT_REJECTED");
    }
    let verb = &args[0];
    if !["init", "import", "status", "handoff"].contains(&verb.as_str()) {
        return Err("ARGUMENT_REJECTED");
    }
    let mut opts = BTreeMap::new();
    let mut i = 1;
    while i < args.len() {
        let key = &args[i];
        if key == "--index-only" {
            if opts.insert(key.as_str(), "true").is_some() {
                return Err("ARGUMENT_REJECTED");
            }
            i += 1;
            continue;
        }
        if !["--workspace", "--manifest", "--report"].contains(&key.as_str())
            || i + 1 >= args.len()
            || opts.insert(key.as_str(), args[i + 1].as_str()).is_some()
        {
            return Err("ARGUMENT_REJECTED");
        }
        i += 2;
    }
    let root = Path::new(*opts.get("--workspace").ok_or("ARGUMENT_REJECTED")?);
    store::absolute(root)?;
    match verb.as_str() {
        "init" => {
            if opts.len() != 2 {
                return Err("ARGUMENT_REJECTED");
            }
            let mp = Path::new(*opts.get("--manifest").ok_or("ARGUMENT_REJECTED")?);
            store::safe_location(mp)?;
            let m: Manifest = json(&store::read_file(mp, REPORT_LIMIT)?.0)?;
            m.validate()?;
            git_session(|g| {
                let e = sample_twice(g, &m, None)?;
                let s = Store::init(root, &m, &e)?;
                s.status(&e)
            })
        }
        "import" => {
            if opts.len() != 2 + usize::from(opts.contains_key("--index-only")) {
                return Err("ARGUMENT_REJECTED");
            }
            let s = Store::open(root)?;
            let snap = s.snapshot()?;
            let path = Path::new(*opts.get("--report").ok_or("ARGUMENT_REJECTED")?);
            store::safe_location(path)?;
            let guard = Guard::open(path, false)?;
            if !snap
                .manifest
                .report_roots
                .iter()
                .any(|r| store::inside(&guard.path, Path::new(r)))
            {
                return Err("REPORT_ROOT_REJECTED");
            }
            let (bytes, _) = store::read_file(path, REPORT_LIMIT)?;
            let report = if opts.contains_key("--index-only") {
                None
            } else {
                Some(Report::parse(&bytes)?)
            };
            if report.is_none() {
                let e = s.audit_envelope()?;
                let result = s.import(
                    &snap,
                    &e,
                    &Import {
                        path: guard.path.to_string_lossy().into(),
                        bytes,
                        report: None,
                    },
                )?;
                return Ok(
                    json!({"import":result,"projection":"NOT_RECOMPUTED_INDEX_ONLY","execution_permission":"NOT_GRANTED_BY_SH1"}),
                );
            }
            git_session(|g| {
                let e = sample_twice(g, &snap.manifest, Some((&snap.base, &snap.base_source)))?;
                if store::read_file(path, REPORT_LIMIT)?.0 != bytes {
                    return Err("FILE_CHANGED");
                }
                let result = s.import(
                    &snap,
                    &e,
                    &Import {
                        path: guard.path.to_string_lossy().into(),
                        bytes,
                        report,
                    },
                )?;
                Ok(json!({"import":result,"status":s.status(&e)?}))
            })
        }
        "status" | "handoff" => {
            if opts.len() != 1 {
                return Err("ARGUMENT_REJECTED");
            }
            let s = Store::open(root)?;
            let snap = s.snapshot()?;
            git_session(|g| {
                match sample_twice(g, &snap.manifest, Some((&snap.base, &snap.base_source))) {
                    Ok(e) => {
                        if verb == "handoff" {
                            s.handoff(&e)
                        } else {
                            s.status(&e)
                        }
                    }
                    Err(code) => {
                        if verb == "handoff" {
                            s.unverified_handoff(code)
                        } else {
                            s.unverified_status(code)
                        }
                    }
                }
            })
        }
        _ => Err("ARGUMENT_REJECTED"),
    }
}
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let (value, code) = match command(&args) {
        Ok(v) => {
            let status = v.get("status").unwrap_or(&v);
            let unknown = v.get("cleanup_warning").is_some()
                || status.get("error_code").is_some()
                || status.get("version_stale").and_then(Value::as_bool) == Some(true)
                || status
                    .get("report_identity_problem")
                    .and_then(Value::as_bool)
                    == Some(true)
                || (args.first().is_some_and(|a| a == "handoff")
                    && status
                        .pointer("/projection/complete")
                        .and_then(Value::as_bool)
                        != Some(true));
            (v, if unknown { 2 } else { 0 })
        }
        Err(e) => (
            json!({"error_code":e,"execution_permission":"NOT_GRANTED_BY_SH1"}),
            2,
        ),
    };
    let text = serde_json::to_string_pretty(&value)
        .unwrap_or_else(|_| "{\"error_code\":\"ENCODING\"}".into());
    if text.len() > 65536 {
        println!("{{\"error_code\":\"HANDOFF_BOUND\"}}");
        std::process::exit(2)
    }
    println!("{text}");
    std::process::exit(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_graph_bounds_and_merge_rejection() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let c = "c".repeat(40);
        assert_eq!(
            chain(&format!("{b} {a}"), &b, &a).unwrap(),
            vec![b.clone(), a.clone()]
        );
        assert_eq!(
            chain(&format!("{b} {a} {c}"), &b, &a),
            Err("MERGE_RANGE_UNSUPPORTED")
        );
        assert_eq!(chain("", &b, &a), Err("VERSION_UNKNOWN"));
    }
    #[test]
    fn raw_diff_checks_all_paths_without_pathspec() {
        let z = "0".repeat(40);
        let a = "a".repeat(40);
        let raw = format!(":000000 100644 {z} {a} A\0outside.txt\0");
        assert_eq!(
            range(raw.as_bytes(), &["Logo.tsx".into()]),
            Err("SCOPE_EXCEEDED")
        );
    }
    #[test]
    fn blocked_reader_returns_unknown_without_unbounded_join() {
        let (send, receive) = std::sync::mpsc::channel::<()>();
        let blocked = std::thread::spawn(move || {
            let _ = receive.recv();
            Ok(vec![])
        });
        let finished = std::thread::spawn(|| Ok(vec![]));
        assert_eq!(
            collect_readers(blocked, finished, Duration::from_millis(5)),
            Err("GIT_STREAM_UNKNOWN")
        );
        drop(send); // releases only this injected reader, no native subprocess.
        let out = std::thread::spawn(|| Ok(vec![1]));
        let err = std::thread::spawn(|| Ok(vec![]));
        assert_eq!(
            collect_readers(out, err, Duration::from_secs(1)),
            Ok((vec![1], vec![]))
        );
    }
}

#[cfg(windows)]
mod process_job {
    use super::*;
    use std::{
        ffi::OsStr,
        os::windows::{ffi::OsStrExt, io::FromRawHandle},
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::{
        Foundation::*,
        Security::SECURITY_ATTRIBUTES,
        Storage::FileSystem::*,
        System::{JobObjects::*, Pipes::CreatePipe, Threading::*},
    };
    pub struct Handle(pub HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }
    fn quote(s: &OsStr) -> Vec<u16> {
        let mut out = vec![34];
        let mut slash = 0;
        for x in s.encode_wide() {
            if x == 92 {
                slash += 1;
                continue;
            }
            if x == 34 {
                out.extend(std::iter::repeat_n(92, slash * 2 + 1));
                out.push(34)
            } else {
                out.extend(std::iter::repeat_n(92, slash));
                out.push(x)
            }
            slash = 0;
        }
        out.extend(std::iter::repeat_n(92, slash * 2));
        out.push(34);
        out
    }
    fn pipe() -> Result<(Handle, Handle)> {
        unsafe {
            let mut a = null_mut();
            let mut b = null_mut();
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: null_mut(),
                bInheritHandle: 1,
            };
            if CreatePipe(&mut a, &mut b, &sa, 0) == 0 {
                return Err("GIT_PIPE");
            };
            Ok((Handle(a), Handle(b)))
        }
    }
    #[derive(Debug)]
    pub struct StopReceipt {
        pub flags: Option<u32>,
        pub active: Option<u32>,
        pub direct_exited: bool,
    }
    impl StopReceipt {
        pub fn confirmed(&self) -> bool {
            self.flags == Some(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE)
                && self.active == Some(0)
                && self.direct_exited
        }
    }
    pub struct Tree {
        pub out: Option<fs::File>,
        pub err: Option<fs::File>,
        job: Handle,
        process: Handle,
        _stdin: Option<Handle>,
        assigned: bool,
    }
    impl Tree {
        pub fn spawn(c: &Command, hold_stdin: bool) -> Result<Self> {
            unsafe {
                let job = Handle(CreateJobObjectW(null(), null()));
                if job.0.is_null() {
                    return Err("GIT_JOB_CREATE");
                }
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as _,
                    std::mem::size_of_val(&limits) as u32,
                ) == 0
                {
                    return Err("GIT_JOB_LIMIT");
                }
                let (stdout, childout) = pipe()?;
                let (stderr, childerr) = pipe()?;
                let (childin, stdin) = pipe()?;
                for h in [&stdout, &stderr, &stdin] {
                    if SetHandleInformation(h.0, HANDLE_FLAG_INHERIT, 0) == 0 {
                        return Err("GIT_PIPE_FLAGS");
                    }
                }
                let mut size = 0;
                InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
                let mut attr = vec![
                    0usize;
                    (size + std::mem::size_of::<usize>() - 1)
                        / std::mem::size_of::<usize>()
                ];
                let list = attr.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
                if InitializeProcThreadAttributeList(list, 1, 0, &mut size) == 0 {
                    return Err("GIT_ATTRIBUTE");
                }
                struct Attr(LPPROC_THREAD_ATTRIBUTE_LIST);
                impl Drop for Attr {
                    fn drop(&mut self) {
                        unsafe { DeleteProcThreadAttributeList(self.0) }
                    }
                }
                let _attr = Attr(list);
                let handles = [childin.0, childout.0, childerr.0];
                if UpdateProcThreadAttribute(
                    list,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    handles.as_ptr() as _,
                    std::mem::size_of_val(&handles),
                    null_mut(),
                    null(),
                ) == 0
                {
                    return Err("GIT_ATTRIBUTE");
                }
                let mut startup: STARTUPINFOEXW = std::mem::zeroed();
                startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
                startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
                startup.StartupInfo.hStdInput = childin.0;
                startup.StartupInfo.hStdOutput = childout.0;
                startup.StartupInfo.hStdError = childerr.0;
                startup.lpAttributeList = list;
                let app = wide(c.get_program());
                let mut command = quote(c.get_program());
                for a in c.get_args() {
                    command.push(32);
                    command.extend(quote(a))
                }
                command.push(0);
                let mut vars = c
                    .get_envs()
                    .filter_map(|(k, v)| v.map(|v| (k.to_os_string(), v.to_os_string())))
                    .collect::<Vec<_>>();
                vars.sort_by_key(|(k, _)| k.to_string_lossy().to_uppercase());
                let mut env = vec![];
                for (k, v) in vars {
                    env.extend(k.encode_wide());
                    env.push(61);
                    env.extend(v.encode_wide());
                    env.push(0)
                }
                env.push(0);
                if env.len() == 1 {
                    env.push(0)
                }
                let cwd = c.get_current_dir().map(|p| wide(p.as_os_str()));
                let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
                if CreateProcessW(
                    app.as_ptr(),
                    command.as_mut_ptr(),
                    null(),
                    null(),
                    1,
                    CREATE_SUSPENDED
                        | CREATE_NO_WINDOW
                        | EXTENDED_STARTUPINFO_PRESENT
                        | CREATE_UNICODE_ENVIRONMENT,
                    env.as_ptr() as _,
                    cwd.as_ref().map_or(null(), |v| v.as_ptr()),
                    &startup.StartupInfo,
                    &mut pi,
                ) == 0
                {
                    return Err("GIT_START_FAILED");
                }
                let thread = Handle(pi.hThread);
                let mut tree = Self {
                    out: Some(fs::File::from_raw_handle(stdout.0 as _)),
                    err: Some(fs::File::from_raw_handle(stderr.0 as _)),
                    job,
                    process: Handle(pi.hProcess),
                    _stdin: if hold_stdin {
                        Some(stdin)
                    } else {
                        drop(stdin);
                        None
                    },
                    assigned: false,
                };
                std::mem::forget(stdout);
                std::mem::forget(stderr);
                drop(childin);
                drop(childout);
                drop(childerr);
                // Child cannot execute until membership is established. Failed assignment
                // terminates only this suspended process; never resume it.
                if AssignProcessToJobObject(tree.job.0, tree.process.0) == 0 {
                    let _ = tree.stop();
                    return Err("GIT_JOB_ASSIGN_UNKNOWN");
                }
                tree.assigned = true;
                if tree.receipt().flags != Some(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE) {
                    let _ = tree.stop();
                    return Err("GIT_JOB_QUERY_UNKNOWN");
                }
                if ResumeThread(thread.0) == u32::MAX {
                    let _ = tree.stop();
                    return Err("GIT_RESUME_UNKNOWN");
                }
                Ok(tree)
            }
        }
        pub fn receipt(&self) -> StopReceipt {
            unsafe {
                let mut l: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                let mut a: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
                let flags = (QueryInformationJobObject(
                    self.job.0,
                    JobObjectExtendedLimitInformation,
                    &mut l as *mut _ as _,
                    std::mem::size_of_val(&l) as u32,
                    null_mut(),
                ) != 0)
                    .then_some(l.BasicLimitInformation.LimitFlags);
                let active = (QueryInformationJobObject(
                    self.job.0,
                    JobObjectBasicAccountingInformation,
                    &mut a as *mut _ as _,
                    std::mem::size_of_val(&a) as u32,
                    null_mut(),
                ) != 0)
                    .then_some(a.ActiveProcesses);
                StopReceipt {
                    flags,
                    active,
                    direct_exited: WaitForSingleObject(self.process.0, 0) == WAIT_OBJECT_0,
                }
            }
        }
        pub fn stop(&mut self) -> StopReceipt {
            unsafe {
                if self.assigned {
                    TerminateJobObject(self.job.0, 1);
                } else {
                    TerminateProcess(self.process.0, 1);
                }
            }
            let end = Instant::now() + Duration::from_secs(1);
            loop {
                let r = self.receipt();
                if r.confirmed() || Instant::now() >= end {
                    return r;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        fn code(&self) -> Option<u32> {
            if !self.receipt().direct_exited {
                return None;
            }
            let mut code = 0;
            (unsafe { GetExitCodeProcess(self.process.0, &mut code) } != 0).then_some(code)
        }
        #[cfg(test)]
        pub fn members(&self) -> Result<Vec<Handle>> {
            unsafe {
                // Bounded test-only inventory of this unnamed Job, never global names.
                let mut data = [0usize; 66];
                if QueryInformationJobObject(
                    self.job.0,
                    JobObjectBasicProcessIdList,
                    data.as_mut_ptr() as _,
                    std::mem::size_of_val(&data) as u32,
                    null_mut(),
                ) == 0
                {
                    return Err("TEST_JOB_INVENTORY");
                }
                let list = &*(data.as_ptr() as *const JOBOBJECT_BASIC_PROCESS_ID_LIST);
                if list.NumberOfProcessIdsInList > 64 {
                    return Err("TEST_JOB_BOUND");
                }
                let ids = std::slice::from_raw_parts(
                    list.ProcessIdList.as_ptr(),
                    list.NumberOfProcessIdsInList as usize,
                );
                let mut out = vec![];
                for id in ids {
                    let h = Handle(OpenProcess(
                        PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                        0,
                        *id as u32,
                    ));
                    if h.0.is_null() {
                        return Err("TEST_PROCESS_HANDLE");
                    }
                    let mut member = 0;
                    if IsProcessInJob(h.0, self.job.0, &mut member) == 0 || member == 0 {
                        return Err("TEST_MEMBERSHIP");
                    }
                    out.push(h)
                }
                Ok(out)
            }
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            if !self.receipt().confirmed() {
                let _ = self.stop();
            }
        }
    }
    pub fn supervise(
        t: &mut Tree,
        deadline: Instant,
        overflow: &AtomicBool,
    ) -> (Option<&'static str>, Option<u32>, StopReceipt) {
        loop {
            let r = t.receipt();
            if r.flags != Some(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE) || r.active.is_none() {
                return (Some("GIT_WAIT_UNKNOWN"), None, t.stop());
            }
            if r.confirmed() {
                return (None, t.code(), r);
            }
            if Instant::now() >= deadline || overflow.load(Ordering::Acquire) {
                return (Some("GIT_DEADLINE_OR_BOUND"), t.code(), t.stop());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    // Handle-based disposition: never recursively delete a substituted pathname.
    pub fn delete_owned(path: &Path, identity: &str, dir: bool) -> Result<()> {
        unsafe {
            let w = wide(path.as_os_str());
            let h = Handle(CreateFileW(
                w.as_ptr(),
                GENERIC_READ | DELETE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                null_mut(),
            ));
            if h.0 == INVALID_HANDLE_VALUE {
                return Err("TEMP_CLEANUP_OPEN");
            }
            let mut i: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
            let mut b = vec![0u16; 32768];
            let n = GetFinalPathNameByHandleW(h.0, b.as_mut_ptr(), b.len() as u32, 0) as usize;
            if GetFileInformationByHandle(h.0, &mut i) == 0
                || i.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
                || n == 0
                || n >= b.len()
            {
                return Err("TEMP_CLEANUP_IDENTITY");
            }
            let actual = format!(
                "{:08x}:{:08x}{:08x}",
                i.dwVolumeSerialNumber, i.nFileIndexHigh, i.nFileIndexLow
            );
            let final_path = String::from_utf16(&b[..n]).map_err(|_| "TEMP_CLEANUP_IDENTITY")?;
            if actual != identity
                || !final_path
                    .trim_start_matches("\\\\?\\")
                    .eq_ignore_ascii_case(&path.to_string_lossy())
                || (!dir && i.nNumberOfLinks != 1)
            {
                return Err("TEMP_CLEANUP_IDENTITY");
            }
            if dir
                && fs::read_dir(path)
                    .map_err(|_| "TEMP_CLEANUP_READ")?
                    .next()
                    .is_some()
            {
                return Err("TEMP_CLEANUP_NOT_EMPTY");
            }
            let info = FILE_DISPOSITION_INFO { DeleteFile: 1 };
            if SetFileInformationByHandle(
                h.0,
                FileDispositionInfo,
                &info as *const _ as _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
            {
                return Err("TEMP_CLEANUP_DELETE");
            }
            Ok(())
        }
    }
}

struct OwnedTemp {
    path: PathBuf,
    root: Option<Guard>,
    hooks: Option<Guard>,
    config: Option<Guard>,
}
impl OwnedTemp {
    fn create() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("nova-sh1-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).map_err(|_| "TEMP_CREATE")?;
        let mut owner = Self {
            root: Some(Guard::open(&path, true)?),
            path,
            hooks: None,
            config: None,
        };
        let setup = (|| {
            fs::create_dir(owner.path.join("hooks")).map_err(|_| "TEMP_CREATE")?;
            owner.hooks = Some(Guard::open(&owner.path.join("hooks"), false)?);
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(owner.path.join("empty-config"))
                .map_err(|_| "TEMP_CREATE")?;
            owner.config = Some(Guard::open(&owner.path.join("empty-config"), false)?);
            Ok(())
        })();
        if let Err(e) = setup {
            eprintln!("TEMP_SETUP_PARTIAL_RETAINED");
            return Err(e);
        }
        Ok(owner)
    }
    #[cfg(windows)]
    fn cleanup(&mut self) -> Result<()> {
        // Root's existing guard prevents parent substitution while children are
        // checked/deleted. Each child is reopened with DELETE and its original ID.
        self.root.as_ref().ok_or("TEMP_CLEANUP_UNKNOWN")?.verify()?;
        for child in [&mut self.config, &mut self.hooks] {
            if let Some(g) = child.take() {
                g.verify()?;
                let p = g.path.clone();
                let id = g.identity.clone();
                let dir = p.file_name() == Some(std::ffi::OsStr::new("hooks"));
                drop(g);
                process_job::delete_owned(&p, &id, dir)?;
            }
        }
        let g = self.root.take().ok_or("TEMP_CLEANUP_UNKNOWN")?;
        let p = g.path.clone();
        let id = g.identity.clone();
        drop(g);
        process_job::delete_owned(&p, &id, true)
    }
    #[cfg(not(windows))]
    fn cleanup(&mut self) -> Result<()> {
        Err("WINDOWS_REQUIRED")
    }
}
impl Git {
    fn finish(&mut self) -> Result<()> {
        if !self.cleanup_allowed {
            return Err("TEMP_RETAINED_LIFECYCLE_UNKNOWN");
        }
        if let Some(mut owner) = self.temp_owner.take() {
            owner.cleanup()?
        }
        Ok(())
    }
}
impl Drop for Git {
    fn drop(&mut self) {
        if self.temp_owner.is_some() {
            if let Err(code) = self.finish() {
                eprintln!("{code}");
            }
        }
    }
}
fn git_session(f: impl FnOnce(&mut Git) -> Result<Value>) -> Result<Value> {
    let mut g = Git::new()?;
    let result = f(&mut g);
    let cleanup = g.finish();
    match (result, cleanup) {
        (Ok(mut v), Err(e)) => {
            v["cleanup_warning"] = json!(e);
            Ok(v)
        }
        (result, Err(e)) => {
            eprintln!("{e}");
            result
        }
        (result, Ok(())) => result,
    }
}

#[cfg(all(test, windows))]
mod remediation_tests {
    use super::*;
    fn scratch() -> PathBuf {
        let root = std::env::var_os("NOVA_SH1_TEST_ROOT")
            .map(PathBuf::from)
            .expect("self-owned test root required")
            .join(format!("adapter-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        root
    }
    #[test]
    fn absent_nested_path_guards_existing_ancestor_without_creating_directories() {
        let root = scratch();
        let target = root.join("new-dir/deeper/file.txt");
        let g = absent_ancestor(&target, &root).unwrap();
        assert_eq!(g.path, root);
        assert!(!root.join("new-dir").exists());
        assert!(absent_ancestor(&root.parent().unwrap().join("outside"), &root).is_err());
    }
    #[test]
    fn owned_temp_normal_cleanup_and_unknown_children_are_not_recursive() {
        let mut t = OwnedTemp::create().unwrap();
        let p = t.path.clone();
        t.cleanup().unwrap();
        assert!(!p.exists());
        let mut t = OwnedTemp::create().unwrap();
        let p = t.path.clone();
        fs::write(p.join("unknown"), b"retain").unwrap();
        assert_eq!(t.cleanup(), Err("TEMP_CLEANUP_NOT_EMPTY"));
        assert_eq!(fs::read(p.join("unknown")).unwrap(), b"retain");
    }
    #[test]
    fn substituted_temp_entry_is_never_deleted() {
        let mut t = OwnedTemp::create().unwrap();
        let p = t.path.join("empty-config");
        t.config.take();
        fs::rename(&p, t.path.join("original")).unwrap();
        fs::write(&p, b"other-owner").unwrap();
        assert!(t.cleanup().is_err());
        assert_eq!(fs::read(p).unwrap(), b"other-owner");
    }
    #[test]
    fn lifecycle_unknown_retains_owned_temp_and_latches_admission() {
        let mut g = Git::new().unwrap();
        let p = g.temp.clone();
        g.failed = true;
        g.cleanup_allowed = false;
        assert_eq!(g.finish(), Err("TEMP_RETAINED_LIFECYCLE_UNKNOWN"));
        assert!(p.exists());
        assert!(g.failed);
    }
    #[test]
    fn actual_git_launcher_tree_timeout_reclaims_job_children_and_pipe_readers() {
        use windows_sys::Win32::{
            Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject,
        };
        let root = scratch();
        let mut g = Git::new().unwrap();
        let mut c = Command::new(&g.exe);
        // Test-only non-repository command. Intentionally retain pipe stdin so the
        // installed launcher and its actual Git child cannot finish naturally.
        c.args(["--no-pager", "hash-object", "--stdin"])
            .current_dir(&root)
            .env_clear()
            .env("SystemRoot", std::env::var_os("SystemRoot").unwrap())
            .env(
                "PATH",
                format!(
                    "{};{}\\System32",
                    g.exe.parent().unwrap().display(),
                    std::env::var("SystemRoot").unwrap()
                ),
            )
            .env("HOME", &g.temp)
            .env("USERPROFILE", &g.temp)
            .env("TEMP", &g.temp)
            .env("TMP", &g.temp)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", g.temp.join("empty-config"))
            .env("GIT_TERMINAL_PROMPT", "0");
        let mut tree = process_job::Tree::spawn(&c, true).unwrap();
        let overflow = Arc::new(AtomicBool::new(false));
        let out = reader(tree.out.take().unwrap(), 1048576, overflow.clone());
        let err = reader(tree.err.take().unwrap(), 32768, overflow.clone());
        let end = Instant::now() + Duration::from_secs(3);
        let (members, observed_images) = loop {
            let members = tree.members().unwrap();
            let mut images = vec![];
            let mut delegated_git = false;
            for h in &members {
                let mut name = vec![0u16; 32768];
                let mut n = name.len() as u32;
                assert_ne!(
                    unsafe {
                        windows_sys::Win32::System::Threading::QueryFullProcessImageNameW(
                            h.0,
                            0,
                            name.as_mut_ptr(),
                            &mut n,
                        )
                    },
                    0
                );
                let image = String::from_utf16(&name[..n as usize]).unwrap();
                delegated_git |= Path::new(&image)
                    .file_name()
                    .is_some_and(|n| n.eq_ignore_ascii_case("git.exe"))
                    && !image.eq_ignore_ascii_case(&g.exe.to_string_lossy());
                images.push(json!({"pid":unsafe{windows_sys::Win32::System::Threading::GetProcessId(h.0)},"image_observed":image}));
            }
            // A second Job member may only be conhost. Require the actual
            // delegated git.exe image before exercising the timeout.
            if delegated_git {
                break (members, images);
            }
            assert!(
                Instant::now() < end,
                "installed launcher did not produce delegated git.exe"
            );
            std::thread::sleep(Duration::from_millis(2));
        };
        let before = tree.receipt();
        assert!(before.active.unwrap() >= 2);
        let clock = Instant::now();
        let (reason, _, receipt) =
            process_job::supervise(&mut tree, clock + Duration::from_millis(75), &overflow);
        assert_eq!(reason, Some("GIT_DEADLINE_OR_BOUND"));
        assert!(receipt.confirmed(), "{receipt:?}");
        let signal_deadline = Instant::now() + Duration::from_secs(1);
        for h in &members {
            assert_eq!(
                unsafe {
                    WaitForSingleObject(
                        h.0,
                        signal_deadline
                            .saturating_duration_since(Instant::now())
                            .as_millis()
                            .min(1000) as u32,
                    )
                },
                WAIT_OBJECT_0
            );
        }
        let streams = collect_readers(out, err, Duration::from_secs(1)).unwrap();
        assert!(clock.elapsed() < Duration::from_secs(3));
        fs::write(root.join("job-timeout-receipt.json"),serde_json::to_vec_pretty(&json!({"source":"WINDOWS_NATIVE_FIXED_GIT_FIXTURE","job_flags":receipt.flags,"active_before":before.active,"active_after":receipt.active,"direct_exited":receipt.direct_exited,"observed_job_process_handles":members.len(),"observed_images":observed_images,"delegated_git_observed":true,"all_observed_processes_signaled":true,"stdout_eof_joined":true,"stderr_eof_joined":true,"stdout_bytes":streams.0.len(),"stderr_bytes":streams.1.len(),"elapsed_ms":clock.elapsed().as_millis(),"identity_boundary":"guarded launcher only; downstream image identity not frozen"})).unwrap()).unwrap();
        drop(tree);
        g.finish().unwrap();
    }
}
