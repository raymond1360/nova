# Fixed P-only Skip host

Official source: openai/codex commit
`092d3acd6bec3e3a14bdc7e7a2810ab628ab759d`.

In an independently verified copy of that workspace, copy `host.rs` to
`codex-rs/app-server/src/bin/nova_p_only_skip_host.rs` and append
`bin-target.toml` to `codex-rs/app-server/Cargo.toml`. These are the only
entry/bin additions. The frozen workspace declares package version `0.162.1`,
but its Cargo.lock records workspace path packages as `0.0.0`. Apply the included
`workspace-version.patch` to align this packaging metadata with the unchanged
lock. This does not change the official source commit or any dependency version.
Preserve the official Cargo.lock, workspace dependency patches, toolchain and
.cargo configuration. Do not add these dependencies to Nova.

Build from `codex-rs` with explicitly selected Rust 1.95.0:

    cargo +1.95.0-x86_64-pc-windows-msvc build --locked --target x86_64-pc-windows-msvc -p codex-app-server --bin nova-p-only-skip-host

This is a build recipe, not permission to run the host, including help/version.
The first dependency download attempt after metadata alignment failed with
Windows Schannel `CRYPT_E_REVOCATION_OFFLINE`; no compiled host identity is
available. Do not treat this source package as a built or admitted executable,
disable TLS/revocation verification, or infer permission to retry downloads.
The normal host invocation takes no arguments. It preserves the official arg0
auxiliary dispatch; that capability is not a proof of tool isolation. Nova must
bind the exact artifact identity, empty argv, P-only profile, request digest and
fixed eight-frame gate. No previous CLI approval applies to this binary.

Skip bypasses the selected plugin startup tasks, not all plugin/config/auth,
network, cache, trust or tool initialization. Production P remains rejected until
the outstanding initialization facts are independently established. This host
does not authorize model turns, M-stage use or deployment to a consumer path.
