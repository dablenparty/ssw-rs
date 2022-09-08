mod minecraft;
mod util;

use std::path::PathBuf;

use tokio::io::AsyncBufReadExt;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let path = std::env::args().nth(1).expect("Missing path to server jar");
    let mut mc_server = minecraft::MinecraftServer::new(dunce::canonicalize(PathBuf::from(path))?);
    let mut stdin_reader = tokio::io::BufReader::new(tokio::io::stdin());
    let cargo_version = env!("CARGO_PKG_VERSION");
    println!("SSW Console v{}", cargo_version);
    // TODO: handle commands & errors properly without propagating them
    loop {
        let mut buf = Vec::new();
        stdin_reader.read_until(b'\n', &mut buf).await?;
        let msg = String::from_utf8_lossy(&buf).into_owned();
        let status = *mc_server.status().lock().unwrap();
        if status == minecraft::MCServerState::Running {
            if let Some(sender) = mc_server.get_server_sender() {
                if let Err(err) = sender.send(msg).await {
                    eprintln!("Error sending message to server: {}", err);
                }
            } else {
                eprintln!("Server is running but no sender is available");
            }
        } else {
            let command = msg.trim();
            match command {
                "start" => {
                    if status == minecraft::MCServerState::Stopped {
                        mc_server.run()?;
                    } else {
                        eprintln!("Server is already running");
                    }
                }
                "exit" => {
                    if status == minecraft::MCServerState::Running {
                        if let Some(sender) = mc_server.get_server_sender() {
                            if let Err(err) = sender.send("stop\n".to_string()).await {
                                eprintln!("Error sending message to server: {}", err);
                            }
                        }
                        mc_server.wait_for_exit().await?;
                    }
                    break;
                }
                "help" => {
                    println!("Available commands:");
                    println!("    start - start the server");
                    println!("    exit - exit ssw");
                    println!("    help - show this help message");
                }
                _ => {
                    eprintln!("Unknown command: {}", command);
                }
            }
        }
    }
    Ok(())
}
