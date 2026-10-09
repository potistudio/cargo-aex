//! Builds and bundles After Effects plugins.

use std::{
	collections::{HashMap, HashSet},
	fs,
	io::BufReader,
	path::{Path, PathBuf},
	process::{Command, ExitCode, Stdio},
};

use anyhow::{Context, Result, bail, ensure};
use cargo_metadata::{Artifact, Message, Metadata, MetadataCommand, Package};
use clap::Args;
use plist::{Dictionary, Value};
use serde::Deserialize;

/// Cargo package selection shared by bundling, installation, and removal.
#[derive(Debug, Default, Args)]
pub(super) struct PackageArgs {
	/// Path to the plugin or workspace Cargo.toml.
	#[arg(long)]
	manifest_path: Option<PathBuf>,

	/// Select packages by exact name; may be repeated.
	#[arg(short, long, conflicts_with = "workspace")]
	package: Vec<String>,

	/// Select all cdylib packages in the workspace.
	#[arg(long)]
	workspace: bool,

	/// Require an unchanged Cargo.lock.
	#[arg(long)]
	locked: bool,

	/// Run Cargo offline.
	#[arg(long)]
	offline: bool,
}

/// Options for the bundle command.
#[derive(Debug, Default, Args)]
pub struct BundleArgs {
	#[command(flatten)]
	packages: PackageArgs,

	/// Build with the release profile; macOS defaults to a universal binary.
	#[arg(long, conflicts_with = "profile")]
	release: bool,

	/// Cargo build profile.
	#[arg(long)]
	profile: Option<String>,

	/// Rust target triple; disables automatic universal builds.
	#[arg(long)]
	target: Option<String>,

	/// Build a macOS universal binary.
	#[arg(long, conflicts_with_all = ["target", "no_universal"])]
	universal: bool,

	/// Disable automatic universal builds.
	#[arg(long)]
	no_universal: bool,

	/// Cargo build output directory.
	#[arg(long)]
	target_dir: Option<PathBuf>,

	/// Bundle output directory; defaults to the artifact directory's `bundle/`.
	#[arg(long)]
	output_dir: Option<PathBuf>,

	/// Cargo features to activate.
	#[arg(long, short = 'F')]
	features: Vec<String>,

	/// Activate all Cargo features.
	#[arg(long)]
	all_features: bool,

	/// Disable default Cargo features.
	#[arg(long)]
	no_default_features: bool,

	/// Signing identity; defaults to Apple Development, then ad-hoc. Use `-` for ad-hoc.
	#[arg(long, allow_hyphen_values = true, conflicts_with = "no_sign")]
	sign: Option<String>,

	/// Skip macOS signing.
	#[arg(long)]
	no_sign: bool,
}

/// Bundle settings from `[package.metadata.aex]`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct BundleConfig {
	/// Bundle name; defaults to the package name.
	name: Option<String>,

	/// macOS bundle identifier override.
	bundle_identifier: Option<String>,
}

/// An After Effects plugin to build and bundle.
struct Plugin<'a> {
	package: &'a Package,
	config: BundleConfig,
	name: String,
}

/// Bundle layout selected from the library filename extension.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Platform {
	Windows,
	MacOs,
}

impl Platform {
	/// Determine the plugin platform from a build artifact.
	fn from_artifact(path: &Path) -> Result<Self> {
		match path.extension().and_then(|extension| extension.to_str()) {
			Some("dll") => Ok(Self::Windows),
			Some("dylib") => Ok(Self::MacOs),
			_ => bail!(
				"unsupported plugin artifact {}; expected a .dll or .dylib file",
				path.display()
			),
		}
	}

	/// Return the plugin bundle extension recognized by After Effects (`aex` or `plugin`).
	fn extension(self) -> &'static str {
		match self {
			Self::Windows => "aex",
			Self::MacOs => "plugin",
		}
	}
}

/// Run the command and report failures to the CLI.
pub fn run(args: BundleArgs) -> ExitCode {
	let result = bundle(&args);
	match result {
		Ok(_) => ExitCode::SUCCESS,
		Err(error) => {
			eprintln!("error: {error:#}");
			ExitCode::FAILURE
		}
	}
}

