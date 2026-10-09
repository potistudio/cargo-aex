//! Shared installation paths and filesystem checks.

use std::{
	fs,
	io::ErrorKind,
	path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};

/// Native plugin extension; installing cross-compiled plugins is not supported.
pub(super) fn plugin_extension() -> Result<&'static str> {
	if cfg!(target_os = "macos") {
		Ok("plugin")
	} else if cfg!(target_os = "windows") {
		Ok("aex")
	} else {
		bail!("plugin installation and removal require macOS or Windows")
	}
}

/// Use Adobe's shared MediaCore directory unless the caller supplies another location.
pub(super) fn install_directory(requested: Option<&Path>) -> Result<PathBuf> {
	plugin_extension()?;
	if let Some(path) = requested {
		return Ok(path.to_path_buf());
	}
	if cfg!(target_os = "macos") {
		Ok(PathBuf::from(
			"/Library/Application Support/Adobe/Common/Plug-ins/7.0/MediaCore",
		))
	} else {
		let program_files = std::env::var_os("ProgramW6432")
			.or_else(|| std::env::var_os("ProgramFiles"))
			.context("cannot locate Program Files; specify --install-dir")?;
		Ok(PathBuf::from(program_files).join("Adobe/Common/Plug-ins/7.0/MediaCore"))
	}
}

/// Inspect installed links without following them, including dangling links.
pub(super) fn plugin_metadata(path: &Path) -> Result<Option<fs::Metadata>> {
	let extension = path.extension().and_then(|value| value.to_str());
	ensure!(
		matches!(extension, Some("plugin" | "aex")),
		"unsupported plugin path {}",
		path.display()
	);
	let metadata = match fs::symlink_metadata(path) {
		Ok(metadata) => metadata,
		Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
		Err(error) => return Err(error).with_context(|| format!("cannot inspect {}", path.display())),
	};
	ensure!(
		metadata.file_type().is_symlink()
			|| if extension == Some("plugin") {
				metadata.is_dir()
			} else {
				metadata.is_file()
			},
		"unexpected plugin file type at {}",
		path.display()
	);
	Ok(Some(metadata))
}

/// Remove a link itself, or an older installation that contains a real plugin.
pub(super) fn remove_plugin(path: &Path, metadata: &fs::Metadata) -> std::io::Result<()> {
	if metadata.file_type().is_symlink() {
		#[cfg(windows)]
		{
			use std::os::windows::fs::FileTypeExt;
			if metadata.file_type().is_symlink_dir() {
				return fs::remove_dir(path);
			}
		}
		fs::remove_file(path)
	} else if metadata.is_dir() {
		fs::remove_dir_all(path)
	} else {
		fs::remove_file(path)
	}
}
