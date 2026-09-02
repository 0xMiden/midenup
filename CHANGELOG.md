# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

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

[1.0.0]: https://github.com/0xMiden/midenup/releases/tag/v1.0.0
