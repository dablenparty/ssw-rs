use std::{env, fs, io, process::Command};

fn main() -> io::Result<()> {
    const PACKAGE_NAME: &str = env!("CARGO_PKG_NAME");
    const CARGO_BUILD_ARGS: &[&str] = &["build", "--release", "--bin", PACKAGE_NAME];
    let build_status = Command::new("cargo").args(CARGO_BUILD_ARGS).status()?;
    if !build_status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, "Build failed"));
    }
    println!("cargo build exited with status: {}", build_status);
    let old_target = format!("target/release/{}{}", PACKAGE_NAME, env::consts::EXE_SUFFIX);
    let new_target = format!(
        "target/release/{}_{}_{}{}",
        PACKAGE_NAME,
        env::consts::OS,
        env::consts::ARCH,
        env::consts::EXE_SUFFIX
    );
    println!("Renaming {} to {}", old_target, new_target);
    fs::rename(old_target, new_target)?;
    Ok(())
}
