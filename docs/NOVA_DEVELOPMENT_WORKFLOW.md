# Nova Development Workflow

## Branch Flow

Current feature branch:

```powershell
feature/nova-technical-mvp
```

Expected review flow:

```text
feature/nova-technical-mvp -> develop -> Claude Code review
```

No merge, release, SDKMAX deploy, or production database access is part of this MVP.

## Commands

Install dependencies:

```powershell
cd D:\nova\apps\desktop
npm install
```

Run desktop app:

```powershell
cd D:\nova\apps\desktop
npm run tauri dev
```

Run tests:

```powershell
cd D:\nova\apps\desktop
npm run test -- --run
cd D:\nova\apps\desktop\src-tauri
cargo test
```

Build production app:

```powershell
cd D:\nova\apps\desktop
npm run tauri -- build
```

The current configuration disables installer bundling. It validates the production desktop executable build without creating a release installer.

## Build Verification and Commit Traceability

`D:\nova\scripts\verify-build.ps1` runs `npm test`, `npm run build`, `cargo test`, and `npm run tauri -- build` for `apps/desktop`, and writes a summary to `D:\nova\test-results\build-info.txt` recording: the exact commit hash and branch under test, `git status --porcelain` output, local `node`/`npm`/`rustc`/`cargo` versions, and each command's exit code.

Workflow:

```powershell
# 1. Finish and commit the change you want verified.
git add <files>
git commit -m "..."

# 2. Verify the committed state (not a dirty working tree).
pwsh -File D:\nova\scripts\verify-build.ps1
```

`build-info.txt` and the `verify-build-*.log` files next to it are regenerable local artifacts, not source — `test-results/` is excluded from version control (see `.gitignore`). Re-running the script after a later commit simply overwrites `build-info.txt` with the new commit hash; it must never be hand-edited or committed, since doing so would create commits whose only purpose is to update a hash reference to itself.

The script exits non-zero if any step fails or a required tool cannot be found, so it is safe to use as a gate (e.g. before opening a PR) rather than only as a report generator.
