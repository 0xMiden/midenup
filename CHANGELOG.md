# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

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

[1.0.1]: https://github.com/0xMiden/midenup/releases/tag/v1.0.1
[1.0.0]: https://github.com/0xMiden/midenup/releases/tag/v1.0.0