/// Create a command to invoke Cargo.
fn cargo() -> Command {
	Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// Read metadata without building so uninstall works even when the source cannot compile.
fn load_metadata(args: &PackageArgs) -> Result<(Metadata, PathBuf)> {
	let manifest = if let Some(path) = &args.manifest_path {
		fs::canonicalize(path).with_context(|| format!("cannot find manifest {}", path.display()))?
	} else {
		let output = cargo().args(["locate-project", "--message-format", "plain"]).output()?;

		ensure!(
			output.status.success(),
			"cargo locate-project failed: {}",
			String::from_utf8_lossy(&output.stderr)
		);
		PathBuf::from(String::from_utf8(output.stdout)?.trim())
	};

	let mut metadata_command = MetadataCommand::new();
	metadata_command.manifest_path(&manifest).no_deps();

	let mut metadata_options = Vec::new();

	if args.locked {
		metadata_options.push("--locked".into());
	}

	if args.offline {
		metadata_options.push("--offline".into());
	}

	let metadata = metadata_command
		.other_options(metadata_options)
		.exec()
		.context("cannot read Cargo metadata")?;
	Ok((metadata, manifest))
}

/// Resolve the same plugin names used by the bundler, without building.
pub(super) fn plugin_names(args: &PackageArgs) -> Result<Vec<String>> {
	let (metadata, manifest) = load_metadata(args)?;
	Ok(select_plugins(&metadata, &manifest, args)?
		.into_iter()
		.map(|plugin| plugin.name)
		.collect())
}

/// Build and bundle plugins for distribution, returning their published paths.
pub(super) fn bundle(args: &BundleArgs) -> Result<Vec<PathBuf>> {
	let (metadata, manifest) = load_metadata(&args.packages)?;
	let plugins = select_plugins(&metadata, &manifest, &args.packages)?;
	let primary = build_plugins(&manifest, &plugins, args, args.target.as_deref())?;
	let universal = args.universal
		|| (!args.no_universal
			&& (args.release || args.profile.as_deref() == Some("release"))
			&& args.target.is_none()
			&& primary.iter().all(|artifact| artifact.platform == Platform::MacOs));
	let universal_builds = if universal {
		ensure!(cfg!(target_os = "macos"), "universal bundling requires macOS and lipo");
		ensure!(
			primary.iter().all(|artifact| artifact.platform == Platform::MacOs),
			"universal bundling requires macOS plugin artifacts"
		);
		ensure_macos_targets(args.packages.offline)?;
		Some((
			build_plugins(&manifest, &plugins, args, Some("x86_64-apple-darwin"))?,
			build_plugins(&manifest, &plugins, args, Some("aarch64-apple-darwin"))?,
		))
	} else {
		None
	};

	let mut destinations = Vec::new();
	for (index, plugin) in plugins.iter().enumerate() {
		let output = args
			.output_dir
			.clone()
			.unwrap_or_else(|| primary[index].binary.parent().unwrap().join("bundle"));
		let (artifact, arm_binary) = if let Some((intel, arm)) = &universal_builds {
			(&intel[index], Some(arm[index].binary.as_path()))
		} else {
			(&primary[index], None)
		};

		let destination = package_plugin(
			plugin,
			&artifact.binary,
			artifact.platform,
			&artifact.resource_dir,
			&output,
			args,
			arm_binary,
		)?;

		println!("Bundled {}", destination.display());
		destinations.push(destination);
	}

	Ok(destinations)
}

/// The compiled library and the directory containing its generated bundle files.
struct BuildArtifact {
	binary: PathBuf,
	platform: Platform,
	resource_dir: PathBuf,
}

/// Build the selected packages and collect their library and resource paths from Cargo output.
fn build_plugins(
	manifest: &Path,
	plugins: &[Plugin<'_>],
	args: &BundleArgs,
	target: Option<&str>,
) -> Result<Vec<BuildArtifact>> {
	let mut command = cargo();
	command.args(["build", "--lib", "--message-format=json-render-diagnostics"]);
	command.arg("--manifest-path").arg(manifest);

	for plugin in plugins {
		command.arg("--package").arg(plugin.package.name.as_str());
	}

	if args.release {
		command.arg("--release");
	}

	for (flag, value) in [("--profile", args.profile.as_deref()), ("--target", target)] {
		if let Some(value) = value {
			command.arg(flag).arg(value);
		}
	}

	if let Some(path) = &args.target_dir {
		command.arg("--target-dir").arg(path);
	}

	for features in &args.features {
		command.arg("--features").arg(features);
	}

	for (flag, enabled) in [
		("--all-features", args.all_features),
		("--no-default-features", args.no_default_features),
		("--locked", args.packages.locked),
		("--offline", args.packages.offline),
	] {
		if enabled {
			command.arg(flag);
		}
	}

	let mut child = command
		.stdout(Stdio::piped())
		.spawn()
		.context("cannot start cargo build")?;
	let mut artifacts = HashMap::new();
	let mut resource_dirs = HashMap::new();
	let mut read_error = None;

	for message in Message::parse_stream(BufReader::new(child.stdout.take().unwrap())) {
		match message {
			Ok(Message::CompilerArtifact(artifact)) if artifact.target.is_cdylib() && !artifact.profile.test => {
				artifacts.insert(artifact.package_id.clone(), artifact);
			}
			Ok(Message::BuildScriptExecuted(script)) => {
				// pipl writes sidecars three levels above OUT_DIR. This can differ from
				// the artifact directory when Cargo's build directory is customized.
				if let Some(directory) = script.out_dir.as_std_path().ancestors().nth(3) {
					resource_dirs.insert(script.package_id, directory.to_path_buf());
				}
			}
			Ok(Message::CompilerMessage(message)) => {
				if let Some(rendered) = message.message.rendered {
					eprint!("{rendered}");
				}
			}
			Ok(Message::TextLine(line)) => eprintln!("{line}"),
			Err(error) => read_error = Some(error),
			_ => {}
		}
	}

	let status = child.wait().context("cannot wait for cargo build")?;
	ensure!(status.success(), "cargo build failed ({status})");

	if let Some(error) = read_error {
		return Err(error).context("cannot read Cargo build output");
	}

	// Validate every selected artifact before publishing any bundles.
	plugins
		.iter()
		.map(|plugin| {
			let artifact = artifacts
				.get(&plugin.package.id)
				.with_context(|| format!("no cdylib artifact was produced for {}", plugin.package.name))?;
			let binary = dynamic_library(artifact)?;
			let platform = Platform::from_artifact(&binary)?;
			let resource_dir = resource_dirs
				.remove(&plugin.package.id)
				.unwrap_or_else(|| binary.parent().unwrap().to_path_buf());

			Ok(BuildArtifact {
				binary,
				platform,
				resource_dir,
			})
		})
		.collect()
}

/// Install missing Rust targets for universal builds, unless offline mode is enabled.
fn ensure_macos_targets(offline: bool) -> Result<()> {
	for target in ["x86_64-apple-darwin", "aarch64-apple-darwin"] {
		let output = Command::new("rustc")
			.args(["--print", "target-libdir", "--target", target])
			.output()?;
		let directory = PathBuf::from(String::from_utf8(output.stdout)?.trim());
		let installed = output.status.success()
			&& fs::read_dir(directory).is_ok_and(|entries| {
				entries
					.filter_map(Result::ok)
					.any(|entry| entry.file_name().to_string_lossy().starts_with("libstd-"))
			});
		if !installed {
			ensure!(
				!offline,
				"Rust target {target} is not installed; run `rustup target add {target}` before using --offline"
			);
			let status = Command::new("rustup")
				.args(["target", "add", target])
				.status()
				.with_context(|| format!("cannot install Rust target {target}; run `rustup target add {target}`"))?;
			ensure!(status.success(), "could not install Rust target {target} ({status})");
		}
	}
	Ok(())
}

/// Resolve the package selection and validate bundle settings and output names.
fn select_plugins<'a>(metadata: &'a Metadata, manifest: &Path, args: &PackageArgs) -> Result<Vec<Plugin<'a>>> {
	let members: Vec<_> = metadata
		.packages
		.iter()
		.filter(|package| metadata.workspace_members.contains(&package.id))
		.collect();
	for name in &args.package {
		ensure!(
			members.iter().any(|package| package.name.as_str() == name),
			"package '{name}' is not a workspace member"
		);
	}
	let manifest_package = members
		.iter()
		.find(|package| package.manifest_path.as_std_path() == manifest)
		.copied();
	let selected: Vec<_> = members
		.into_iter()
		.filter(|package| {
			if args.workspace {
				package.targets.iter().any(|target| target.is_cdylib())
			} else if !args.package.is_empty() {
				args.package.iter().any(|name| package.name.as_str() == name)
			} else if let Some(root) = manifest_package {
				package.id == root.id
			} else {
				metadata.workspace_default_members.contains(&package.id)
			}
		})
		.collect();
	let mut names = HashSet::new();
	let mut plugins = Vec::new();
	for package in selected {
		ensure!(
			package.targets.iter().any(|target| target.is_cdylib()),
			"package '{}' must declare [lib] crate-type = [\"cdylib\"]",
			package.name
		);
		let config: BundleConfig = package
			.metadata
			.get("aex")
			.map(|value| serde_json::from_value(value.clone()))
			.transpose()
			.with_context(|| format!("invalid package.metadata.aex for {}", package.name))?
			.unwrap_or_default();
		let name = config.name.clone().unwrap_or_else(|| package.name.to_string());
		validate_name(&name)?;
		ensure!(
			names.insert(name.to_lowercase()),
			"multiple packages use bundle name '{name}'"
		);
		plugins.push(Plugin { package, config, name });
	}
	ensure!(!plugins.is_empty(), "no cdylib plugin packages selected");
	Ok(plugins)
}

