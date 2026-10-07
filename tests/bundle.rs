//! Bundle integration tests using real libraries and dummy PiPL resources.

use std::{
	fs,
	path::Path,
	process::{Command, Output},
};

use tempfile::{TempDir, tempdir};

fn fixture(name: &str, config: &str) -> TempDir {
	let directory = tempdir().unwrap();
	write_package(directory.path(), name, config);
	directory
}

fn write_package(root: &Path, name: &str, config: &str) {
	fs::create_dir_all(root.join("src")).unwrap();
	fs::write(
		root.join("Cargo.toml"),
		format!(
			r#"
[package]
name = "{name}"
version = "1.2.3"
edition = "2024"
[lib]
name = "custom_binary"
crate-type = ["cdylib", "rlib"]
[features]
required = []
[profile.production]
inherits = "release"
[package.metadata.aex]
{config}
"#
		),
	)
	.unwrap();
	fs::write(
		root.join("src/lib.rs"),
		"#[unsafe(no_mangle)] pub extern \"C\" fn EffectMain() -> i32 { 42 }",
	)
	.unwrap();
	fs::write(root.join("build.rs"), r##"
fn main() {
    let directory = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let directory = directory.ancestors().nth(3).unwrap();
    let name = std::env::var("CARGO_PKG_NAME").unwrap();
    std::fs::write(directory.join(format!("{name}.rsrc")), b"test PiPL resource").unwrap();
    std::fs::write(directory.join(format!("{name}_PkgInfo")), b"AEgxTEST").unwrap();
    std::fs::write(directory.join(format!("{name}_Info.plist")), r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundlePackageType</key><string>AEgx</string><key>LSRequiresCarbon</key><true/></dict></plist>"#).unwrap();
}
"##).unwrap();
}

fn run(root: &Path, arguments: &[&str]) -> Output {
	Command::new(env!("CARGO_BIN_EXE_cargo-aex"))
		.current_dir(root)
		.env_remove("CARGO_BUILD_TARGET")
		.env_remove("CARGO_TARGET_DIR")
		.env_remove("CARGO_BUILD_BUILD_DIR")
		.env("CARGO_NET_OFFLINE", "true")
		.args(["aex", "bundle"])
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

#[test]
fn rejects_non_cdylib_and_unknown_package() {
	let directory = fixture("example", "");
	let manifest = directory.path().join("Cargo.toml");
	let contents = fs::read_to_string(&manifest)
		.unwrap()
		.replace("[\"cdylib\", \"rlib\"]", "[\"rlib\"]");
	fs::write(manifest, contents).unwrap();
	let output = run(directory.path(), &[]);
	assert!(!output.status.success());
	assert!(String::from_utf8_lossy(&output.stderr).contains("crate-type"));
	let output = run(directory.path(), &["-p", "missing"]);
	assert!(!output.status.success());
	assert!(String::from_utf8_lossy(&output.stderr).contains("not a workspace member"));
}

#[test]
fn rejects_unsafe_and_duplicate_bundle_names() {
	let directory = fixture("example", "name = '../escape'");
	let output = run(directory.path(), &[]);
	assert!(!output.status.success());
	assert!(String::from_utf8_lossy(&output.stderr).contains("invalid bundle name"));
	fs::remove_file(directory.path().join("Cargo.toml")).unwrap();
	write_package(&directory.path().join("first"), "first", "name = 'Same'");
	write_package(&directory.path().join("second"), "second", "name = 'same'");
	fs::write(
		directory.path().join("Cargo.toml"),
		"[workspace]\nmembers = ['first', 'second']\nresolver = '3'\n",
	)
	.unwrap();
	let output = run(directory.path(), &["--workspace"]);
	assert!(!output.status.success());
	assert!(String::from_utf8_lossy(&output.stderr).contains("multiple packages"));
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn bundle_path(root: &Path, profile: &str, name: &str) -> std::path::PathBuf {
	root.join("target").join(profile).join("bundle").join(format!(
		"{name}.{}",
		if cfg!(target_os = "macos") { "plugin" } else { "aex" }
	))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn builds_bundles_and_replaces_fresh_artifacts() {
	let directory = fixture(
		"example",
		"name = 'Example & Effect'\nbundle-identifier = 'studio.example.effect'",
	);
	success(run(directory.path(), &["--offline", "--no-sign"]));
	let bundle = bundle_path(directory.path(), "debug", "Example & Effect");
	if cfg!(target_os = "macos") {
		let contents = bundle.join("Contents");
		assert_eq!(
			fs::read(contents.join("MacOS/Example & Effect")).unwrap(),
			fs::read(directory.path().join("target/debug/libcustom_binary.dylib")).unwrap()
		);
		assert_eq!(
			fs::read(contents.join("Resources/Example & Effect.rsrc")).unwrap(),
			b"test PiPL resource"
		);
		assert_eq!(fs::read(contents.join("PkgInfo")).unwrap(), b"AEgxTEST");
		let info = plist::Value::from_file(contents.join("Info.plist")).unwrap();
		let info = info.as_dictionary().unwrap();
		assert_eq!(info["CFBundleExecutable"].as_string(), Some("Example & Effect"));
		assert_eq!(info["CFBundleIdentifier"].as_string(), Some("studio.example.effect"));
		assert_eq!(info["CFBundleVersion"].as_string(), Some("1.2.3"));
		assert_eq!(info["LSRequiresCarbon"].as_boolean(), Some(true));
		fs::write(contents.join("stale"), "stale").unwrap();
	} else {
		assert_eq!(
			fs::read(&bundle).unwrap(),
			fs::read(directory.path().join("target/debug/custom_binary.dll")).unwrap()
		);
		fs::write(&bundle, "stale").unwrap();
	}
	success(run(directory.path(), &["--locked", "--no-sign"]));
	assert!(!bundle.join("Contents/stale").exists());
	fs::write(
		directory.path().join("src/lib.rs"),
		"compile_error!(\"intentional failure\");",
	)
	.unwrap();
	let output = run(directory.path(), &["--no-sign"]);
	assert!(!output.status.success());
	assert!(String::from_utf8_lossy(&output.stderr).contains("intentional failure"));
	assert!(bundle.exists());
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn honors_manifest_profile_features_and_output_directories() {
	let directory = fixture("example", "");
	fs::write(
		directory.path().join("src/lib.rs"),
		"#[cfg(not(feature = \"required\"))] compile_error!(\"missing feature\");",
	)
	.unwrap();
	let caller = tempdir().unwrap();
	let rustc = Command::new("rustc").arg("-vV").output().unwrap();
	let version = String::from_utf8(rustc.stdout).unwrap();
	let target = version.lines().find_map(|line| line.strip_prefix("host: ")).unwrap();
	let output = Command::new(env!("CARGO_BIN_EXE_cargo-aex"))
		.current_dir(caller.path())
		.env("CARGO_BUILD_BUILD_DIR", caller.path().join("intermediate files"))
		.args(["bundle", "--manifest-path"])
		.arg(directory.path().join("Cargo.toml"))
		.args(["--target", target])
		.args([
			"--profile",
			"production",
			"--features",
			"required",
			"--target-dir",
			"build output",
			"--output-dir",
			"bundles",
			"--offline",
			"--no-sign",
		])
		.output()
		.unwrap();
	success(output);
	assert!(
		caller
			.path()
			.join("build output")
			.join(target)
			.join("production")
			.exists()
	);
	assert!(caller.path().join("intermediate files").exists());
	assert!(
		caller
			.path()
			.join(format!(
				"bundles/example.{}",
				if cfg!(target_os = "macos") { "plugin" } else { "aex" }
			))
			.exists()
	);
}

#[cfg(target_os = "macos")]
#[test]
fn signs_bundle_and_preserves_it_when_resources_or_signing_fail() {
	let directory = fixture("example", "");
	success(run(directory.path(), &["--sign", "-"]));
	let bundle = bundle_path(directory.path(), "debug", "example");
	assert!(
		Command::new("codesign")
			.args(["--verify", "--strict"])
			.arg(&bundle)
			.status()
			.unwrap()
			.success()
	);
	let original = fs::read(bundle.join("Contents/MacOS/example")).unwrap();
	fs::remove_file(directory.path().join("target/debug/example.rsrc")).unwrap();
	let output = run(directory.path(), &[]);
	assert!(!output.status.success());
	assert!(String::from_utf8_lossy(&output.stderr).contains("PiPL resource missing"));
	assert_eq!(original, fs::read(bundle.join("Contents/MacOS/example")).unwrap());
	fs::write(directory.path().join("target/debug/example.rsrc"), "resource").unwrap();
	let output = run(directory.path(), &["--sign", "nonexistent-cargo-aex-test-identity"]);
	assert!(!output.status.success());
	assert_eq!(original, fs::read(bundle.join("Contents/MacOS/example")).unwrap());
}

#[cfg(target_os = "macos")]
#[test]
fn generates_fallback_plist() {
	let directory = fixture("example_plugin", "");
	fs::write(
		directory.path().join("build.rs"),
		r#"
fn main() {
    let directory = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let directory = directory.ancestors().nth(3).unwrap();
    std::fs::write(directory.join("example_plugin.rsrc"), b"test PiPL resource").unwrap();
}
"#,
	)
	.unwrap();
	success(run(directory.path(), &["--release", "--no-universal", "--no-sign"]));
	let contents = bundle_path(directory.path(), "release", "example_plugin").join("Contents");
	assert_eq!(
		fs::read(contents.join("Resources/example_plugin.rsrc")).unwrap(),
		b"test PiPL resource"
	);
	assert_eq!(fs::read(contents.join("PkgInfo")).unwrap(), b"eFKTFXTC");
	let info = plist::Value::from_file(contents.join("Info.plist")).unwrap();
	assert_eq!(
		info.as_dictionary().unwrap()["CFBundleIdentifier"].as_string(),
		Some("com.after-effects.example-plugin")
	);
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn selects_workspace_plugins_and_manifest_package() {
	let directory = tempdir().unwrap();
	for name in ["first", "second"] {
		let root = directory.path().join(name);
		write_package(&root, name, "");
		let manifest = root.join("Cargo.toml");
		let contents = fs::read_to_string(&manifest).unwrap().replace("custom_binary", name);
		fs::write(manifest, contents).unwrap();
	}
	fs::create_dir_all(directory.path().join("helper/src")).unwrap();
	fs::write(directory.path().join("helper/src/lib.rs"), "").unwrap();
	fs::write(
		directory.path().join("helper/Cargo.toml"),
		"[package]\nname = 'helper'\nversion = '0.1.0'\n",
	)
	.unwrap();
	fs::write(
		directory.path().join("Cargo.toml"),
		"[workspace]\nmembers = ['first', 'second', 'helper']\ndefault-members = ['second']\nresolver = '3'\n",
	)
	.unwrap();
	success(run(directory.path(), &["--no-sign"]));
	assert!(bundle_path(directory.path(), "debug", "second").exists());
	assert!(!bundle_path(directory.path(), "debug", "first").exists());
	success(run(&directory.path().join("first"), &["--no-sign"]));
	assert!(bundle_path(directory.path(), "debug", "first").exists());
	fs::remove_dir_all(directory.path().join("target/debug/bundle")).unwrap();
	success(run(directory.path(), &["-p", "second", "--no-sign"]));
	assert!(!bundle_path(directory.path(), "debug", "first").exists());
	success(run(directory.path(), &["--workspace", "--no-sign"]));
	assert!(bundle_path(directory.path(), "debug", "first").exists());
	assert!(bundle_path(directory.path(), "debug", "second").exists());
	assert!(!bundle_path(directory.path(), "debug", "helper").exists());
}

#[cfg(target_os = "macos")]
#[test]
fn builds_signed_universal_release_and_preserves_it_on_cross_build_failure() {
	let directory = fixture("example", "name = 'Universal Effect'");
	let script = directory.path().join("build.rs");
	let source = fs::read_to_string(&script).unwrap().replace(
		"b\"test PiPL resource\"",
		"std::env::var(\"CARGO_CFG_TARGET_ARCH\").unwrap().as_bytes()",
	);
	fs::write(script, source).unwrap();
	success(run(directory.path(), &["--release", "--offline", "--sign", "-"]));
	let contents = bundle_path(directory.path(), "release", "Universal Effect").join("Contents");
	let executable = contents.join("MacOS/Universal Effect");
	let output = Command::new("lipo").arg("-archs").arg(&executable).output().unwrap();
	assert!(output.status.success());
	let architectures = String::from_utf8(output.stdout).unwrap();
	assert!(architectures.contains("x86_64"));
	assert!(architectures.contains("arm64"));
	assert_eq!(
		fs::read(contents.join("Resources/Universal Effect.rsrc")).unwrap(),
		b"x86_64"
	);
	assert_eq!(fs::read(contents.join("PkgInfo")).unwrap(), b"AEgxTEST");
	let signature = Command::new("codesign")
		.args(["--display", "--verbose=4"])
		.arg(contents.parent().unwrap())
		.output()
		.unwrap();
	assert!(signature.status.success());
	assert!(String::from_utf8_lossy(&signature.stderr).contains("runtime"));
	let original = fs::read(&executable).unwrap();
	fs::write(
		directory.path().join("src/lib.rs"),
		"#[cfg(target_arch = \"x86_64\")] compile_error!(\"intentional Intel build failure\");",
	)
	.unwrap();
	let output = run(directory.path(), &["--release", "--offline", "--sign", "-"]);
	assert!(!output.status.success());
	assert!(String::from_utf8_lossy(&output.stderr).contains("intentional Intel build failure"));
	assert_eq!(original, fs::read(executable).unwrap());
}

#[cfg(target_os = "macos")]
#[test]
fn supports_explicit_universal_debug_and_binary_named_sidecars() {
	let directory = fixture("example", "");
	let script = directory.path().join("build.rs");
	let source = fs::read_to_string(&script).unwrap().replace(
		"let name = std::env::var(\"CARGO_PKG_NAME\").unwrap();",
		"let name = \"custom_binary\";",
	);
	fs::write(script, source).unwrap();
	success(run(directory.path(), &["--universal", "--no-sign", "--offline"]));
	let contents = bundle_path(directory.path(), "debug", "example").join("Contents");
	assert_eq!(fs::read(contents.join("PkgInfo")).unwrap(), b"AEgxTEST");
	let output = Command::new("lipo")
		.arg("-archs")
		.arg(contents.join("MacOS/example"))
		.output()
		.unwrap();
	assert!(output.status.success());
	assert_eq!(String::from_utf8(output.stdout).unwrap().split_whitespace().count(), 2);
	let info = plist::Value::from_file(contents.join("Info.plist")).unwrap();
	assert_eq!(
		info.as_dictionary().unwrap()["CFBundleSignature"].as_string(),
		Some("TEST")
	);
}

#[cfg(target_os = "macos")]
#[test]
fn rejects_mismatched_pkginfo_without_replacing_bundle() {
	let directory = fixture("example", "");
	success(run(directory.path(), &["--no-sign"]));
	let contents = bundle_path(directory.path(), "debug", "example").join("Contents");
	let original = fs::read(contents.join("MacOS/example")).unwrap();
	fs::write(directory.path().join("target/debug/example_PkgInfo"), "eFKTFXTC").unwrap();
	let output = run(directory.path(), &["--no-sign"]);
	assert!(!output.status.success());
	assert!(String::from_utf8_lossy(&output.stderr).contains("does not match"));
	assert_eq!(fs::read(contents.join("MacOS/example")).unwrap(), original);
}
