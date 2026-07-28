use std::{ffi::OsString, path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::{channel::Channel, config::Config, manifest::Component};

/// Represents an executable action that can be invoked by the `miden` CLI
#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(try_from = "Vec::<String>", into = "Vec::<String>")]
pub struct Executable {
    args: Vec<Expr>,
}

impl Executable {
    pub fn default_call_format() -> Self {
        Self { args: vec![Expr::Executable] }
    }
}

impl From<Executable> for Vec<String> {
    fn from(value: Executable) -> Self {
        let mut out = Vec::with_capacity(value.args.len());

        for expr in value.args {
            match expr {
                Expr::Executable => out.push("%installed-executable".to_string()),
                Expr::LibPath(None) => out.push("%lib".to_string()),
                Expr::LibPath(Some(name)) => out.push(format!("%lib({name})")),
                Expr::VarPath(None) => out.push("%var".to_string()),
                Expr::VarPath(Some(name)) => out.push(format!("%var({name})")),
                Expr::EtcPath(name) => out.push(format!("%etc({name})")),
                Expr::Verbatim(expr) => out.push(expr),
            }
        }

        out
    }
}

impl TryFrom<Vec<crate::manifest::v1::CliCommand>> for Executable {
    type Error = InvalidExecutable;

    fn try_from(values: Vec<crate::manifest::v1::CliCommand>) -> Result<Self, Self::Error> {
        use crate::manifest::v1::CliCommand;

        let mut exprs = Vec::with_capacity(values.len());
        let mut values = values.into_iter();

        while let Some(value) = values.next() {
            match value {
                CliCommand::Executable => exprs.push(Expr::Executable),
                CliCommand::LibPath => exprs.push(Expr::LibPath(None)),
                CliCommand::VarPath => {
                    let subdir = values.next();
                    match subdir {
                        None => exprs.push(Expr::VarPath(None)),
                        Some(CliCommand::Verbatim(arg)) => exprs.push(Expr::VarPath(Some(arg))),
                        Some(
                            cmd @ (CliCommand::Executable
                            | CliCommand::LibPath
                            | CliCommand::VarPath),
                        ) => return Err(InvalidExecutable::InvalidVarExpr(cmd.to_string())),
                    }
                },
                CliCommand::Verbatim(arg) => exprs.push(Expr::Verbatim(arg)),
            }
        }

        if exprs.is_empty() {
            Err(InvalidExecutable::Empty)
        } else {
            Ok(Self { args: exprs })
        }
    }
}

impl TryFrom<Vec<String>> for Executable {
    type Error = InvalidExecutable;

    fn try_from(values: Vec<String>) -> Result<Self, Self::Error> {
        let mut exprs = Vec::with_capacity(values.len());

        for value in values {
            exprs.push(value.parse::<Expr>()?);
        }

        if exprs.is_empty() {
            Err(InvalidExecutable::Empty)
        } else {
            Ok(Self { args: exprs })
        }
    }
}

/// Represents each possible "word" variant that is passed to the command line.
///
/// These are used to resolve an [Alias] to its associated command.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Expr {
    /// Resolve the command to the installed executable of the containing component
    Executable,
    /// Resolve the command to a toolchain library directory (`<toolchain>/lib`)
    ///
    /// Optionally, it can contain a file name, which represents a file in `<toolchain>/lib/<file>`.
    LibPath(Option<String>),
    /// Resolve the command to a toolchain var directory (`<toolchain>/var`).
    ///
    /// Optionally, it can contain a file name, which represents a file in `<toolchain>/var/<file>`.
    VarPath(Option<String>),
    /// Resolve the command to a file in the toolchain etc directory (`<toolchain>/etc/<file>`).
    EtcPath(String),
    /// An argument that is passed verbatim, as is.
    Verbatim(String),
}

impl FromStr for Expr {
    type Err = InvalidExecutable;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        #[inline]
        fn parse_parenthesized(input: &str) -> Option<&str> {
            input.strip_prefix('(')?.strip_suffix(')')
        }

        if value == "%installed-executable" {
            Ok(Expr::Executable)
        } else if let Some(rest) = value.strip_prefix("%lib") {
            if rest.is_empty() {
                return Ok(Expr::LibPath(None));
            }
            let name = parse_parenthesized(rest)
                .ok_or_else(|| InvalidExecutable::InvalidLibExpr(rest.to_string()))?;
            Ok(Expr::LibPath(Some(name.to_string())))
        } else if let Some(rest) = value.strip_prefix("%var") {
            if rest.is_empty() {
                return Ok(Expr::VarPath(None));
            }
            let name = parse_parenthesized(rest)
                .ok_or_else(|| InvalidExecutable::InvalidVarExpr(rest.to_string()))?;
            Ok(Expr::VarPath(Some(name.to_string())))
        } else if let Some(rest) = value.strip_prefix("%etc") {
            if rest.is_empty() {
                return Err(InvalidExecutable::MissingEtcPath);
            }
            let name = parse_parenthesized(rest)
                .ok_or_else(|| InvalidExecutable::InvalidEtcExpr(rest.to_string()))?;
            Ok(Expr::EtcPath(name.to_string()))
        } else {
            Ok(Expr::Verbatim(value.to_string()))
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidExecutable {
    #[error("invalid executable: empty executable expression")]
    Empty,
    #[error("invalid executable: expected expression to start with an executable")]
    NotExecutable,
    #[error("invalid executable: component '{0}' is not executable, but was referenced as one")]
    NotAExecutable(String),
    #[error(
        "invalid executable: invalid expr: `%etc` requires specifying a subpath, e.g. \
         `%etc(foo.config)`"
    )]
    MissingEtcPath,
    #[error(
        "invalid executable: invalid `%etc` expr: expected format is '%etc(path/to/file)', got \
         `{0}`"
    )]
    InvalidEtcExpr(String),
    #[error(
        "invalid executable: invalid `%lib` expr: expected format is one of `%lib` or \
         '%lib(path/to/file)', got `{0}`"
    )]
    InvalidLibExpr(String),
    #[error(
        "invalid executable: invalid `%var` expr: expected format is one of `%var` or \
         '%var(path/to/file)', got `{0}`"
    )]
    InvalidVarExpr(String),
    #[error("invalid executable: '{0}' does not exist")]
    InvalidFile(PathBuf),
    #[error("invalid executable: unknown package component '{0}'")]
    UnknownPackage(String),
}

