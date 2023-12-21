use clap::Parser;
use cmd::CommandLineArgs;

mod cmd;

pub fn v3_main() {
    let args = CommandLineArgs::parse();
    #[cfg(debug_assertions)]
    println!("Parsed args: {args:?}");
}
