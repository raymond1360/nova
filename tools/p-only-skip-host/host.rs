//! Fixed P-probe host for official Codex commit 092d3acd6bec3e3a14bdc7e7a2810ab628ab759d.
//! Build only; running this artifact requires a separate approval.
#![recursion_limit = "256"]

#[cfg(not(windows))]
compile_error!("Nova P-only Skip host requires Windows");
#[cfg(not(debug_assertions))]
compile_error!("Nova P-only Skip host requires a debug build");

use codex_app_server::{
    AppServerExit, AppServerRuntimeOptions, AppServerTransport, CodeModeHostTransport,
    PluginStartupTasks, RemoteControlStartupMode, run_main_with_transport_options,
};
use codex_arg0::arg0_dispatch_or_else;
use codex_config::LoaderOverrides;
use codex_protocol::protocol::SessionSource;
use codex_utils_cli::CliConfigOverrides;
use codex_websocket_auth::WebsocketAuthArgs;

fn main() -> anyhow::Result<()> {
    // Preserve the official auxiliary dispatch and runtime/PATH lifetime contract.
    // Nova's P launcher binds an empty argv; it never invokes auxiliary modes.
    arg0_dispatch_or_else(|arg0_paths| async move {
        if std::env::args_os().len() != 1 {
            anyhow::bail!("P_HOST_ARGUMENTS_REJECTED");
        }
        let exit = run_main_with_transport_options(
            arg0_paths,
            CliConfigOverrides::default(),
            // No debug managed-config replacement or ignore hook.
            LoaderOverrides::default(),
            true,
            false,
            AppServerTransport::Stdio,
            SessionSource::VSCode,
            WebsocketAuthArgs::default().try_into_settings()?,
            AppServerRuntimeOptions {
                code_mode_host_transport: CodeModeHostTransport::Local,
                plugin_startup_tasks: PluginStartupTasks::Skip,
                remote_control_startup_mode: RemoteControlStartupMode::DisabledEphemeral,
                install_shutdown_signal_handler: true,
                managed_daemon: false,
            },
        )
        .await?;
        if exit == AppServerExit::Forced {
            // Match official standalone main: runtime teardown may block on rollout I/O.
            std::process::exit(0);
        }
        Ok(())
    })
}
