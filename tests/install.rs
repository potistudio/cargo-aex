//! Exercise installation and removal using isolated plugin directories.

#![cfg(any(target_os = "macos", target_os = "windows"))]

use std::{
	fs,
	path::{Path, PathBuf},
	process::{Command, Output},
};

use tempfile::{TempDir, tempdir};

fn write_package(root: &Path, package: &str, name: &str) {
	fs::create_dir_all(root.join("src")).unwrap();
	fs::write(
		root.join("Cargo.toml"),
		format!(
			r#"
[package]
name = "{package}"
version = "1.2.3"
edition = "2024"
[lib]
crate-type = ["cdylib"]
[features]
required = []
[profile.production]
inherits = "release"
[package.metadata.aex]
name = "{name}"
"#
		),
	)
	.unwrap();
	fs::write(root.join("src/lib.rs"), "pub fn effect() -> i32 { 42 }").unwrap();
	fs::write(
		root.join("build.rs"),
		r#"
fn main() {
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let name = std::env::var("CARGO_PKG_NAME").unwrap();
    std::fs::write(out.ancestors().nth(3).unwrap().join(format!("{name}.rsrc")), b"PiPL").unwrap();
}
"#,
	)
	.unwrap();
}

fn fixture() -> TempDir {
	let directory = tempdir().unwrap();
	write_package(directory.path(), "example", "日本語 Effect");
	directory
}

fn run(root: &Path, command: &str, directory: &Path, arguments: &[&str]) -> Output {
	Command::new(env!("CARGO_BIN_EXE_cargo-aex"))
		.current_dir(root)
		.env_remove("CARGO_BUILD_TARGET")
		.env_remove("CARGO_TARGET_DIR")
		.env_remove("CARGO_BUILD_BUILD_DIR")
		.env("CARGO_NET_OFFLINE", "true")
		.args(["aex", command, "--offline", "--install-dir"])
		.arg(directory)
		.args(arguments)
		.output()
		.unwrap()
}

