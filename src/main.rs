mod minecraft;
mod util;

use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).expect("Missing path to server jar");
    let mut mc_server = minecraft::MinecraftServer::new(dunce::canonicalize(PathBuf::from(path))?);
    mc_server.run()?;
    match tokio::signal::ctrl_c().await {
        Ok(_) => {
            // not sure if using ? here is a good idea... but hey, this is just an example
            // Rust doesn't stop any child processes when the parent process exits, so we need to do it manually
            println!("Type 'stop' to stop the server");
        }
        Err(e) => {
            eprintln!("Error: {}", e);
        }
    }
    mc_server.wait_for_exit().await?;
    Ok(())
}
