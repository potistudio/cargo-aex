mod cli;
mod commands;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::Cli;

fn main() -> ExitCode {
	let mut args: Vec<_> = std::env::args_os().collect();

	// Strips the redundant `aex` argument since Cargo invokes the command as `cargo-aex aex <subcommand>`.
	if args.get(1).is_some_and(|arg| arg == "aex") {
		args.remove(1);
	}

	let cli = Cli::parse_from(args);
	commands::run(cli.command)
}
