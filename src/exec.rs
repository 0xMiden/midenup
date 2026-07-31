use std::{ffi::OsString, fmt, path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::manifest::Component;

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

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Executable => f.write_str("%installed-executable"),
            Expr::LibPath(None) => f.write_str("%lib"),
            Expr::LibPath(Some(name)) => write!(f, "%lib({name})"),
            Expr::VarPath(None) => f.write_str("%var"),
            Expr::VarPath(Some(name)) => write!(f, "%var({name})"),
            Expr::EtcPath(name) => write!(f, "%etc({name})"),
            Expr::Verbatim(arg) => f.write_str(arg),
        }
    }
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
    #[error(
        "component '{component}' refers to {expression}, but '{path}' is not in the installed \
         toolchain"
    )]
    MissingPath {
        component: String,
        expression: String,
        path: PathBuf,
    },
    #[error("unable to create the mutable state directory '{path}': {reason}")]
    Var { path: PathBuf, reason: String },
    #[error("invalid executable: unknown package component '{0}'")]
    UnknownPackage(String),
}

/// Where `%`-expressions resolve to, for one invocation.
///
/// Built once per dispatch and passed down, so that every expression in every alias resolves
/// against the same publication -- an invocation that resolved `%lib` against one toolchain and
/// `%etc` against another would be a very quiet kind of wrong.
#[derive(Debug, Clone)]
pub struct Resolver {
    /// The active publication, reached through `toolchains/<channel>`.
    sysroot: PathBuf,
    /// `$MIDENUP_HOME/var/<channel>`: mutable state, deliberately *outside* the publication, so it
    /// survives every republication of the toolchain (spec section 3.2).
    var: PathBuf,
}

impl Resolver {
    pub fn new(
        sysroot: impl Into<PathBuf>,
        home: &std::path::Path,
        channel: &semver::Version,
    ) -> Self {
        Self {
            sysroot: sysroot.into(),
            var: crate::paths::var_dir(home, channel),
        }
    }

    /// Resolves one expression on behalf of `component`.
    pub fn resolve(
        &self,
        expr: &Expr,
        component: &Component,
    ) -> Result<OsString, InvalidExecutable> {
        match expr {
            // The `opt/` shim, when the component has one: it is a real file in the publication,
            // and executing it by path gives `clap` the `argv[0]` that makes help read
            // `miden vm ...` rather than `miden-vm ...` (spec section 3.3). Falling back to
            // `bin/` covers hidden components, which have no shim by definition.
            Expr::Executable => {
                let installed = component
                    .installed_executable()
                    .ok_or_else(|| InvalidExecutable::NotAExecutable(component.name.to_string()))?;

                let path = match component.get_symlink_name() {
                    Some(shim) => self.sysroot.join("opt").join(shim),
                    None => self.sysroot.join("bin").join(installed),
                };
                Ok(path.into_os_string())
            },
            Expr::LibPath(None) => Ok(self.sysroot.join("lib").into_os_string()),
            Expr::LibPath(Some(file)) => {
                self.existing(self.sysroot.join("lib").join(file), component, expr)
            },
            Expr::EtcPath(file) => {
                self.existing(self.sysroot.join("etc").join(file), component, expr)
            },
            // Deliberately not checked for existence, and created on demand. `%var` names *mutable*
            // state the component owns and creates -- `%var(data)` is the client's database
            // directory, which does not exist until the client makes it. Requiring it made
            // `miden start-node` fail on every fresh installation.
            Expr::VarPath(file) => {
                std::fs::create_dir_all(&self.var).map_err(|source| InvalidExecutable::Var {
                    path: self.var.clone(),
                    reason: source.to_string(),
                })?;
                Ok(match file {
                    Some(file) => self.var.join(file).into_os_string(),
                    None => self.var.clone().into_os_string(),
                })
            },
            Expr::Verbatim(arg) => Ok(arg.clone().into()),
        }
    }

    /// A path that must already be in the publication, reported against the component that asked
    /// for it.
    ///
    /// `%lib` and `%etc` name *installed* files. One that is missing means the toolchain is not
    /// what its receipt says it is, which is worth saying plainly -- passing the path through and
    /// letting the component fail on it names the wrong culprit.
    fn existing(
        &self,
        path: PathBuf,
        component: &Component,
        expr: &Expr,
    ) -> Result<OsString, InvalidExecutable> {
        if path.try_exists().is_ok_and(|exists| exists) {
            Ok(path.into_os_string())
        } else {
            Err(InvalidExecutable::MissingPath {
                component: component.name.to_string(),
                expression: expr.to_string(),
                path,
            })
        }
    }
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
        resolver: &Resolver,
    ) -> Result<Vec<OsString>, InvalidExecutable> {
        let mut argv = Vec::with_capacity(self.args.len());

        for expr in self.args.iter() {
            // A path expression cannot be the program itself: `miden` would be asking the OS to
            // execute a library directory.
            if argv.is_empty() && matches!(expr, Expr::LibPath(_) | Expr::VarPath(_)) {
                return Err(InvalidExecutable::NotExecutable);
            }
            argv.push(resolver.resolve(expr, component)?);
        }

        if argv.is_empty() {
            Err(InvalidExecutable::Empty)
        } else {
            Ok(argv)
        }
    }
}

