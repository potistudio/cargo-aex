//! Builds and bundles plugins, then links or copies them into After Effects' plugin directory.

use std::{
	fs,
	path::{Path, PathBuf},
	process::ExitCode,
};

use anyhow::{Context, Result, bail, ensure};
use clap::Args;

use super::{
	bundle::{self, BundleArgs},
	deployment,
};

#[derive(Debug, Args)]
pub struct InstallArgs {
	#[command(flatten)]
	bundle: BundleArgs,

	/// Plugin installation directory; defaults to Adobe's shared MediaCore directory.
	#[arg(long)]
	install_dir: Option<PathBuf>,

	/// Install plugin bundles as independent copies in the installation directory.
	#[arg(long)]
	by_copy: bool,
}

pub fn run(args: InstallArgs) -> ExitCode {
	match install(&args) {
		Ok(()) => ExitCode::SUCCESS,
		Err(error) => {
			eprintln!("error: {error:#}");
			ExitCode::FAILURE
		}
	}
}

fn install(args: &InstallArgs) -> Result<()> {
	let directory = deployment::install_directory(args.install_dir.as_deref())?;
	let extension = deployment::plugin_extension()?;
	let bundles = bundle::bundle(&args.bundle)?;
	// Check every bundle before installing any plugins into the host application.
	for source in &bundles {
		ensure!(
			source.extension().and_then(|value| value.to_str()) == Some(extension),
			"cannot install {} on this host; expected a .{extension} plugin",
			source.display()
		);
		deployment::plugin_metadata(source)?.context("built plugin is missing")?;
		deployment::plugin_metadata(&directory.join(source.file_name().unwrap()))?;
	}
	fs::create_dir_all(&directory).with_context(|| {
		format!(
			"cannot create installation directory {}; check its permissions or specify --install-dir",
			directory.display()
		)
	})?;
	for source in bundles {
		let destination = install_plugin(&source, &directory, args.by_copy).with_context(|| {
			format!(
				"cannot install into {}; check its permissions and close After Effects before retrying",
				directory.display()
			)
		})?;
		println!("Installed {}", destination.display());
	}
	Ok(())
}

/// Stage a link or copy and keep the old installation until publication succeeds.
fn install_plugin(source: &Path, directory: &Path, by_copy: bool) -> Result<PathBuf> {
	let metadata = deployment::plugin_metadata(source)?.context("source plugin is missing")?;
	ensure!(
		!metadata.file_type().is_symlink(),
		"source plugin must not be a symlink: {}",
		source.display()
	);
	let destination = directory.join(source.file_name().context("source plugin has no filename")?);
	let existing = deployment::plugin_metadata(&destination)?;
	let source = fs::canonicalize(source)?;
	ensure!(
		!fs::canonicalize(directory)?.starts_with(&source),
		"installation directory must not be inside source plugin {}",
		source.display()
	);
	if existing
		.as_ref()
		.is_some_and(|metadata| !metadata.file_type().is_symlink())
	{
		ensure!(
			source != fs::canonicalize(&destination)?,
			"source and installation destination are the same: {}",
			destination.display()
		);
	}
	let staging = tempfile::tempdir_in(directory)?;
	let staged = staging.path().join(source.file_name().unwrap());
	if by_copy {
		copy_plugin(&source, &staged)?;
	} else {
		link_plugin(&source, &staged)?;
	}
	let backup = staging.path().join("previous");
	if existing.is_some() {
		fs::rename(&destination, &backup)
			.with_context(|| format!("cannot move previous plugin {}", destination.display()))?;
	}
	if let Err(error) = fs::rename(&staged, &destination) {
		if existing.is_some()
			&& let Err(restore_error) = fs::rename(&backup, &destination)
		{
			// Preserve the backup for recovery if the filesystem also rejects rollback.
			let recovery = staging.keep();
			bail!(
				"cannot publish {}: {error}; cannot restore previous plugin: {restore_error}; backup retained at {}",
				destination.display(),
				recovery.join("previous").display()
			);
		}
		return Err(error).with_context(|| format!("cannot publish {}", destination.display()));
	}
	staging
		.close()
		.context("plugin installed, but cannot clean up staging directory")?;
	Ok(destination)
}

/// Copy the complete bundle, including signature files and executable permissions.
fn copy_plugin(source: &Path, destination: &Path) -> Result<()> {
	let metadata = fs::symlink_metadata(source)?;
	ensure!(
		!metadata.file_type().is_symlink(),
		"refusing to copy symlink {}",
		source.display()
	);
	if metadata.is_dir() {
		fs::create_dir(destination)?;
		for entry in fs::read_dir(source)? {
			let entry = entry?;
			copy_plugin(&entry.path(), &destination.join(entry.file_name()))?;
		}
		fs::set_permissions(destination, metadata.permissions())?;
	} else {
		ensure!(metadata.is_file(), "unsupported file type at {}", source.display());
		fs::copy(source, destination).with_context(|| format!("cannot copy {}", source.display()))?;
	}
	Ok(())
}