impl Executable {
    pub fn is_empty(&self) -> bool {
        self.args.is_empty()
    }

    /// Resolve this [Executable] to an argument vector that can be passed to
    /// [std::process::Command].
    ///
    /// It is guaranteed that the vector will be non-empty, and that the first argument is the
    /// executable that should be invoked.
    ///
    /// It is not guaranteed that the executable is _actually_ executable - we leave that to the OS.
    pub fn to_argv(
        &self,
        component: &Component,
        channel: &Channel,
        config: &Config,
    ) -> Result<Vec<OsString>, InvalidExecutable> {
        let toolchain_dir = config.toolchain_dir(channel);

        let mut argv = Vec::with_capacity(self.args.len());
        for expr in self.args.iter() {
            match expr {
                Expr::Executable => {
                    if let Some(cli) = component.get_cli_display() {
                        argv.push(cli.into());
                    } else {
                        return Err(InvalidExecutable::NotAExecutable(component.name.to_string()));
                    }
                },
                Expr::LibPath(None) => {
                    if argv.is_empty() {
                        return Err(InvalidExecutable::NotExecutable);
                    }
                    argv.push(toolchain_dir.join("lib").into_os_string());
                },
                Expr::LibPath(Some(file)) => {
                    if argv.is_empty() {
                        return Err(InvalidExecutable::NotExecutable);
                    }
                    let path = toolchain_dir.join("lib").join(file);
                    if path.try_exists().is_ok_and(|exists| exists) {
                        argv.push(path.into_os_string());
                    } else {
                        return Err(InvalidExecutable::InvalidFile(path));
                    }
                },
                Expr::VarPath(None) => {
                    if argv.is_empty() {
                        return Err(InvalidExecutable::NotExecutable);
                    }
                    argv.push(var_dir(channel, config)?.into_os_string());
                },
                Expr::VarPath(Some(file)) => {
                    if argv.is_empty() {
                        return Err(InvalidExecutable::NotExecutable);
                    }
                    // Deliberately not checked for existence. `%var` names *mutable* state that
                    // the component owns and creates -- `%var(data)` is the client's database
                    // directory, which does not exist until the client makes it. Requiring it
                    // here made `miden start-node` fail on every fresh installation.
                    argv.push(var_dir(channel, config)?.join(file).into_os_string());
                },
                Expr::EtcPath(file) => {
                    let path = toolchain_dir.join("etc").join(file);
                    if path.try_exists().is_ok_and(|exists| exists) {
                        argv.push(path.into_os_string());
                    } else {
                        return Err(InvalidExecutable::InvalidFile(path));
                    }
                },
                Expr::Verbatim(arg) => {
                    argv.push(arg.clone().into());
                },
            }
        }

        if argv.is_empty() {
            Err(InvalidExecutable::Empty)
        } else {
            Ok(argv)
        }
    }
}

/// The channel's `var/` directory, created on demand.
///
/// It lives outside the publication (`$MIDENUP_HOME/var/<channel>`), so it survives every
/// republication of the toolchain. Created here, at dispatch, because nothing in the installation
/// path is allowed to touch it -- creating it at install time would be one step away from
/// replacing it at install time.
fn var_dir(channel: &Channel, config: &Config) -> Result<std::path::PathBuf, InvalidExecutable> {
    let dir = crate::paths::var_dir(&config.midenup_home, &channel.name);
    std::fs::create_dir_all(&dir).map_err(|_| InvalidExecutable::InvalidFile(dir.clone()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    /// `%var` must resolve outside the publication. Inside it, every toolchain update deleted the
    /// client's database along with the publication it replaced.
    #[test]
    fn var_resolves_outside_the_publication_and_is_created_on_demand() {
        let temp = tempdir::TempDir::new("exec-var").unwrap();
        let home = temp.path().join("midenup");

        let dir = crate::paths::var_dir(&home, &semver::Version::new(0, 15, 0));
        assert_eq!(dir, home.join("var").join("0.15.0"));
        assert!(!dir.starts_with(crate::paths::publications_dir(&home)));

        std::fs::create_dir_all(&dir).unwrap();
        assert!(dir.is_dir());
    }
}
