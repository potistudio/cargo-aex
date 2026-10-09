# cargo-aex

A CLI tool to support After Effects plugin development.

```sh
cargo install --path .
cargo aex --help
```

## Bundle an After Effects plugin

Run from a Rust plugin project with a `cdylib` library:

```toml
[lib]
crate-type = ["cdylib"]
```

```sh
cargo aex bundle
cargo aex bundle --release
cargo aex bundle --release --no-universal
cargo aex bundle --release --target x86_64-pc-windows-msvc
cargo aex bundle --release --target aarch64-apple-darwin
```

The command runs `cargo build --lib` and packages the resulting dynamic library.
Windows produces `<name>.aex`; macOS produces:

```text
<name>.plugin/
└── Contents/
    ├── Info.plist
    ├── PkgInfo
    ├── MacOS/<name>
    └── Resources/<name>.rsrc
```

Bundles are written to the build artifact directory's `bundle/` subdirectory,
for example `target/release/bundle/` or
`target/aarch64-apple-darwin/release/bundle/`. Use `--output-dir dist` to choose
another directory. `--target-dir` controls Cargo's build directory independently.
Relative CLI paths are resolved from the current directory.

The default name is the Cargo package name. Configure each plugin in its
`Cargo.toml`:

```toml
[package.metadata.aex]
name = "My Effect"
bundle-identifier = "studio.example.my-effect"
```

