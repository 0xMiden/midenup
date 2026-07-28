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
                    argv.push(toolchain_dir.join("var").into_os_string());
                },
                Expr::VarPath(Some(file)) => {
                    if argv.is_empty() {
                        return Err(InvalidExecutable::NotExecutable);
                    }
                    let path = toolchain_dir.join("var").join(file);
                    if path.try_exists().is_ok_and(|exists| exists) {
                        argv.push(path.into_os_string());
                    } else {
                        return Err(InvalidExecutable::InvalidFile(path));
                    }
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
