# ssw-rs

Simple Server Wrapper (SSW for short) is a Rust CLI that wraps a Minecraft server with simple automation and management features. It is the spiritual successor to my old [SSW for Java](https://github.com/dablenparty/Simple-Server-Wrapper) project, although it is not a direct port.

## Features

The current feature list is as follows:

- Automatically restarts the server after a configurable amount of time, in hours (default: `12.0`)
- Shuts down the server when no players are online for a configurable amount of time, in minutes (default: `5.0`)
- Start the server when a player joins if it is not already running

### Planned

These are features that I plan to implement in the future, but have not yet.

- [ ] Automatically restart the server when it crashes
- [ ] SSW update checker

## Usage

Download the binary for your system (or build it yourself following [these instructions](#building)) and place it somewhere you can easily access. I recommend renaming it to `ssw` (`ssw.exe` on Windows) for ease of use. From there, you can run it with the `--help` (or `-h`) flag to see the available options.

## Building

MSRV: `1.66.0`

It might be lower, but this is what I made it with.
There are no special instructions for building this project. As far as I know, there are also no external dependencies (although I recommend installing `build-essential` on Ubuntu or its equivalent on other systems just to be safe).
