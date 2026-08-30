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
