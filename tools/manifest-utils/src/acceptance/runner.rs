//! Installs a toolchain into a scratch home and runs scenarios against it.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, bail};
use midenup::manifest::Channel;
use tempfile::TempDir;

use super::scenarios::{self, Kind, Need, Scenario};

/// How a scenario ended.
pub enum Outcome {
    Pass,
    Fail(String),
    Skipped(String),
}

/// A midenup home with one toolchain installed, used by every scenario of that toolchain.
pub struct Home<'m> {
    /// Removed, with everything installed in it, when the home is dropped.
    _scratch: TempDir,
    /// The scratch directory, resolved.
    dir: PathBuf,
    midenup_home: PathBuf,
    cargo_home: PathBuf,
    manifest_uri: String,
    toolchain: &'m Channel,
}

impl<'m> Home<'m> {
    /// Installs `toolchain` from the manifest at `manifest_uri` with the `midenup` at `midenup`.
    pub fn install(
        midenup: &Path,
        manifest_uri: &str,
        toolchain: &'m Channel,
    ) -> anyhow::Result<Self> {
        let root = tempfile::Builder::new()
            .prefix("acceptance")
            .tempdir()
            .context("failed to create a scratch directory")?;
        // Resolved, because some build scripts compare canonical paths, and the temporary
        // directory sits behind a symlink on macOS (`/var` -> `/private/var`).
        let resolved = std::fs::canonicalize(root.path())?;
        let home = Self {
            midenup_home: resolved.join("midenup"),
            cargo_home: resolved.join("cargo"),
            dir: resolved,
            _scratch: root,
            manifest_uri: manifest_uri.to_string(),
            toolchain,
        };

        // Next to `miden`, as in a user's install: components such as the debugger run `midenup`
        // to find the active toolchain.
        let bin = home.cargo_home.join("bin");
        std::fs::create_dir_all(&bin)?;
        std::os::unix::fs::symlink(std::fs::canonicalize(midenup)?, bin.join("midenup"))?;

        let version = toolchain.name.to_string();
        for args in [vec!["init"], vec!["install", &version, "--profile", "complete"]] {
            let output = home
                .command(midenup)
                .args(&args)
                .output()
                .with_context(|| format!("failed to run {}", midenup.display()))?;
            if !output.status.success() {
                bail!("`midenup {}` failed:\n{}", args.join(" "), output_text(&output));
            }
        }
        Ok(home)
    }

    /// Runs `scenario`, whose lit suites live next to `scenarios_path`.
    pub fn run(&self, scenario: &Scenario, scenarios_path: &Path) -> anyhow::Result<Outcome> {
        let missing: Vec<&str> = scenario
            .requires
            .iter()
            .filter(|name| self.toolchain.get_component(name.as_str()).is_none())
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            return Ok(Outcome::Fail(format!(
                "requires {}, which toolchain {} does not have",
                missing.join(", "),
                self.toolchain.name
            )));
        }
        if scenario.needs.contains(&Need::Docker) && !docker_available() {
            return Ok(Outcome::Skipped("needs Docker, which this host does not have".into()));
        }

        let work = tempfile::Builder::new().prefix("scenario").tempdir_in(&self.dir)?;
        let output = match &scenario.kind {
            Kind::Script { script } => {
                // Beside the working directory, so the scenario starts in an empty one.
                let path = work.path().with_extension("sh");
                std::fs::write(&path, script)?;
                self.command("bash").arg(&path).current_dir(work.path()).output()?
            },
            Kind::Lit { suite } => {
                let copy = work.path().join(suite);
                copy_dir(&scenarios::suite_dir(scenarios_path, suite), &copy)?;
                add_features(&copy.join("lit.suite.toml"), self.features())?;
                self.command("litcheck")
                    .args(["lit", "run", "--verbose", "--path"])
                    .arg(self.cargo_home.join("bin"))
                    .arg(&copy)
                    .current_dir(work.path())
                    .output()
                    .context("failed to run litcheck; install it with `cargo install litcheck`")?
            },
        };
        Ok(if output.status.success() {
            Outcome::Pass
        } else {
            Outcome::Fail(output_text(&output))
        })
    }

    /// A command that sees this home, with `miden` on `PATH` and this toolchain selected.
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut path = OsString::from(self.cargo_home.join("bin"));
        if let Some(inherited) = std::env::var_os("PATH") {
            path.push(":");
            path.push(inherited);
        }

        let mut command = Command::new(program);
        command
            .env("MIDENUP_HOME", &self.midenup_home)
            .env("CARGO_HOME", &self.cargo_home)
            .env("MIDENUP_MANIFEST_URI", &self.manifest_uri)
            .env("MIDENUP_TOOLCHAIN", self.toolchain.name.to_string())
            .env("MIDEN_TOOLCHAIN_VERSION", self.toolchain.name.to_string())
            .env("PATH", path)
            .env_remove("XDG_DATA_HOME");
        for component in &self.toolchain.components {
            command.env(component_variable(&component.name), component.version.to_string());
        }
        command
    }

    /// `toolchain=<version>` and `<component>=<version>`, for lit's `REQUIRES` and `UNSUPPORTED`.
    fn features(&self) -> Vec<String> {
        let mut features = vec![format!("toolchain={}", self.toolchain.name)];
        for component in &self.toolchain.components {
            features.push(format!("{}={}", component.name, component.version));
        }
        features
    }
}

/// Appends `features` to the `available_features` of the lit suite configured in `suite_config`.
fn add_features(suite_config: &Path, features: Vec<String>) -> anyhow::Result<()> {
    let mut config: toml::Table = toml::from_str(&std::fs::read_to_string(suite_config)?)?;
    config
        .entry("available_features")
        .or_insert_with(|| toml::Value::Array(vec![]))
        .as_array_mut()
        .context("`available_features` must be an array")?
        .extend(features.into_iter().map(toml::Value::from));
    std::fs::write(suite_config, toml::to_string(&config)?)?;
    Ok(())
}

/// `MIDEN_COMPONENT_<NAME>_VERSION`, with the name uppercased and `-` replaced by `_`.
fn component_variable(name: &str) -> String {
    format!("MIDEN_COMPONENT_{}_VERSION", name.to_uppercase().replace('-', "_"))
}

fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn copy_dir(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_are_appended_to_the_suite_configuration() {
        let dir = tempfile::Builder::new().prefix("acceptance-features").tempdir().unwrap();
        let config = dir.path().join("lit.suite.toml");
        std::fs::write(&config, "name = 'debug'\navailable_features = ['native']\n").unwrap();
        add_features(&config, vec!["toolchain=0.17.0".into(), "vm=0.17.2".into()]).unwrap();

        let written: toml::Table =
            toml::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(written["name"].as_str(), Some("debug"));
        let features: Vec<&str> = written["available_features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|feature| feature.as_str().unwrap())
            .collect();
        assert_eq!(features, ["native", "toolchain=0.17.0", "vm=0.17.2"]);
    }

    #[test]
    fn component_variables_are_valid_environment_names() {
        assert_eq!(component_variable("vm"), "MIDEN_COMPONENT_VM_VERSION");
        assert_eq!(component_variable("remote-prover"), "MIDEN_COMPONENT_REMOTE_PROVER_VERSION");
    }
}
