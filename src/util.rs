use std::io::{self, Write};

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
