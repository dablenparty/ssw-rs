mod minecraft;

use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).expect("Missing path to server jar");
    let mut mc_server = minecraft::MinecraftServer::new(PathBuf::from(path));
    let mut proc = mc_server.run()?;
    let exit_status = proc.wait().await?;
    println!("Server exited with status: {}", exit_status);
    Ok(())
}
