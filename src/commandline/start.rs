use std::{io, path::PathBuf};

use clap::Args;
use log::error;

use crate::{
    minecraft::MinecraftServer, run_ssw_event_loop, start_proxy_task, start_stdin_task, SswEvent,
};

#[derive(Debug, Args)]
pub struct StartArgs {
    /// The path to the Minecraft server JAR file.
    #[arg(required = true)]
    server_jar: PathBuf,

    /// Binds the proxy to the specific address.
    #[arg(short, long, value_parser, default_value_t = String::from("0.0.0.0"))]
    proxy_ip: String,

    /// Disables output from the Minecraft server. This only affects the output that is sent to the
    /// console, not the log file. That is managed by the server itself, and not SSW.
    #[arg(short, long)]
    no_mc_output: bool,
}

pub async fn start_main(args: StartArgs) -> io::Result<()> {
    let mut mc_server = MinecraftServer::init(dunce::canonicalize(args.server_jar)?).await;
    mc_server.show_output = !args.no_mc_output;
    if mc_server.ssw_config.mc_version.is_none() {
        if let Err(e) = mc_server.load_version().await {
            error!("failed to load version: {}", e);
        }
    }
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<SswEvent>(100);
    let (proxy_handle, proxy_cancel_token, proxy_tx) = start_proxy_task(
        mc_server.ssw_config.clone(),
        args.proxy_ip,
        mc_server.status(),
        mc_server.subscribe_to_state_changes(),
        event_tx.clone(),
    );
    //? separate cancel token
    let stdin_handle = start_stdin_task(event_tx.clone(), proxy_cancel_token.clone());
    run_ssw_event_loop(
        &mut mc_server,
        proxy_cancel_token,
        (event_tx, event_rx),
        proxy_tx,
    )
    .await;
    stdin_handle.await?;
    proxy_handle.await?;
    Ok(())
}
