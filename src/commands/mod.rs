mod bench;
mod bundle;
mod doctor;
mod inspect;
mod install;
mod test;
mod uninstall;

use std::process::ExitCode;

use clap::CommandFactory;

use crate::cli::{Cli, Command};

pub fn run(command: Command) -> ExitCode {
	match command {
		Command::Bundle => bundle::run(),
		Command::Install => install::run(),
		Command::Uninstall => uninstall::run(),
		Command::Inspect => inspect::run(),
		Command::Doctor => doctor::run(),
		Command::Test => test::run(),
		Command::Bench => bench::run(),
		Command::Version => {
			print!("{}", Cli::command().render_version());
			ExitCode::SUCCESS
		}
	}
}
