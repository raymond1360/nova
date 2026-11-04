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
        let temp = std::env::temp_dir().join(format!("nova-sh1-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&temp).map_err(|_| "TEMP_CREATE")?;
        fs::create_dir(temp.join("hooks")).map_err(|_| "TEMP_CREATE")?;
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(temp.join("empty-config"))
            .map_err(|_| "TEMP_CREATE")?;
        Ok(Self {
            exe,
            _exe_guard: exe_guard,
            temp,
            start: Instant::now(),
            total: 0,
        })
    }
    fn call(&mut self, m: &Manifest, args: &[String]) -> Result<Vec<u8>> {
        self._exe_guard.verify()?;
        if self.start.elapsed() >= Duration::from_secs(30) {
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
        let mut p = c.spawn().map_err(|_| "GIT_START_FAILED")?;
        let overflow = Arc::new(AtomicBool::new(false));
        fn reader<T: Read + Send + 'static>(
            mut r: T,
            cap: usize,
            overflow: Arc<AtomicBool>,
        ) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
            std::thread::spawn(move || {
                let mut out = vec![];
                let mut buf = [0u8; 4096];
                loop {
                    let n = r.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    if out.len() + n > cap {
                        overflow.store(true, Ordering::Release);
                        break;
                    }
                    out.extend_from_slice(&buf[..n]);
                }
                Ok(out)
            })
        }
        let out = reader(
            p.stdout.take().ok_or("GIT_PIPE")?,
            1048576,
            overflow.clone(),
        );
        let err = reader(p.stderr.take().ok_or("GIT_PIPE")?, 32768, overflow.clone());
        let local = Instant::now();
        let mut failure = None;
        let code = loop {
            if overflow.load(Ordering::Acquire)
                || local.elapsed() >= Duration::from_secs(5)
                || self.start.elapsed() >= Duration::from_secs(30)
            {
                failure = Some("GIT_DEADLINE_OR_BOUND");
                let _ = p.kill();
                break p.wait().ok();
            }
            match p.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) => std::thread::sleep(Duration::from_millis(2)),
                Err(_) => {
                    failure = Some("GIT_WAIT_UNKNOWN");
                    let _ = p.kill();
                    break p.wait().ok();
                }
            }
        };
        let o = out
            .join()
            .map_err(|_| "GIT_READ")?
            .map_err(|_| "GIT_READ")?;
        let e = err
            .join()
            .map_err(|_| "GIT_READ")?
            .map_err(|_| "GIT_READ")?;
        if failure.is_some()
            || overflow.load(Ordering::Acquire)
            || self.start.elapsed() >= Duration::from_secs(30)
        {
            return Err("GIT_DEADLINE_OR_BOUND");
        }
        self.total = self
            .total
            .checked_add(o.len() + e.len())
            .ok_or("GIT_BOUND")?;
        if self.total > 8 * 1024 * 1024 {
            return Err("GIT_BOUND");
        }
        if !code.is_some_and(|s| s.success()) {
            return Err("GIT_QUERY_FAILED");
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
                let _ = Guard::open(abs.parent().ok_or("PATH_REJECTED")?, true)?;
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
            let mut g = Git::new()?;
            let e = sample_twice(&mut g, &m, None)?;
            let s = Store::init(root, &m, &e)?;
            s.status(&e)
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
            let mut g = Git::new()?;
            let e = sample_twice(
                &mut g,
                &snap.manifest,
                Some((&snap.base, &snap.base_source)),
            )?;
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
        }
        "status" | "handoff" => {
            if opts.len() != 1 {
                return Err("ARGUMENT_REJECTED");
            }
            let s = Store::open(root)?;
            let snap = s.snapshot()?;
            let mut g = Git::new()?;
            match sample_twice(
                &mut g,
                &snap.manifest,
                Some((&snap.base, &snap.base_source)),
            ) {
                Ok(e) => s.status(&e),
                Err(code) => s.unverified_status(code),
            }
        }
        _ => Err("ARGUMENT_REJECTED"),
    }
}
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let (value, code) = match command(&args) {
        Ok(v) => {
            let status = v.get("status").unwrap_or(&v);
            let unknown = status.get("error_code").is_some()
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
}