fn validate_name(name: &str) -> Result<()> {
	ensure!(
		!name.is_empty()
			&& name != "."
			&& name != ".."
			&& !name.ends_with(['.', ' '])
			&& !name.chars().any(|c| c.is_control() || "/\\:*?\"<>|".contains(c)),
		"invalid bundle name '{name}'"
	);
	Ok(())
}

fn dynamic_library(artifact: &Artifact) -> Result<PathBuf> {
	artifact
		.filenames
		.iter()
		.find(|path| matches!(path.extension(), Some("dll" | "dylib" | "so")))
		.map(|path| path.as_std_path().to_path_buf())
		.with_context(|| format!("no dynamic library in Cargo artifacts for {}", artifact.target.name))
}

/// Assemble and sign a bundle in staging before replacing its output.
/// When `arm_binary` is supplied, combine it with the Intel `binary` using `lipo`.
fn package_plugin(
	plugin: &Plugin<'_>,
	binary: &Path,
	platform: Platform,
	resource_dir: &Path,
	output: &Path,
	args: &BundleArgs,
	arm_binary: Option<&Path>,
) -> Result<PathBuf> {
	fs::create_dir_all(output).with_context(|| format!("cannot create {}", output.display()))?;
	let staging = tempfile::tempdir_in(output)?;
	let filename = format!("{}.{}", plugin.name, platform.extension());
	let staged = staging.path().join(&filename);
	match platform {
		Platform::Windows => {
			fs::copy(binary, &staged)?;
		}
		Platform::MacOs => {
			let contents = staged.join("Contents");
			fs::create_dir_all(contents.join("MacOS"))?;
			fs::create_dir_all(contents.join("Resources"))?;
			let rsrc = sidecar_path(plugin, resource_dir, ".rsrc")
				.filter(|path| path.is_file())
				.with_context(|| {
					format!(
						"PiPL resource missing in {}; generate it in build.rs using pipl",
						resource_dir.display()
					)
				})?;
			fs::copy(rsrc, contents.join("Resources").join(format!("{}.rsrc", plugin.name)))?;
			let executable = contents.join("MacOS").join(&plugin.name);
			if let Some(arm_binary) = arm_binary {
				ensure!(
					Platform::from_artifact(arm_binary)? == Platform::MacOs,
					"universal binary requires two macOS libraries"
				);
				let status = Command::new("lipo")
					.arg(binary)
					.arg(arm_binary)
					.args(["-create", "-output"])
					.arg(&executable)
					.status()
					.context("cannot run lipo; install Xcode Command Line Tools")?;
				ensure!(status.success(), "lipo failed ({status})");
			} else {
				fs::copy(binary, &executable)?;
			}
			let mut info = if let Some(plist_path) = sidecar_path(plugin, resource_dir, "_Info.plist") {
				Value::from_file(&plist_path).with_context(|| format!("cannot read {}", plist_path.display()))?
			} else {
				Value::Dictionary(Dictionary::new())
			};
			let pkginfo = if let Some(pkginfo_path) = sidecar_path(plugin, resource_dir, "_PkgInfo") {
				let bytes = fs::read(&pkginfo_path)?;
				ensure!(
					bytes.len() == 8 && bytes.is_ascii(),
					"{} must contain eight ASCII bytes",
					pkginfo_path.display()
				);
				let dict = info
					.as_dictionary_mut()
					.context("Info.plist must contain a dictionary")?;
				for (key, bytes) in [("CFBundlePackageType", &bytes[..4]), ("CFBundleSignature", &bytes[4..])] {
					let value = std::str::from_utf8(bytes)?;
					if let Some(existing) = dict.get(key) {
						ensure!(
							existing.as_string() == Some(value),
							"{key} in Info.plist does not match {}",
							pkginfo_path.display()
						);
					} else {
						dict.insert(key.into(), Value::String(value.into()));
					}
				}
				Some(bytes)
			} else {
				None
			};
			configure_plist(&mut info, plugin)?;
			let dict = info.as_dictionary().unwrap();
			let kind = dict["CFBundlePackageType"]
				.as_string()
				.context("CFBundlePackageType must be a string")?;
			let signature = dict["CFBundleSignature"]
				.as_string()
				.context("CFBundleSignature must be a string")?;
			ensure!(
				kind.len() == 4 && kind.is_ascii() && signature.len() == 4 && signature.is_ascii(),
				"bundle package type and signature must be four ASCII characters"
			);
			fs::write(
				contents.join("PkgInfo"),
				pkginfo.unwrap_or_else(|| format!("{kind}{signature}").into_bytes()),
			)?;
			info.to_file_xml(contents.join("Info.plist"))?;
			if !args.no_sign {
				sign_bundle(&staged, args.sign.as_deref())?;
			}
		}
	}
	let destination = output.join(filename);
	// Build and sign in staging so a failed build/resource copy/sign leaves the
	// previous bundle intact. Refuse symlinks at the publication destination.
	if let Ok(existing) = fs::symlink_metadata(&destination) {
		ensure!(
			!existing.file_type().is_symlink(),
			"refusing to replace symlink {}",
			destination.display()
		);
		if existing.is_dir() {
			fs::remove_dir_all(&destination)?;
		} else {
			fs::remove_file(&destination)?;
		}
	}
	fs::rename(staged, &destination).with_context(|| format!("cannot publish {}", destination.display()))?;
	Ok(destination)
}

