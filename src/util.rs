use std::io::{self, Write};

use tokio::io::{AsyncBufReadExt, AsyncRead};

/// Pipes the given readable to stdout
///
/// # Arguments
///
/// * `readable` - The readable to pipe to stdout
///
/// # Errors
///
/// This function will return an error if one occurs while reading a line from the readable or flushing stdout
pub async fn pipe_readable_to_stdout<R>(readable: R) -> io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let buf = &mut String::new();
    let mut reader = tokio::io::BufReader::new(readable);
    loop {
        let n = reader.read_line(buf).await?;
        if n == 0 {
            break;
        }
        print!("{}", buf);
        std::io::stdout().flush()?;
        buf.clear();
    }
    Ok(())
}
