use clap::{Parser, Subcommand};

use crate::commands::{BundleArgs, InstallArgs, UninstallArgs};

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
	Install(Box<InstallArgs>),

	/// Uninstall an After Effects plugin.
	Uninstall(UninstallArgs),

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