/// Composes the argv for a command component, per spec section 13.3.
///
/// ```text
/// with subcommands:     resolve(format) ++ resolve(subcommands[argv[1]]) ++ argv[2..]
/// without subcommands:  resolve(format) ++ argv[1..]
/// ```
///
/// The `format` prefix used to be dropped whenever a subcommand matched, so a component that
/// declared both -- `format: ["docker", "compose", "-f", "%etc(...)"]` plus `subcommands: {up:
/// ["up", "-d"]}` -- executed `up -d` as though it were a program. Nothing shipped in that shape
/// yet, which is the only reason it was not visible.
pub fn compose(
    component: &Component,
    format: &Executable,
    subcommand: Option<&Executable>,
    user_args: impl IntoIterator<Item = OsString>,
    resolver: &Resolver,
) -> Result<Vec<OsString>, InvalidExecutable> {
    let mut argv = if format.is_empty() {
        Vec::new()
    } else {
        format.to_argv(component, resolver)?
    };

    if let Some(subcommand) = subcommand {
        for expr in subcommand.args.iter() {
            argv.push(resolver.resolve(expr, component)?);
        }
    }

    argv.extend(user_args);

    if argv.is_empty() {
        Err(InvalidExecutable::Empty)
    } else {
        Ok(argv)
    }
}

#[cfg(test)]
mod tests {
    use std::{borrow::Cow, collections::BTreeMap};

    use super::*;
    use crate::manifest::{ComponentKind, ExecutableComponent};

    const CHANNEL: semver::Version = semver::Version::new(0, 15, 0);

    /// A `MIDENUP_HOME` with one publication, reached through `toolchains/0.15.0`.
    struct Env {
        _temp: tempdir::TempDir,
        home: std::path::PathBuf,
        sysroot: std::path::PathBuf,
    }

    impl Env {
        fn new() -> Self {
            let temp = tempdir::TempDir::new("exec").unwrap();
            let home = temp.path().join("midenup");
            let sysroot = home.join("publications").join("0.15.0-abc");
            for dir in ["bin", "lib", "etc", "opt"] {
                std::fs::create_dir_all(sysroot.join(dir)).unwrap();
            }
            Env { _temp: temp, home, sysroot }
        }

        fn resolver(&self) -> Resolver {
            Resolver::new(self.sysroot.clone(), &self.home, &CHANNEL)
        }

