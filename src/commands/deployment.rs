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

/// Use the latest installed After Effects application's Plug-ins directory by default.
pub(super) fn install_directory(requested: Option<&Path>) -> Result<PathBuf> {
	plugin_extension()?;
	if let Some(path) = requested {
		return Ok(path.to_path_buf());
	}
	let (root, plugin_path) = if cfg!(target_os = "macos") {
		(PathBuf::from("/Applications"), Path::new("Plug-ins"))
	} else {
		let program_files = std::env::var_os("ProgramW6432")
			.or_else(|| std::env::var_os("ProgramFiles"))
			.context("cannot locate Program Files; specify --install-dir")?;
		(
			PathBuf::from(program_files).join("Adobe"),
			Path::new("Support Files/Plug-ins"),
		)
	};
	application_plugin_directory(&root, plugin_path)
}

/// Compare numeric version components rather than lexically sorting version numbers.
fn application_plugin_directory(root: &Path, plugin_path: &Path) -> Result<PathBuf> {
	let entries =
		fs::read_dir(root).with_context(|| format!("cannot inspect {}; specify --install-dir", root.display()))?;
	let mut candidates = Vec::new();
	for entry in entries {
		let entry = entry?;
		let name = entry.file_name().to_string_lossy().into_owned();
		let Some(version) = name.strip_prefix("Adobe After Effects ") else {
			continue;
		};
		// Do not prefer a beta application over a regular installation.
		if version.to_ascii_lowercase().contains("beta") {
			continue;
		}
		let directory = entry.path().join(plugin_path);
		if directory.is_dir() {
			let version = version
				.split(|character: char| !character.is_ascii_digit())
				.filter_map(|component| component.parse::<u32>().ok())
				.collect::<Vec<_>>();
			candidates.push((version, name, directory));
		}
	}
	candidates.sort();
	candidates
		.pop()
		.map(|(_, _, directory)| directory)
		.context("cannot locate After Effects' Plug-ins directory; specify --install-dir")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn selects_latest_application_for_each_platform() {
		for plugin_path in ["Plug-ins", "Support Files/Plug-ins"] {
			let root = tempfile::tempdir().unwrap();
			for name in [
				"Adobe After Effects 2024",
				"Adobe After Effects 2025.9",
				"Adobe After Effects 2025.10",
				"Adobe After Effects 2026 (Beta)",
				"Adobe Premiere Pro 2026",
			] {
				fs::create_dir_all(root.path().join(name).join(plugin_path)).unwrap();
			}
			// An incomplete installation must not be selected.
			fs::create_dir(root.path().join("Adobe After Effects 2027")).unwrap();
			assert_eq!(
				application_plugin_directory(root.path(), Path::new(plugin_path)).unwrap(),
				root.path().join("Adobe After Effects 2025.10").join(plugin_path)
			);
		}
	}

	#[test]
	fn missing_application_requests_explicit_directory() {
		let root = tempfile::tempdir().unwrap();
		let error = application_plugin_directory(root.path(), Path::new("Plug-ins")).unwrap_err();
		assert!(error.to_string().contains("--install-dir"));
	}

	#[cfg(any(target_os = "macos", target_os = "windows"))]
	#[test]
	fn explicit_directory_bypasses_application_discovery() {
		let directory = Path::new("custom/Plug-ins");
		assert_eq!(install_directory(Some(directory)).unwrap(), directory);
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
