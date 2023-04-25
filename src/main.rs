use log::{info, LevelFilter};

use crate::logging::init_logging;

mod logging;

#[tokio::main]
async fn main() {
    if let Err(e) = init_logging(LevelFilter::Info) {
        eprintln!("Failed to initialize logging: {}", e);
        eprintln!("Debug info: {:?}", e);
        std::process::exit(1);
    }
    info!("Hello, world!");
}
