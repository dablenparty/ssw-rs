use log::{LevelFilter, info};

use crate::logging::init_logging;

mod logging;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Err(e) = init_logging(LevelFilter::Info) {
        eprintln!("Failed to initialize logging: {}", e);
        eprintln!("Debug info: {:?}", e);
        std::process::exit(1);
    }
    info!("Hello, world!");
    Ok(())
}
