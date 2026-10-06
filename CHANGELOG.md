# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

## [1.2.1]

### Added

- `midenup show sysroot` prints the active toolchain's installation path for use as
  `MIDEN_SYSROOT`. If the active toolchain is not installed, it reports an error on stderr and
  exits with status 2.
- `midenup --version` now accepts `=plain` for just the release version, `=revision` for the Git
  revision, or `=json` for machine-readable version and environment information. Bare
  `--version` and `--version=detailed` retain the detailed report.

### Migration and breaking changes

- Rust API users calling `miden_wrapper::display_version` must pass a
  `commands::VersionStyle` argument after `config`; use `VersionStyle::Detailed` to retain the
  previous output. Add `ShowCommand::Sysroot` to exhaustive matches on `ShowCommand`.

## [1.2.0]

### Added

- Named toolchains: add `name = "project-dev"` to `[toolchain]` in `miden-toolchain.toml` to
  install an independent variant of a version or network. Select it elsewhere with
  `custom:project-dev`. Updates preserve its source and patches; network variants follow
  promotions independently of the shared installation. Each variant has separate runtime data,
  preserved on uninstall unless `--purge` is given.
- `midenup install` now accepts an omitted channel, selecting the active toolchain from
  `MIDENUP_TOOLCHAIN`, `miden-toolchain.toml`, the system default, or finally `mainnet`, in that
  order. When the project file selects the toolchain, its profile, components and patches apply;
  `--profile` overrides the profile and `--component` adds components.

### Changed

- `midenup gc` now reclaims Cargo path-build caches when no installed toolchain or pending
  operation uses their source. Caches shared by multiple variants remain until their last
  reference is removed.
- Installation documentation now lists Rust and the Docker Compose version needed for `miden node`.

### Fixed

- Fixed `miden` mixing executable, library, sysroot or subprocess PATH files from different
  installations when another process installs or updates a toolchain during command execution.
- Fixed reinstallations retaining stale files after a component's definition, source or output
  ownership changes.
- Fixed local path builds reusing another checkout's binary when Cargo has a shared target or
  build directory. Each source now has its own build cache under `$MIDENUP_HOME/cache/cargo`.
- Invalid project installation definitions no longer block unrelated maintenance and reporting
  commands. Bare updates reject invalid definitions and conflicting named toolchains before
  changing any installations.
- Network-identity conflicts now explain that a new toolchain name is required, instead of
  suggesting a reinstall that would also fail.
- Fixed startup migration losing named defaults such as `custom:stable` and their runtime data.

### Migration and breaking changes

- Installing without a channel replaces the installed component selection with the active
  project's request and any CLI overrides, potentially removing other components. Include every
  component you want to retain in the project file or with `--component`.
- Project patches now require `[toolchain] name`. Add a unique name to each patched project's
  `miden-toolchain.toml` and run `midenup install` there. Patches affect that named installation
  instead of the shared channel; select it outside the project with `custom:<name>`. Updates
  preserve its patches; installing or updating the upstream channel no longer resets them.
- Projects sharing a name must agree on its source and patches. After changing either, run
  `midenup install` from the declaring project to reconcile the definition. Switching networks,
  or switching between a network and a pinned version, requires a new name. A toolchain file
  selecting `channel = "custom:<name>"` must omit `name`, `profile`, `components` and `[patches]`;
  keep those settings in the declaring file.
- Bare `midenup update` now updates installed toolchains and then installs the active toolchain
  and any missing requested components. On a fresh installation it requires an upstream
  manifest or cached copy instead of reporting nothing to update. Pass an explicit installed
  version to update only that toolchain. An active network keeps its local selection if it
  satisfies the project; missing requirements can install the network's current upstream version.
  Pin a version to avoid following network promotions.
- The GitHub Action now uses `miden-toolchain.toml` in the working directory when its `toolchain`
  input is omitted, instead of installing `mainnet`. In this mode `profile` and `components`
  inputs are ignored; put them in the file, or pass an explicit `toolchain` input to retain
  input-based selection.
- Local state is upgraded from schema version 1 to version 2 on write without reinstalling
  existing publications. Older `midenup` binaries cannot read version 2; keep using this version
  or newer with an upgraded home directory.
- Custom manifests may no longer use network names `custom`, `default` or names beginning with
  `custom:`. Rename these networks and update their selectors.
- Rust API users must change `InstallationOptions.profile` from `Profile` to `Option<Profile>`:
  wrap explicit profiles in `Some(...)` and handle `None`. An omitted profile uses the active
  project's profile for a bare install, or `minimal` for an explicit-channel install.