        fn write(&self, relative: &str) -> std::path::PathBuf {
            let path = self.sysroot.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"x").unwrap();
            path
        }
    }

    fn executable(words: &[&str]) -> Executable {
        Executable::try_from(words.iter().map(|w| w.to_string()).collect::<Vec<_>>()).unwrap()
    }

    fn component(name: &'static str, kind: ComponentKind) -> Component {
        Component {
            name: Cow::Borrowed(name),
            version: crate::version::Authority::Registry { version: semver::Version::new(0, 1, 0) },
            kind,
            profiles: vec![],
            requires: vec![],
            artifacts: Default::default(),
            extra: Default::default(),
        }
    }

    /// `node`: a command component with a `format` prefix and two subcommands.
    fn node() -> Component {
        let mut subcommands = BTreeMap::new();
        subcommands.insert("up".to_string(), executable(&["up", "-d"]));
        subcommands.insert("down".to_string(), executable(&["down"]));

        component(
            "node",
            ComponentKind::Command {
                command_name: None,
                format: executable(&["docker", "compose", "-f", "%etc(node/docker-compose.yml)"]),
                subcommands,
                aliases: BTreeMap::new(),
            },
        )
    }

    fn vm() -> Component {
        component(
            "vm",
            ComponentKind::Executable {
                installation_method: crate::manifest::InstallationMethod::Prebuilt,
                spec: ExecutableComponent {
                    installed_executable: "miden-vm".to_string(),
                    ..Default::default()
                },
            },
        )
    }

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    /// Spec section 13.3: `format ++ subcommand ++ user args`, in that order.
    ///
    /// Regression: the `format` prefix was dropped whenever a subcommand matched, so `miden node
    /// up` would have tried to execute `up` as a program.
    #[test]
    fn subcommand_expansion_follows_format_then_subcommand_then_user_args() {
        let env = Env::new();
        let compose_file = env.write("etc/node/docker-compose.yml");
        let node = node();

        let ComponentKind::Command { format, subcommands, .. } = node.kind() else {
            unreachable!()
        };

        let argv =
            compose(&node, format, subcommands.get("up"), args(&["--extra"]), &env.resolver())
                .expect("should compose");

        assert_eq!(
            argv,
            args(&[
                "docker",
                "compose",
                "-f",
                compose_file.to_str().unwrap(),
                "up",
                "-d",
                "--extra"
            ])
        );
    }

    #[test]
    fn a_component_without_subcommands_appends_all_user_args() {
        let env = Env::new();
        let vm = vm();

        let argv = compose(
            &vm,
            &Executable::default_call_format(),
            None,
            args(&["run", "-i", "x"]),
            &env.resolver(),
        )
        .expect("should compose");

        // The `opt/` shim, not `bin/`: `clap` derives its program name from `argv[0]`, and this is
        // what makes help read `miden vm ...` rather than `miden-vm ...`.
        assert_eq!(
            argv,
            args(&[env.sysroot.join("opt").join("miden vm").to_str().unwrap(), "run", "-i", "x"])
        );
    }

    /// A hidden component has no shim, so there is nothing to execute but the binary itself.
    #[test]
    fn a_hidden_component_resolves_to_its_installed_binary() {
        let env = Env::new();
        let hidden = component(
            "cargo-miden",
            ComponentKind::CargoExtension {
                installation_method: crate::manifest::InstallationMethod::Prebuilt,
                spec: ExecutableComponent {
                    installed_executable: "cargo-miden".to_string(),
                    hide: true,
                    ..Default::default()
                },
            },
        );

        let argv = Executable::default_call_format()
            .to_argv(&hidden, &env.resolver())
            .expect("should resolve");
        assert_eq!(argv, args(&[env.sysroot.join("bin").join("cargo-miden").to_str().unwrap()]));
    }

    /// `%lib` and `%etc` resolve *into* the publication; `%var` resolves outside it, and is created
    /// on demand because the component owns whatever ends up there.
    #[test]
    fn var_resolves_outside_the_publication_and_etc_inside_it() {
        let env = Env::new();
        let compose_file = env.write("etc/node/docker-compose.yml");
        let resolver = env.resolver();
        let node = node();

        assert_eq!(
            resolver.resolve(&Expr::VarPath(Some("data".into())), &node).unwrap(),
            OsString::from(env.home.join("var").join("0.15.0").join("data"))
        );
        assert_eq!(
            resolver
                .resolve(&Expr::EtcPath("node/docker-compose.yml".into()), &node)
                .unwrap(),
            OsString::from(compose_file)
        );
        assert!(
            env.home.join("var").join("0.15.0").is_dir(),
            "`%var` is created on demand: nothing else may touch it"
        );
    }

    /// An `%etc` path that is not in the publication names the component that asked for it.
    /// Passing it through and letting the component fail on it blames the wrong thing.
    #[test]
    fn a_missing_etc_path_is_an_error_naming_the_declaring_component() {
        let env = Env::new();
        let node = node();
        let ComponentKind::Command { format, .. } = node.kind() else {
            unreachable!()
        };

        let err = compose(&node, format, None, Vec::new(), &env.resolver())
            .expect_err("a missing asset must not be passed through");

        let message = err.to_string();
        assert!(message.contains("node"), "must name the component: {message}");
        assert!(message.contains("%etc"), "and the expression: {message}");
    }

    /// Spec section 13.5: a spawned component is told where it is running.
    ///
    /// `MIDEN_SYSROOT` is how a component finds its own toolchain without asking midenup, and
    /// `opt/` on `PATH` is how one component invokes another by its `miden <name>` spelling.
    #[test]
    fn a_spawned_component_is_given_its_toolchain_environment() {
        let temp = tempdir::TempDir::new("exec-env").unwrap();
        let home = temp.path().join("midenup");
        let channel = crate::manifest::Channel::new(CHANNEL, None, vec![]);
        let sysroot = crate::paths::toolchain_link(&home, &CHANNEL);
        std::fs::create_dir_all(sysroot.join("opt")).unwrap();

        // The component reports its environment by writing it down: `execute_command` gives the
        // child this process's stdout, so there is nothing to capture.
        let reported = temp.path().join("environment");
        let probe = temp.path().join("probe.sh");
        std::fs::write(
            &probe,
            format!(
                "#!/bin/sh\nprintf '%s\\n%s\\n%s\\n' \"$MIDEN_SYSROOT\" \"$MIDENUP_TOOLCHAIN\" \
                 \"$PATH\" > '{}'\n",
                reported.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let config = crate::config::Config::init(
            temp.path().to_path_buf(),
            home.clone(),
            temp.path().join("cargo"),
            "file:///nonexistent.json",
            true,
        )
        .unwrap();

        let mut child =
            config.execute_command(&channel, probe.as_os_str(), &[]).expect("should spawn");
        assert!(child.wait().unwrap().success());

        let reported = std::fs::read_to_string(&reported).expect("the probe must have run");
        let mut lines = reported.lines();

        assert_eq!(lines.next().unwrap(), sysroot.to_str().unwrap(), "MIDEN_SYSROOT");
        assert_eq!(lines.next().unwrap(), CHANNEL.to_string(), "MIDENUP_TOOLCHAIN");
        assert!(
            lines.next().unwrap().starts_with(sysroot.join("opt").to_str().unwrap()),
            "the toolchain's opt/ must come first on PATH"
        );
    }

    #[test]
    fn a_path_expression_cannot_be_the_program_itself() {
        let env = Env::new();
        assert!(matches!(
            executable(&["%lib"]).to_argv(&vm(), &env.resolver()),
            Err(InvalidExecutable::NotExecutable)
        ));
    }
}
