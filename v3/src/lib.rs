use clap::Parser;
use commandline::CommandLineArgs;

mod commandline;

pub fn v3_main() {
    let args = CommandLineArgs::parse();
    #[cfg(debug_assertions)]
    println!("Parsed args: {args:?}");
    // TODO: init logging
    // TODO: refresh manifest if requested
    //? consider checking if the manifest is up to date and refreshing if it isn't
    //? this can be done very simply by checking the last modified time of the manifest
    //? and comparing it to the current time

    // EVERYTHING GETS A DEDICATED TASK!
    // LITERALLY EVERYTHING
    // This is basically microservices but for a single program
    // EXAMPLE: Task1 takes user input and sends it to Task2 which parses it as a command
    // and then sends said command to Task3 which executes it and sends the result to
    // Task4 which does whatever with it, you get the idea.

    // TODO: create async task management system
    //* each task owns its own CancellationToken, JoinHandle, and optional name
    //* tasks track any child tasks they spawn, propagating cancellation
    //* channels should be optional. if structs can hold a generic channel
    //* type, then they can be used with both mpsc, oneshot, etc. channels

    // TODO: create command system
    //* commands are async tasks that can be run by the user
    //* each command has a name, description, args, and function (consider a macro for this)
}
