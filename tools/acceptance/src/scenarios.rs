//! The scenario file: `acceptance/scenarios.toml`.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use semver::{Op, VersionReq};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    scenarios: Vec<Scenario>,
}

/// One acceptance scenario.
#[derive(Debug, Deserialize)]
#[serde(try_from = "Fields")]
pub struct Scenario {
    pub title: String,
    /// The toolchains this scenario applies to; must have an upper bound.
    pub toolchain: VersionReq,
    /// The components this scenario exercises.
    pub requires: Vec<String>,
    /// What the scenario needs from the host besides the toolchain.
    pub needs: Vec<Need>,
    pub kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Need {
    Docker,
}

#[derive(Debug)]
pub enum Kind {
    /// A shell script that passes when it exits successfully.
    Script { script: String },
    /// A lit suite in `acceptance/<suite>`, run with `litcheck`.
    Lit { suite: String },
}

/// A scenario as written, so that a misspelled field is an error rather than ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fields {
    title: String,
    toolchain: VersionReq,
    requires: Vec<String>,
    #[serde(default)]
    needs: Vec<Need>,
    kind: String,
    script: Option<String>,
    suite: Option<String>,
}

impl TryFrom<Fields> for Scenario {
    type Error = String;

    fn try_from(fields: Fields) -> Result<Self, String> {
        let kind = match (fields.kind.as_str(), fields.script, fields.suite) {
            ("script", Some(script), None) => Kind::Script { script },
            ("lit", None, Some(suite)) => Kind::Lit { suite },
            ("script", ..) => {
                return Err("a `script` scenario takes `script` and no `suite`".into());
            },
            ("lit", ..) => return Err("a `lit` scenario takes `suite` and no `script`".into()),
            (other, ..) => {
                return Err(format!("unknown kind '{other}'; expected `script` or `lit`"));
            },
        };
        Ok(Self {
            title: fields.title,
            toolchain: fields.toolchain,
            requires: fields.requires,
            needs: fields.needs,
            kind,
        })
    }
}

/// Reads and checks the scenarios in `path`; lit suites are resolved next to it.
pub fn load(path: &Path) -> anyhow::Result<Vec<Scenario>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let file: File =
        toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;

    for scenario in &file.scenarios {
        if !is_bounded(&scenario.toolchain) {
            bail!(
                "scenario '{}': toolchain range '{}' has no upper bound; name the toolchains it \
                 covers explicitly",
                scenario.title,
                scenario.toolchain
            );
        }
        if scenario.requires.is_empty() {
            bail!("scenario '{}': `requires` must name at least one component", scenario.title);
        }
        if let Kind::Lit { suite } = &scenario.kind {
            let dir = suite_dir(path, suite);
            if !dir.join("lit.suite.toml").is_file() {
                bail!(
                    "scenario '{}': lit suite '{suite}' has no {}",
                    scenario.title,
                    dir.join("lit.suite.toml").display()
                );
            }
        }
    }
    Ok(file.scenarios)
}

/// Where the lit suite named `suite` lives, relative to the scenario file.
pub fn suite_dir(scenarios_path: &Path, suite: &str) -> PathBuf {
    scenarios_path.parent().unwrap_or(Path::new(".")).join(suite)
}

/// Whether `req` excludes every version past some point.
fn is_bounded(req: &VersionReq) -> bool {
    !req.comparators.is_empty()
        && req
            .comparators
            .iter()
            .any(|comparator| !matches!(comparator.op, Op::Greater | Op::GreaterEq))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounded(req: &str) -> bool {
        is_bounded(&VersionReq::parse(req).unwrap())
    }

    #[test]
    fn only_ranges_with_an_upper_bound_are_accepted() {
        assert!(bounded("^0.17.0"));
        assert!(bounded("0.17.*"));
        assert!(bounded(">=0.17.0, <0.19.0"));
        assert!(bounded("=0.17.0"));
        assert!(!bounded(">=0.17.0"));
        assert!(!bounded(">0.16"));
        assert!(!bounded("*"));
    }

    #[test]
    fn a_misspelled_field_or_mismatched_kind_is_an_error() {
        let parse = |body: &str| {
            toml::from_str::<Scenario>(&format!("title = 't'\ntoolchain = '^0.17.0'\n{body}"))
        };
        assert!(parse("requries = ['vm']\nkind = 'script'\nscript = 'true'").is_err());
        assert!(parse("requires = ['vm']\nkind = 'script'\nsuite = 'x'").is_err());
        assert!(parse("requires = ['vm']\nkind = 'shell'\nscript = 'true'").is_err());
        assert!(parse("requires = ['vm']\nkind = 'script'\nscript = 'true'").is_ok());
    }

    #[test]
    fn a_scenario_file_parses_both_kinds() {
        let file: File = toml::from_str(
            r#"
            [[scenarios]]
            title = "compile and prove"
            toolchain = "^0.17.0"
            requires = ["vm", "midenc"]
            kind = "script"
            script = "true"

            [[scenarios]]
            title = "debug"
            toolchain = "^0.17.0"
            requires = ["debug"]
            needs = ["docker"]
            kind = "lit"
            suite = "debug-midenc-produced"
            "#,
        )
        .unwrap();
        assert!(matches!(file.scenarios[0].kind, Kind::Script { .. }));
        assert!(
            matches!(&file.scenarios[1].kind, Kind::Lit { suite } if suite == "debug-midenc-produced")
        );
        assert_eq!(file.scenarios[1].needs, [Need::Docker]);
    }
}