- Add `custom: None` to `InstallationOptions` and `state::Installation` struct literals for
  canonical installations, and add `custom: None` and `previous_channel: None` to
  `publish::JournalEntry` literals. Handle `UserChannel::Custom` in exhaustive matches. In
  `PublishError::OperationInProgress` and `PublishError::DivergentState`, replace the `channel`
  field with `installation: InstallationId`.
- Calls to `install::cargo::build`, `install::cargo::argv_for` and `install::stage::execute`
  must pass a `build_cache: &Path` after `staging_root`. Use a persistent cache directory such as
  `$MIDENUP_HOME/cache/cargo`.

## [1.1.0]

### Added

- `miden-toolchain.toml` now accepts a `[patches]` table that builds individual components from a
  git repository, a local path or another registry version instead of the channel's published
  release. Patched executables are built with cargo; legacy packages distributed as Rust crates
  are extracted from the patched crate. Relative paths resolve from the toolchain file, and
  patching an executable published only pre-built requires `crate_name`.
- Command argument templates can now combine literal text with multiple substitutions in one
  argument, use `%{...}` to separate substitutions from adjacent text, and use `%version` for the
  owning component's registry version.
- Added the 0.17.0 toolchain with updated release candidates, including client 0.17.0-rc.5,
  node 0.17.0-rc.4, protocol 0.17.0-rc.9, cargo-miden and midenc 0.11.0-rc.3, and core/VM 0.35.0.

### Changed

- Updated `miden-verify` in the 0.16.0 toolchain from 0.5.0 to 0.6.0.
- Interactive path-update prompts now name the component being updated.

### Fixed

- Fixed `midenup --version` showing an empty revision when `midenup` was installed from crates.io,
  and reporting the active toolchain as `unknown` when it is simply not installed yet. An
  uninstalled toolchain is reported as `not installed`, without a network name.
- Fixed the parentheses around update instructions in `midenup show`.
- Fixed installations reusing a component's previous build after its patch is added, changed or
  removed.
- Fixed updates leaving patches recorded when they match the published component definitions;
  these patches are now cleared without rebuilding unchanged components. Network moves and
  channel migrations also reconcile patches and changed components in an already installed target.

### Migration and breaking changes

- `midenup install` now makes the installed toolchain the global default when no default is set.
  Use `midenup override <TOOLCHAIN>` afterward if a different default is required.
- `devnet` now tracks 0.17.0 instead of 0.16.0. Pin `0.16.0` in `miden-toolchain.toml` or use
  `midenup override 0.16.0` to keep using that toolchain instead of following devnet.
- The 0.16.0 and 0.17.0 `miden node` commands now load the versioned
  `ghcr.io/0xmiden/miden-local-network` Compose configuration through an `oci://` reference,
  replacing downloaded Compose files and, for 0.16.0, correcting the Docker image references.
  Use a Docker Compose installation that supports OCI configurations, and update workflows that
  relied on the installed `etc/node` Compose files.
- Patches affect the channel's shared installation. Running `miden` outside the project or with
  different patches reinstalls affected components. `midenup install` and `midenup update` restore
  the published components regardless of `--path-update`; the next `miden` run inside the project
  reapplies its patches. Patched installations are always marked `(update available)`.
- Manifest command arguments now interpret `%` substitutions anywhere in the string. Escape
  literal percent signs as `%%` (for example, change a literal `printf` argument `%s` to `%%s`);
  bare percent signs, unknown keywords and unclosed delimiters are errors. Legacy v1 verbatim
  arguments keep their literal meaning.
- Custom manifests now reject invalid executable argument formats and `%version` in arguments or
  artifact URIs on components with a git or path authority. Correct invalid formats, and replace
  those version substitutions with explicit values or use a registry authority.
- Rust API users constructing `InstallationOptions`, `state::Installation` or `Toolchain` with
  struct literals must add `patches: Default::default()` when no patches are needed. Update
  exhaustive matches on `Expr` for `Template` and `Version`, and on `InvalidExecutable` and
  `manifest::validate::ValidationError` for the new template and manifest validation errors.

## [1.0.1]

### Added

- Added a composite GitHub Action (`uses: 0xMiden/midenup@v1`) that installs `midenup` and a toolchain.
- Added an installer script, published as `installer.sh` with each release, that installs a pre-built
  `midenup` executable and runs `midenup init`.
- Added the `MIDENUP_TOOLCHAIN` environment variable as an override for the active toolchain.
- `midenup show` and `midenup list` now mark an installed toolchain with `(update available)` when
  the manifest has moved past it.