/// Find generated bundle files by package name.
fn sidecar_path(plugin: &Plugin<'_>, directory: &Path, suffix: &str) -> Option<PathBuf> {
	let package_path = directory.join(format!("{}{suffix}", plugin.package.name));
	if package_path.exists() {
		return Some(package_path);
	}

	if let Some(target) = plugin.package.targets.iter().find(|target| target.is_cdylib()) {
		let binary_path = directory.join(format!("{}{suffix}", target.name));
		if binary_path.exists() {
			return Some(binary_path);
		}
	}

	None
}

fn development_identity(output: &str) -> Option<String> {
	output
		.lines()
		.filter(|line| line.contains("\"Apple Development:"))
		.find_map(|line| {
			let hash = line.split_whitespace().nth(1)?;
			(hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| hash.to_string())
		})
}

/// Sign with the requested or available development identity, then verify the bundle.
fn sign_bundle(bundle: &Path, requested_identity: Option<&str>) -> Result<()> {
	let identity = if let Some(identity) = requested_identity {
		identity.to_string()
	} else {
		Command::new("security")
			.args(["find-identity", "-v", "-p", "codesigning"])
			.output()
			.ok()
			.filter(|output| output.status.success())
			.and_then(|output| development_identity(&String::from_utf8_lossy(&output.stdout)))
			.unwrap_or_else(|| "-".into())
	};
	let mut command = Command::new("codesign");
	command.args(["--force", "--options", "runtime", "--strict", "--sign", &identity]);
	if identity != "-" {
		command.arg("--timestamp");
	} else {
		eprintln!("Using an ad-hoc signature; select a distribution certificate with --sign for distribution.");
	}
	let status = command
		.arg(bundle)
		.status()
		.context("cannot run codesign; bundle on macOS or use --no-sign")?;
	ensure!(status.success(), "codesign failed ({status})");
	let status = Command::new("codesign")
		.args(["--verify", "--strict", "--all-architectures"])
		.arg(bundle)
		.status()
		.context("cannot verify code signature")?;
	ensure!(status.success(), "code signature verification failed ({status})");
	Ok(())
}

