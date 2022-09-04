mod minecraft;

use std::{io::Write, path::PathBuf};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).expect("Missing path to server jar");
    let mut mc_server = minecraft::MinecraftServer::new(dunce::canonicalize(PathBuf::from(path))?);
    let mut proc = mc_server.run()?;
    let stdout = proc.stdout.take().unwrap();
    let server_handle = tokio::spawn(async move {
        let buf = &mut String::new();
        let mut stdout = tokio::io::BufReader::new(stdout);
        loop {
            let n = stdout.read_line(buf).await?;
            if n == 0 {
                break;
            }
            print!("{}", buf);
            std::io::stdout().flush()?;
            buf.clear();
        }
        // this allows the ? operator to work
        Ok::<(), std::io::Error>(())
    });
    match tokio::signal::ctrl_c().await {
        Ok(_) => {
            // not sure if using ? here is a good idea... but hey, this is just an example
            // Rust doesn't stop any child processes when the parent process exits, so we need to do it manually
            println!("Stopping server...");
            let mut stdin = proc.stdin.take().unwrap();
            stdin.write_all(b"stop\n").await?;
            stdin.flush().await?;
        }
        Err(e) => {
            eprintln!("Error: {}", e);
        }
    }
    let exit_status = proc.wait().await?;
    println!("Server exited with status: {}", exit_status);
    server_handle.await??;
    println!("Server stopped");
    Ok(())
}