- `midenup install <network>` now prints the toolchain version the network resolves to.
- Installations now record the `midenup` version that produced them, for use by future migrations.

### Changed

- Updated the 0.16.0 toolchain, updated the client aliases, and removed the `deploy` alias.
- The `mainnet` and `testnet` networks now point at the 0.16.0 toolchain.

### Fixed

- Fixed the `miden` wrapper discarding the exit code of the command it ran.
- Fixed components installed with `--component` outside any profile being missing from
  `miden help toolchain` and treated as outside the toolchain when no `miden-toolchain.toml` is
  present.
- Fixed network links: only the networks the user installed are linked and listed, uninstalling a
  network no longer unlinks a channel that other networks still name, and links follow a migration
  to its target toolchain.
- Fixed an interrupted uninstall discarding pending network cleanup on recovery.
- Fixed the partial-install marker being derived from the full channel instead of what was
  requested.
- Fixed a successor channel being reported as an update when `midenup update` would not migrate to
  it.
- Fixed comparison of relative-path component authorities against their installed absolute form.
- Fixed artifacts that install into subdirectories, and rejected manifests where nested artifacts
  claim the same path as both a file and a directory.
- Fixed the installer's cargo fallback ignoring the requested version.
- Fixed the formatting of the libraries list in `miden help toolchain`.

## [1.0.0]

### Added

- Added configurable installation and update progress, including work summaries, numbered
  component steps, transfer rates, elapsed-time displays, completion messages, and selected
  low-level trace diagnostics.
- Added per-command output controls: `-q`/`--quiet`, `-v`/`--verbose[=<LEVEL>]`,
  `--progress[=<STYLE>]`, `--no-progress`, `--color[=<WHEN>]`, and `--plain`.

### Changed

- Updated the 0.16.0 devnet toolchain to use the 0.16.0-rc.6 protocol package and the 0.29.1
  core package.

### Fixed

- Fixed `miden deploy` for the 0.15.0 and 0.16.0 toolchains so it creates and deploys a public
  account.
- Renamed command components now report the correct `miden <command>` invocation instead of
  aborting when invoked by their component name.
- Cargo compiler errors and other child-process diagnostics, including prompts without trailing
  newlines and non-UTF-8 output, now remain visible alongside live progress. Descendants that
  inherit stderr no longer leave installations waiting indefinitely.
- Automatic color detection now follows each destination stream and honors `CLICOLOR`,
  `NO_COLOR`, and `CLICOLOR_FORCE`, keeping redirected stdout free of ANSI escapes while
  retaining color on interactive stderr.
- `midenup update` now checks local state before fetching the upstream manifest, so checking an
  empty installation works offline and a missing installed version is not masked by a network
  error.
- Interactive path-update prompts are flushed before input, remain visible in quiet mode, and no
  longer mix acknowledgements with command results.
- One-item installation summaries now say `1 step` instead of `1 steps`.

### Migration and breaking changes

- Output and debug-build flags are now scoped to their command. Move them from before the command
  to after it; for example, replace `midenup --verbose install stable` with
  `midenup install stable --verbose`. Place flags for `show` after `active-toolchain` or `list`;
  `show home` accepts no reporting flags.
- Stdout is now reserved for command results. Progress, status messages, warnings, traces,
  subprocess diagnostics, and interactive prompts use stderr, so update scripts and redirections
  that consumed them from stdout. Full spawned-program output is suppressed at the default level;
  pass `-v` or `--verbose=debug` to show it.
- `midenup show active-toolchain --verbose` now writes only the selected channel to stdout and its
  selection explanation to stderr. Update parsers that expected the former explanatory sentence
  on stdout.
- The 0.16.0 toolchain replaces the `miden send` alias with `miden transfer`; update invocations
  and scripts accordingly.
- Rust API users must remove the `verbose` field from `InstallationOptions` and `UpdateOptions`,
  pass the new `verbose: bool` argument to `install::extract::extract`, and update `ShowCommand`
  construction and matches to use `Current { flags }` and `List { flags }`. Reporting is now
  configured through `report::set`.

[1.2.1]: https://github.com/0xMiden/midenup/releases/tag/v1.2.1
[1.2.0]: https://github.com/0xMiden/midenup/releases/tag/v1.2.0
[1.1.0]: https://github.com/0xMiden/midenup/releases/tag/v1.1.0
[1.0.1]: https://github.com/0xMiden/midenup/releases/tag/v1.0.1
[1.0.0]: https://github.com/0xMiden/midenup/releases/tag/v1.0.0