/// Apply the plugin name, identifier, and version while preserving custom plist keys.
fn configure_plist(info: &mut Value, plugin: &Plugin<'_>) -> Result<()> {
	let dict = info
		.as_dictionary_mut()
		.context("Info.plist must contain a dictionary")?;
	let identifier = plugin
		.config
		.bundle_identifier
		.clone()
		.unwrap_or_else(|| format!("com.after-effects.{}", plugin.package.name.replace('_', "-")));
	ensure!(
		!identifier.is_empty()
			&& identifier
				.chars()
				.all(|c| c.is_ascii_alphanumeric() || ".-".contains(c)),
		"invalid bundle identifier '{identifier}'"
	);
	for (key, value) in [
		("CFBundleExecutable", plugin.name.clone()),
		("CFBundleName", plugin.name.clone()),
		("CFBundleIdentifier", identifier),
		("CFBundleInfoDictionaryVersion", "6.0".into()),
		(
			"CFBundleVersion",
			format!(
				"{}.{}.{}",
				plugin.package.version.major, plugin.package.version.minor, plugin.package.version.patch
			),
		),
		(
			"CFBundleShortVersionString",
			format!(
				"{}.{}.{}",
				plugin.package.version.major, plugin.package.version.minor, plugin.package.version.patch
			),
		),
	] {
		dict.insert(key.into(), Value::String(value));
	}
	for (key, value) in [("CFBundlePackageType", "eFKT"), ("CFBundleSignature", "FXTC")] {
		if !dict.contains_key(key) {
			dict.insert(key.into(), Value::String(value.into()));
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn selects_only_valid_apple_development_identities() {
		let hash = "0123456789ABCDEF0123456789ABCDEF01234567";
		let output = format!(
			"  1) bad-hash \"Apple Development: Invalid\"\n  2) {hash} \"Developer ID Application: Example\"\n  3) {hash} \"Apple Development: Example (TEAM)\"\n  2 valid identities found"
		);

		assert_eq!(development_identity(&output).as_deref(), Some(hash));
		assert_eq!(development_identity("  0 valid identities found"), None);
		assert_eq!(
			development_identity(&format!("1) {hash} \"Developer ID Application: Example\"")),
			None
		);
	}

	#[test]
	fn packages_windows_artifact_without_changing_bytes() {
		let directory = tempfile::tempdir().unwrap();
		fs::create_dir(directory.path().join("src")).unwrap();
		fs::write(directory.path().join("src/lib.rs"), "").unwrap();
		fs::write(
			directory.path().join("Cargo.toml"),
			"[package]\nname = 'test-plugin'\nversion = '0.1.0'\nedition = '2024'\n[lib]\ncrate-type = ['cdylib']\n",
		)
		.unwrap();

		let metadata = MetadataCommand::new()
			.manifest_path(directory.path().join("Cargo.toml"))
			.no_deps()
			.exec()
			.unwrap();
		let plugin = Plugin {
			package: &metadata.packages[0],
			config: BundleConfig::default(),
			name: "Test Plugin".into(),
		};
		let binary = directory.path().join("test_plugin.dll");
		let contents = b"MZ\0\0binary and embedded PiPL";
		fs::write(&binary, contents).unwrap();

		let output = directory.path().join("bundles");
		let destination = package_plugin(
			&plugin,
			&binary,
			Platform::Windows,
			directory.path(),
			&output,
			&BundleArgs::default(),
			None,
		)
		.unwrap();

		assert_eq!(destination.file_name().unwrap(), "Test Plugin.aex");
		assert_eq!(fs::read(&destination).unwrap(), contents);
		assert_eq!(fs::read(binary).unwrap(), contents);
		fs::write(&destination, "stale").unwrap();

		package_plugin(
			&plugin,
			&directory.path().join("test_plugin.dll"),
			Platform::Windows,
			directory.path(),
			&output,
			&BundleArgs::default(),
			None,
		)
		.unwrap();

		assert_eq!(fs::read(destination).unwrap(), contents);
	}

	#[test]
	fn rejects_unsupported_artifacts_and_unsafe_names() {
		assert_eq!(
			Platform::from_artifact(Path::new("effect.dll")).unwrap(),
			Platform::Windows
		);
		assert_eq!(
			Platform::from_artifact(Path::new("libeffect.dylib")).unwrap(),
			Platform::MacOs
		);
		assert!(Platform::from_artifact(Path::new("libeffect.so")).is_err());

		for name in [
			"",
			".",
			"..",
			"../effect",
			"a\\b",
			"effect:",
			"effect.",
			"effect ",
			"effect\n",
		] {
			assert!(validate_name(name).is_err(), "{name:?}");
		}

		assert!(validate_name("日本語 Effect & Name").is_ok());
	}
}