fn success(output: Output) {
	assert!(
		output.status.success(),
		"stdout: {}\nstderr: {}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
}

fn plugin_path(directory: &Path, name: &str) -> PathBuf {
	directory.join(format!(
		"{name}.{}",
		if cfg!(target_os = "macos") { "plugin" } else { "aex" }
	))
}

fn binary_path(plugin: &Path, name: &str) -> PathBuf {
	if cfg!(target_os = "macos") {
		plugin.join("Contents/MacOS").join(name)
	} else {
		plugin.to_path_buf()
	}
}

#[test]
fn installs_updates_and_uninstalls_without_building() {
	let project = fixture();
	let directory = project.path().join("installed plugins");
	success(run(project.path(), "install", &directory, &["--no-sign"]));
	let installed = plugin_path(&directory, "日本語 Effect");
	let bundled = plugin_path(&project.path().join("target/debug/bundle"), "日本語 Effect");
	assert_eq!(fs::read_link(&installed).unwrap(), fs::canonicalize(&bundled).unwrap());
	let binary = binary_path(&installed, "日本語 Effect");
	assert_eq!(
		fs::read(&binary).unwrap(),
		fs::read(binary_path(&bundled, "日本語 Effect")).unwrap()
	);
	fs::write(&binary, "old plugin").unwrap();
	if cfg!(target_os = "macos") {
		fs::write(installed.join("Contents/stale"), "stale").unwrap();
	}
	success(run(project.path(), "install", &directory, &["--no-sign", "--locked"]));
	assert!(!installed.join("Contents/stale").exists());
	let original = fs::read(&binary).unwrap();
	fs::write(
		project.path().join("src/lib.rs"),
		"compile_error!(\"intentional failure\");",
	)
	.unwrap();
	let output = run(project.path(), "install", &directory, &["--no-sign"]);
	assert!(!output.status.success());
	assert_eq!(fs::read(&binary).unwrap(), original);
	fs::write(directory.join("unrelated.txt"), "keep").unwrap();
	fs::remove_dir_all(project.path().join("target")).unwrap();
	assert!(fs::symlink_metadata(&installed).unwrap().file_type().is_symlink());
	success(run(project.path(), "uninstall", &directory, &["--locked"]));
	assert!(!installed.exists());
	assert!(fs::symlink_metadata(&installed).is_err());
	assert!(!project.path().join("target").exists());
	assert_eq!(fs::read(directory.join("unrelated.txt")).unwrap(), b"keep");
	success(run(project.path(), "uninstall", &directory, &[]));
	let missing = project.path().join("missing");
	success(run(project.path(), "uninstall", &missing, &[]));
	assert!(!missing.exists());
}

#[test]
fn switches_between_links_and_copies_and_uninstalls_after_cleaning() {
	let project = fixture();
	let directory = project.path().join("installed");
	let installed = plugin_path(&directory, "日本語 Effect");
	let bundled = plugin_path(&project.path().join("target/debug/bundle"), "日本語 Effect");
	success(run(project.path(), "install", &directory, &["--no-sign"]));
	assert!(fs::symlink_metadata(&installed).unwrap().file_type().is_symlink());
	success(run(project.path(), "install", &directory, &["--by-copy", "--no-sign"]));
	assert!(!fs::symlink_metadata(&installed).unwrap().file_type().is_symlink());
	let binary = binary_path(&installed, "日本語 Effect");
	let original = fs::read(&binary).unwrap();
	fs::write(binary_path(&bundled, "日本語 Effect"), "changed bundle").unwrap();
	assert_eq!(fs::read(&binary).unwrap(), original);
	success(run(project.path(), "install", &directory, &["--no-sign"]));
	assert!(fs::symlink_metadata(&installed).unwrap().file_type().is_symlink());
	success(run(project.path(), "install", &directory, &["--by-copy", "--no-sign"]));
	if cfg!(target_os = "macos") {
		fs::write(installed.join("Contents/stale"), "old file").unwrap();
	}
	success(run(project.path(), "install", &directory, &["--by-copy", "--no-sign"]));
	assert!(!installed.join("Contents/stale").exists());
	fs::remove_dir_all(project.path().join("target")).unwrap();
	assert_eq!(fs::read(&binary).unwrap(), original);
	success(run(project.path(), "uninstall", &directory, &[]));
	assert!(fs::symlink_metadata(&installed).is_err());
	assert!(!project.path().join("target").exists());
}

#[test]
fn honors_manifest_profile_features_and_relative_directories() {
	let project = fixture();
	let caller = tempdir().unwrap();
	fs::write(
		project.path().join("src/lib.rs"),
		"#[cfg(not(feature = \"required\"))] compile_error!(\"missing feature\");",
	)
	.unwrap();
	let manifest = project.path().join("Cargo.toml");
	success(run(
		caller.path(),
		"install",
		Path::new("installed"),
		&[
			"--manifest-path",
			manifest.to_str().unwrap(),
			"--profile",
			"production",
			"--features",
			"required",
			"--target-dir",
			"build output",
			"--output-dir",
			"bundles",
			"--no-sign",
		],
	));
	assert!(caller.path().join("build output/production").exists());
	let bundled = plugin_path(&caller.path().join("bundles"), "日本語 Effect");
	assert!(bundled.exists());
	let installed = plugin_path(&caller.path().join("installed"), "日本語 Effect");
	assert_eq!(fs::read_link(&installed).unwrap(), fs::canonicalize(&bundled).unwrap());
	success(run(
		caller.path(),
		"uninstall",
		Path::new("installed"),
		&["--manifest-path", manifest.to_str().unwrap()],
	));
	assert!(bundled.exists());
	assert!(fs::symlink_metadata(installed).is_err());
}

#[test]
fn selects_workspace_packages_for_installation_and_removal() {
	let project = tempdir().unwrap();
	for name in ["first", "second"] {
		write_package(&project.path().join(name), name, name);
	}
	fs::create_dir_all(project.path().join("helper/src")).unwrap();
	fs::write(project.path().join("helper/src/lib.rs"), "").unwrap();
	fs::write(
		project.path().join("helper/Cargo.toml"),
		"[package]\nname = 'helper'\nversion = '0.1.0'\n",
	)
	.unwrap();
	fs::write(
		project.path().join("Cargo.toml"),
		"[workspace]\nmembers = ['first', 'second', 'helper']\ndefault-members = ['second']\nresolver = '3'\n",
	)
	.unwrap();
	let directory = project.path().join("installed");
	let first = plugin_path(&directory, "first");
	let second = plugin_path(&directory, "second");
	success(run(project.path(), "install", &directory, &["--no-sign"]));
	assert!(second.exists());
	assert!(!first.exists());
	success(run(
		project.path(),
		"install",
		&directory,
		&["--workspace", "--no-sign"],
	));
	assert!(first.exists());
	assert!(second.exists());
	assert!(!plugin_path(&directory, "helper").exists());
	success(run(project.path(), "uninstall", &directory, &["-p", "first"]));
	assert!(!first.exists());
	assert!(second.exists());
	success(run(project.path(), "uninstall", &directory, &["--workspace"]));
	assert!(!second.exists());
}

#[cfg(target_os = "macos")]
#[test]
fn installed_signatures_verify_for_links_and_copies() {
	let project = fixture();
	let directory = project.path().join("installed");
	let installed = plugin_path(&directory, "日本語 Effect");
	let bundled = plugin_path(&project.path().join("target/debug/bundle"), "日本語 Effect");
	for arguments in [&["--sign", "-"][..], &["--sign", "-", "--by-copy"][..]] {
		success(run(project.path(), "install", &directory, arguments));
		success(
			Command::new("codesign")
				.args(["--verify", "--strict", "--all-architectures"])
				.arg(&installed)
				.output()
				.unwrap(),
		);
		success(run(project.path(), "uninstall", &directory, &[]));
		assert!(fs::symlink_metadata(&installed).is_err());
		assert!(bundled.join("Contents/MacOS/日本語 Effect").is_file());
		success(
			Command::new("codesign")
				.args(["--verify", "--strict", "--all-architectures"])
				.arg(&bundled)
				.output()
				.unwrap(),
		);
	}
}