On macOS, the plugin's `build.rs` must generate a compiled PiPL resource. The
sidecars produced by [`pipl::plugin_build`](https://docs.rs/pipl/latest/pipl/fn.plugin_build.html)
are discovered using Cargo's reported build-script output directory:
`<package-name>.rsrc`, `<package-name>_Info.plist`, and `<package-name>_PkgInfo`.
Sidecars named after the library target (`BinaryName` in `AdobePlugin.just`) are
also supported. Missing PiPL
resources fail the bundle operation. The bundler preserves custom plist keys
and plugin type, sets the executable, name, identifier, and package version,
and copies generated `PkgInfo` without changing its bytes. A generated `PkgInfo`
must match the plist's package type and signature. If `PkgInfo` is missing, it
is created from those plist keys. Without an input plist or `PkgInfo`, the
effect plugin type `eFKT` and signature `FXTC` are used.
The default identifier is `com.after-effects.<package-name>`, with underscores
in the package name replaced by hyphens.

Following the after-effects crate's bundle script, macOS `--release` builds
produce a universal binary with Intel (`x86_64`) and Apple Silicon (`arm64`)
slices. This also applies to `--profile release`. The command builds both Rust
targets, uses the Intel build's sidecars, and combines the libraries with `lipo`.
Missing Rust targets are installed with `rustup target add`; `--offline` instead
requires them to be installed already. Xcode Command Line Tools are required.
Use `--no-universal` or specify `--target` to build a single architecture, or
use `--universal` to request both architectures in a debug or custom profile.

macOS signing automatically selects an available Apple Development certificate,
falling back to an ad-hoc signature when none is found. Use
`--sign "Developer ID Application: Your Name (TEAMID)"` for a specific identity,
`--sign -` to force ad-hoc signing, or `--no-sign` to skip signing. Signing uses
hardened runtime and strict validation; certificate signatures also include
a secure timestamp. The resulting signature is verified for every architecture
before publication. Signing requires macOS and `codesign`; notarization is not
performed.

Windows plugins must already contain their PiPL resources and exported entry
points, normally provided by the plugin's build script and AE bindings. Bundling
copies the built DLL without modifying its contents. Cross compilation requires
the target's Rust standard library, linker, SDK, and other build dependencies.
The build inherits `AESDK_ROOT` and `PRSDK_ROOT` from the environment, so SDK
paths can be supplied in the same way as with `AdobePlugin.just`. Bundling does
not install plugins into After Effects or invoke `sudo`.

For workspaces:

```sh
cargo aex bundle -p my-plugin --release
cargo aex bundle --workspace --release
cargo aex bundle --manifest-path plugins/my-plugin/Cargo.toml
```

`-p` accepts exact package names and can be repeated. `--workspace` selects all
workspace packages with `cdylib` targets, skipping ordinary libraries and CLI
packages. Without a selection, the current manifest's package is used; a virtual
workspace uses its default members. Each selected package must be a `cdylib`,
and plugin names must be unique even when compared without case sensitivity.

Other Cargo build options include `--profile`, `--features` / `-F`,
`--all-features`, `--no-default-features`, `--locked`, and `--offline`.
The bundler uses Cargo's reported artifact filenames, including cached builds,
and forwards compiler diagnostics. It stages each bundle before replacing its
previous output, so build, resource, and signing failures preserve that plugin's
existing bundle. A workspace run publishes plugins individually.

## Install and uninstall plugins

Run these commands from a Rust plugin project:

```sh
cargo aex install
cargo aex install --release
cargo aex install --by-copy
cargo aex uninstall
```

`install` builds and bundles the selected plugins, then creates symbolic links
in Adobe's shared MediaCore directory:

- macOS: `/Library/Application Support/Adobe/Common/Plug-ins/7.0/MediaCore/`
- Windows: `<Program Files>\Adobe\Common\Plug-ins\7.0\MediaCore\`

By default, installation creates a link to each generated `.plugin` directory
or `.aex` file. Specify `--by-copy` to install independent copies of
those bundles in the installation directory:

```sh
cargo aex install --by-copy --release
cargo aex install --by-copy --install-dir "/path/to/Plug-ins"
```

Copy mode preserves executable permissions and macOS signatures. Installed
copies remain available after moving or deleting the local bundle directory,
including with `cargo clean`. Run `install --by-copy` again to update
them.

Installation and removal require macOS or Windows. Installation accepts the
same build, output, universal binary, and signing options as `bundle`. Only
plugins for the host platform can be installed. Use `--install-dir` on both
commands to select another plugin directory, such as a specific After Effects
installation's `Plug-ins` folder. Relative paths are resolved from the current
directory:

```sh
cargo aex install --install-dir "/path/to/Plug-ins"
cargo aex uninstall --install-dir "/path/to/Plug-ins"
cargo aex install -p my-plugin --release
cargo aex uninstall -p my-plugin
cargo aex install --workspace
cargo aex uninstall --workspace
```

Both commands support `--manifest-path`, repeated `-p` / `--package`,
`--workspace`, `--locked`, and `--offline`. Package selection and plugin names
follow the same rules as `bundle`, including `[package.metadata.aex].name`.
`--output-dir` selects the local bundle output, while `--install-dir` selects
the installed plugin's location. Links use absolute paths, including when the
bundle output directory is specified with a relative path.

Installation stages a symbolic link or complete copy before replacing an
existing installation. Existing links, dangling links, and copies are replaced,
so specifying or omitting `--by-copy` switches installation methods.
A failed link creation, copy, or publication preserves the previous
installation. Plugins are installed individually in workspace runs. Unexpected
plugin file types are rejected; copy mode also rejects links inside a bundle.

After Effects loads the local bundle through the link, so bundling again to the
same path updates the installed plugin. Keep that bundle directory available;
moving or deleting it, including with `cargo clean`, breaks the installed link.

`uninstall` reads Cargo metadata without building, so it also works when the
plugin's source does not compile or build artifacts have been removed. It
removes installed links without following them, leaves their targets in place,
and also removes dangling links. Copied plugin files or directories are removed
as well; removal does not require an installation method. The command succeeds
when a plugin is already absent. Keep the plugin's configured name unchanged
until removal, since it determines the installed filename.

Close After Effects before updating or removing loaded plugins. The chosen
directory must be writable; the shared MediaCore directory may require
administrator privileges. These commands do not request privilege elevation
or invoke `sudo` automatically. On Windows, creating symbolic links requires
Developer Mode or administrator privileges; `--by-copy` only
requires write access to the installation directory.
