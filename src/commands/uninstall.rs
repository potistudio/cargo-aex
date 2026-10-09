//! Removes selected plugins without invoking a build.

use std::{path::PathBuf, process::ExitCode};

use anyhow::{Context, Result};
use clap::Args;

use super::{
	bundle::{self, PackageArgs},
	deployment,
};

#[derive(Debug, Args)]
pub struct UninstallArgs {
	#[command(flatten)]
	packages: PackageArgs,

	/// Plugin installation directory; defaults to Adobe's shared MediaCore directory.
	#[arg(long)]
	install_dir: Option<PathBuf>,
}

pub fn run(args: UninstallArgs) -> ExitCode {
	match uninstall(&args) {
		Ok(()) => ExitCode::SUCCESS,
		Err(error) => {
			eprintln!("error: {error:#}");
			ExitCode::FAILURE
		}
	}
}

fn uninstall(args: &UninstallArgs) -> Result<()> {
	let directory = deployment::install_directory(args.install_dir.as_deref())?;
	let extension = deployment::plugin_extension()?;
	let names = bundle::plugin_names(&args.packages)?;
	let plugins = names
		.iter()
		.map(|name| {
			let path = directory.join(format!("{name}.{extension}"));
			let metadata = deployment::plugin_metadata(&path)?;
			Ok((path, metadata))
		})
		.collect::<Result<Vec<_>>>()?;
	for (path, metadata) in plugins {
		let Some(metadata) = metadata else {
			println!("Not installed: {}", path.display());
			continue;
		};
		let result = deployment::remove_plugin(&path, &metadata);
		result.with_context(|| {
			format!(
				"cannot uninstall {}; check its permissions and close After Effects before retrying",
				path.display()
			)
		})?;
		println!("Uninstalled {}", path.display());
	}
	Ok(())
}
