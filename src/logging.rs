use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};

use chrono::{DateTime, Local};
use flate2::{Compression, GzBuilder};
use log::{LevelFilter, SetLoggerError};
use simplelog::{
    ColorChoice, CombinedLogger, ConfigBuilder, LevelPadding, TermLogger, TerminalMode,
    ThreadLogMode, WriteLogger,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SswLoggingError {
    #[error("Failed to rotate logs: {0}")]
    RotateLogs(#[from] io::Error),
    #[error("Failed to initialize logging: {0}")]
    InitLogging(#[from] SetLoggerError),
}

/// Rotates the logs by compressing the current log file and renaming it
/// to include the current date and time. The current log file is then
/// deleted.
fn rotate_logs() -> io::Result<PathBuf> {
    // if we're in debug mode, just make a folder in the current directory
    let log_path = if cfg!(debug_assertions) {
        std::path::PathBuf::from("ssw-logs")
    } else {
        dirs::data_dir().map_or_else(
            || {
                eprintln!("Failed to get data directory, using current directory");
                std::path::PathBuf::from(".")
            },
            |p| p.join("ssw"),
        )
    }
    .join("logs");
    fs::create_dir_all(&log_path)?;

    println!("Logs can be found at: {}", log_path.display());

    let latest_log = log_path.join("latest.log");
    // if the latest log doesn't exist, we don't need to rotate it
    if !latest_log.exists() {
        return Ok(latest_log);
    }
    // get the creation time of the latest log, or use the current time if
    // we can't get the creation time
    let create_time = fs::metadata(&latest_log)?
        .modified()
        .map_or_else(|_| Local::now(), DateTime::<Local>::from);
    let package_name = env!("CARGO_PKG_NAME");
    let dated_name = create_time
        .format(&format!("{package_name}-%Y-%m-%d_%H-%M-%S.log"))
        .to_string();

    // this is where the actual zipping happens
    let archive_path = log_path.join(format!("{dated_name}.gz"));
    let file_handle = fs::File::create(archive_path)?;
    let log_data = fs::read(&latest_log)?;
    let mut gz = GzBuilder::new()
        .filename(dated_name)
        .write(file_handle, Compression::default());
    gz.write_all(&log_data)?;
    gz.finish()?;
    fs::remove_file(&latest_log)?;
    Ok(latest_log)
}

/// Initializes the logging library. The current implementation uses
/// `simplelog` and logs to both the terminal and a file. The terminal
/// log level is set to `min_level` and the file log level is set to `Debug`,
/// unless `min_level` is higher than `Debug` (i.e., `min_level = Trace`),
/// in which case the file log level is set to `min_level`.
///
/// # Arguments
///
/// * `min_level` - The log level to use for the terminal.
pub fn init_logging(min_level: LevelFilter) -> Result<(), SswLoggingError> {
    let config = ConfigBuilder::new()
        .add_filter_allow_str("ssw")
        .set_time_offset_to_local()
        .unwrap_or_else(|c| {
            eprintln!("Failed to determine local time offset, using UTC");
            c
        })
        .set_level_padding(LevelPadding::Right)
        .set_thread_mode(ThreadLogMode::Both)
        .set_target_level(LevelFilter::Error)
        .set_thread_level(LevelFilter::Error)
        .build();
    let term_logger = TermLogger::new(
        min_level,
        config.clone(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    );
    let file_level = if min_level > LevelFilter::Debug {
        min_level
    } else {
        LevelFilter::Debug
    };
    let next_log_file = rotate_logs()?;
    let file_logger = WriteLogger::new(file_level, config, fs::File::create(next_log_file)?);
    CombinedLogger::init(vec![term_logger, file_logger])?;
    Ok(())
}
