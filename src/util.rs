use std::{
    cmp::min,
    fs::create_dir_all,
    io,
    path::{Path, PathBuf},
};

use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use lazy_static::lazy_static;
use log::{debug, warn};
use regex::bytes::Regex;
use tokio::io::AsyncWriteExt;

use crate::ssw_error;

/// Gets the path to the directory containing the executable, resolving symlinks if necessary.
///
/// The current working directory is not used by default because the executable may be running from a
/// different directory than the one containing said executable. This is especially important for
/// logging and/or data storage, where having a constant directory is useful.
///
/// # Errors
/// There are several errors that may occur in this process, although they are all handled.
///
/// If an error occurs...
/// - when attempting to get the executable path, the executable is taken from the command line args.
/// - when attempting to get the executable from the command line args, a default value is used.
///     - `.\{CARGO_PKG_NAME}.exe` on Windows, `./{CARGO_PKG_NAME}` on Unix
/// - when resolving a symlink, the original symlink path is returned.
/// - when attempting to get the parent of the executable path, the current working directory is
/// used.
/// - when attempting to canonicalize the final parent path, the current working directory is used as `.`.
///
/// returns: `PathBuf`
pub fn get_exe_parent_dir() -> PathBuf {
    let initial_path = std::env::current_exe().unwrap_or_else(|_| {
        debug!("failed to get current executable path");
        let exec_name = std::env::args().next().unwrap_or_else(|| {
            debug!("failed to get executable name from args");
            let dummy_name = env!("CARGO_PKG_NAME");
            if cfg!(windows) {
                format!(".\\{}.exe", dummy_name)
            } else {
                format!("./{}", dummy_name)
            }
        });
        PathBuf::from(exec_name)
    });
    // is_symlink also checks for existence and permissions, so we don't need to do that here
    let resolved_path = if initial_path.is_symlink() {
        initial_path.read_link().unwrap_or_else(|_| {
            debug!("failed to read link {}", initial_path.display());
            initial_path
        })
    } else {
        initial_path
    };
    dunce::canonicalize(resolved_path.parent().unwrap_or_else(|| {
        debug!("failed to get parent of executable");
        Path::new(".")
    }))
    .unwrap_or_else(|e| {
        debug!(
            "failed to canonicalize path {}: {}",
            resolved_path.display(),
            e
        );
        resolved_path
    })
}

/// Synchronously creates a directory if it does not exist, failing if some other error occurs
///
/// # Arguments
///
/// * `file_path`: the path to the directory
///
/// returns: ()
pub fn create_dir_if_not_exists(dir_path: &Path) -> io::Result<()> {
    if let Err(e) = create_dir_all(dir_path) {
        if e.kind() == io::ErrorKind::AlreadyExists {
            debug!("directory {} already exists, skipping", dir_path.display());
            Ok(())
        } else {
            Err(e)
        }
    } else {
        Ok(())
    }
}

/// Asynchronously creates a directory if it does not exist, failing if some other error occurs
///
/// # Arguments
///
/// * `file_path`: the path to the directory
///
/// returns: ()
pub async fn async_create_dir_if_not_exists(dir_path: &Path) -> io::Result<()> {
    if let Err(e) = tokio::fs::create_dir_all(dir_path).await {
        if e.kind() == io::ErrorKind::AlreadyExists {
            debug!("directory {} already exists, skipping", dir_path.display());
            Ok(())
        } else {
            Err(e)
        }
    } else {
        Ok(())
    }
}

/// Runs the given Java executable with the `-version` flag and parses the output to get the version
/// number.
///
/// # Arguments
///
/// * `java_executable`: the path to the Java executable
///
/// # Errors
///
/// If the Java executable fails to run, the output cannot be parsed, or the version string is not found
/// in the output, an error is returned.
pub async fn get_java_version(java_executable: &Path) -> io::Result<String> {
    lazy_static! {
        static ref JAVA_VERSION_REGEX: Regex =
            Regex::new(r#"^(\w+) version "(?P<version>\d+\.\d+\.\d+)(_\d+)?""#).unwrap();
    }
    let output = tokio::process::Command::new(java_executable)
        .arg("-version")
        .output()
        .await?;
    let version = match JAVA_VERSION_REGEX.captures(&output.stderr) {
        Some(captures) => String::from_utf8_lossy(
            captures
                .name("version")
                .map_or("17.0".as_bytes(), |v| v.as_bytes()),
        ),

        None => {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "failed to get java version from stderr".to_string(),
            ))
        }
    };
    Ok(version.to_string())
}

/// Converts a `Path` to a `&str` if it is valid UTF-8, otherwise returns an error.
///
/// # Arguments
///
/// * `path`: the path to convert
///
/// # Errors
///
/// If the path is not valid UTF-8, an error is returned.
pub fn path_to_str(path: &Path) -> io::Result<&str> {
    path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("path '{}' is not valid UTF-8", path.display()),
        )
    })
}

/// Downloads a file from the given URL to the given destination path, displaying a progress bar.
///
/// # Arguments
///
/// * `url`: the URL to download from
/// * `dest`: the path to download to. If the files exists, it is overwritten.
pub async fn download_file_with_progress(url: &str, dest: &Path) -> ssw_error::Result<()> {
    if dest.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} is a directory", dest.display()),
        )
        .into());
    }
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    async_create_dir_if_not_exists(parent).await?;
    debug!("downloading {} to {}", url, dest.display());
    let response = reqwest::get(url).await?.error_for_status()?;
    let total_size = response.content_length().unwrap_or_else(|| {
        warn!("failed to get content length for {}", url);
        0
    });
    debug!("total size: {} bytes", total_size);
    let mut file_handle = tokio::fs::File::create(&dest).await?;
    let mut bytes_downloaded = 0u64;
    let mut stream = response.bytes_stream();

    let pb = ProgressBar::new(total_size);
    pb.set_style(ProgressStyle::default_bar()
        .template("{msg}\n{spinner:.green} [{elapsed}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
        .unwrap()
        .progress_chars("=>-"));
    let file_name = dest.file_name().unwrap();
    pb.set_message(format!("Downloading {}", file_name.to_string_lossy()));

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file_handle.write_all(&chunk).await?;
        let chunk_size = chunk.len() as u64;
        // sometimes, the chunk size is slightly wrong, so we need to clamp the downloaded bytes to the
        // server size; although, it's ok for a general progress bar.
        bytes_downloaded = min(
            total_size,
            bytes_downloaded.checked_add(chunk_size).unwrap_or_else(|| {
                warn!("Downloaded more than 2^128 bytes of data. This is probably a bug.");
                bytes_downloaded
            }),
        );
        pb.set_position(bytes_downloaded);
    }
    pb.finish_with_message(format!("Downloaded {} to {}", url, dest.display()));
    Ok(())
}
