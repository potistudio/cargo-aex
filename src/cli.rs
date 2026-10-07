use clap::{Parser, Subcommand};

use crate::commands::BundleArgs;

/// Parsed CLI invocation.
#[derive(Parser)]
#[command(name = "cargo-aex", bin_name = "cargo aex", version, about = "Aex Cargo subcommands")]
pub struct Cli {
	#[command(subcommand)]
	pub command: Command,
}

/// Available subcommands.
#[derive(Subcommand)]
pub enum Command {
	/// Build and bundle After Effects plugins.
	Bundle(Box<BundleArgs>),

	/// Install an After Effects plugin.
	Install,

	/// Uninstall an After Effects plugin.
	Uninstall,

	/// Inspect an After Effects plugin.
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
