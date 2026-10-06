use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "cargo-aex", bin_name = "cargo aex", version, about = "Aex Cargo subcommands")]
pub struct Cli {
	#[command(subcommand)]
	pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
	/// Bundle an application.
	Bundle,

	/// Install an application.
	Install,

	/// Uninstall an application.
	Uninstall,

	/// Inspect an application.
	Inspect,

	/// Check the development environment.
	Doctor,

	/// Run tests.
	Test,

	/// Run benchmarks.
	Bench,

	/// Print version information.
	Version,
}