/// Use a directory link for macOS bundles and a file link for Windows plugins.
fn link_plugin(source: &Path, destination: &Path) -> Result<()> {
	#[cfg(unix)]
	std::os::unix::fs::symlink(source, destination)?;
	#[cfg(windows)]
	{
		let result = if source.is_dir() {
			std::os::windows::fs::symlink_dir(source, destination)
		} else {
			std::os::windows::fs::symlink_file(source, destination)
		};
		result.context(
			"cannot create symbolic link; enable Windows Developer Mode or run with administrator privileges",
		)?;
	}
	#[cfg(not(any(unix, windows)))]
	bail!("symbolic links are not supported on this platform");
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn copied_windows_plugin_is_independent_of_source() {
		let root = tempfile::tempdir().unwrap();
		let source = root.path().join("Effect.aex");
		let directory = root.path().join("installed");
		fs::create_dir(&directory).unwrap();
		fs::write(&source, b"MZ plugin").unwrap();
		let destination = install_plugin(&source, &directory, true).unwrap();
		assert!(fs::symlink_metadata(&destination).unwrap().is_file());
		fs::write(&source, b"MZ updated plugin").unwrap();
		assert_eq!(fs::read(&destination).unwrap(), b"MZ plugin");
		install_plugin(&source, &directory, true).unwrap();
		fs::remove_file(source).unwrap();
		assert_eq!(fs::read(destination).unwrap(), b"MZ updated plugin");
	}

	#[cfg(unix)]
	#[test]
	fn failed_bundle_copy_preserves_existing_installation() {
		let root = tempfile::tempdir().unwrap();
		let source = root.path().join("Effect.plugin");
		let directory = root.path().join("installed");
		let destination = directory.join("Effect.plugin");
		fs::create_dir(&source).unwrap();
		fs::create_dir_all(&destination).unwrap();
		fs::write(destination.join("original"), "preserved").unwrap();
		std::os::unix::fs::symlink(root.path().join("missing"), source.join("link")).unwrap();
		assert!(install_plugin(&source, &directory, true).is_err());
		assert_eq!(fs::read(destination.join("original")).unwrap(), b"preserved");
		assert_eq!(fs::read_dir(directory).unwrap().count(), 1);
	}

	#[test]
	fn links_windows_plugin_and_replaces_existing_copy_and_dangling_link() {
		let root = tempfile::tempdir().unwrap();
		let source = root.path().join("Effect.aex");
		let directory = root.path().join("installed");
		let destination = directory.join("Effect.aex");
		fs::create_dir(&directory).unwrap();
		fs::write(&source, b"MZ plugin").unwrap();
		fs::write(&destination, b"old copy").unwrap();
		install_plugin(&source, &directory, false).unwrap();
		assert!(fs::symlink_metadata(&destination).unwrap().file_type().is_symlink());
		assert_eq!(fs::read_link(&destination).unwrap(), fs::canonicalize(&source).unwrap());
		fs::write(&source, b"MZ updated plugin").unwrap();
		assert_eq!(fs::read(&destination).unwrap(), b"MZ updated plugin");
		install_plugin(&source, &directory, false).unwrap();
		assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
		assert!(install_plugin(&destination, &directory, false).is_err());
		fs::remove_file(&source).unwrap();
		let target = fs::read_link(&destination).unwrap();
		assert!(install_plugin(&source, &directory, false).is_err());
		assert_eq!(fs::read_link(&destination).unwrap(), target);
		fs::write(&source, b"MZ rebuilt plugin").unwrap();
		install_plugin(&source, &directory, false).unwrap();
		assert_eq!(fs::read(&destination).unwrap(), b"MZ rebuilt plugin");
		// Replace a link whose original target has disappeared.
		fs::remove_file(&destination).unwrap();
		link_plugin(&root.path().join("missing.aex"), &destination).unwrap();
		assert!(!destination.exists());
		install_plugin(&source, &directory, false).unwrap();
		assert_eq!(fs::read(&destination).unwrap(), b"MZ rebuilt plugin");
	}

	#[test]
	fn links_macos_bundle_and_unlinks_without_removing_source() {
		let root = tempfile::tempdir().unwrap();
		let source = root.path().join("Effect.plugin");
		let directory = root.path().join("installed");
		let destination = directory.join("Effect.plugin");
		fs::create_dir(&directory).unwrap();
		fs::create_dir_all(source.join("Contents/MacOS")).unwrap();
		fs::write(source.join("Contents/MacOS/Effect"), "binary").unwrap();
		fs::create_dir_all(&destination).unwrap();
		fs::write(destination.join("stale"), "old copy").unwrap();
		install_plugin(&source, &directory, false).unwrap();
		assert_eq!(fs::read_link(&destination).unwrap(), fs::canonicalize(&source).unwrap());
		assert!(!destination.join("stale").exists());
		install_plugin(&source, &directory, false).unwrap();
		let metadata = deployment::plugin_metadata(&destination).unwrap().unwrap();
		deployment::remove_plugin(&destination, &metadata).unwrap();
		assert!(fs::symlink_metadata(destination).is_err());
		assert_eq!(fs::read(source.join("Contents/MacOS/Effect")).unwrap(), b"binary");
		assert!(install_plugin(&source, &source.join("Contents"), false).is_err());
		assert!(install_plugin(&source, root.path(), false).is_err());
	}

	#[test]
	fn replacing_external_link_preserves_its_target() {
		let root = tempfile::tempdir().unwrap();
		let source = root.path().join("Effect.plugin");
		let external = root.path().join("external.plugin");
		let directory = root.path().join("installed");
		let destination = directory.join("Effect.plugin");
		fs::create_dir(&source).unwrap();
		fs::create_dir(&external).unwrap();
		fs::create_dir(&directory).unwrap();
		fs::write(external.join("original"), "preserved").unwrap();
		link_plugin(&external, &destination).unwrap();
		install_plugin(&source, &directory, false).unwrap();
		assert_eq!(fs::read_link(destination).unwrap(), fs::canonicalize(source).unwrap());
		assert_eq!(fs::read(external.join("original")).unwrap(), b"preserved");
	}
}
