use std::{
    fs::create_dir_all,
    io::{self, Write},
    path::{Path, PathBuf},
};

use log::debug;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead},
    select,
};
use tokio_util::sync::CancellationToken;

/// Pipes the given readable to stdout
///
/// # Arguments
///
/// * `readable` - The readable to pipe to stdout
/// * `cancel_token` - An token that can be used to cancel the pipe
///
/// # Errors
///
/// This function will return an error if one occurs while reading a line from the readable or flushing stdout
pub async fn pipe_readable_to_stdout<R>(
    readable: R,
    cancellation_token: CancellationToken,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let buf = &mut String::new();
    let mut reader = tokio::io::BufReader::new(readable);
    loop {
        select! {
            n = reader.read_line(buf) => {
                if n? == 0 {
                    break;
                }
                print!("{}", buf);
                std::io::stdout().flush()?;
                buf.clear();
            }
            _ = cancellation_token.cancelled() => {
                break;
            }
        }
    }
    Ok(())
}

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
    resolved_path
        .parent()
        .unwrap_or_else(|| {
            debug!("failed to get parent of executable");
            Path::new(".")
        })
        .canonicalize()
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
