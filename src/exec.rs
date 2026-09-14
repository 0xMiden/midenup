use std::{ffi::OsString, fmt, path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::{manifest::Component, version::Authority};

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
            out.push(expr.to_string());
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
    /// A template string, permitting a single argument to be built out of one or more expression
    /// fragments, e.g. `"VER=%version,BIN=%installed-executable"`.
    ///
    /// Expression keywords contain Unicode letters and digits or underscores. An unknown keyword,
    /// such as `VER=%versioned`, is an error rather than a partial `%version` expansion.
    /// However, `VER=%version,` would be recognized as containing `%version`, as it is unambiguous.
    ///
    /// If you require an expression fragment in a place where it would otherwise be ambiguous, you
    /// may use the syntax `%{..}` to disambiguate, e.g. `VER=%{version}ed`.
    ///
    /// Lastly, if you have a string that may contain what appears to be a valid expression fragment
    /// that you _don't_ want expanded, you should use `%%` to escape the fragment keyword, e.g.
    /// `%%version` would emit the string `%version`.
    Template(Box<[Expr]>),
    /// Resolves to the registry version of the current component.
    ///
    /// If the current component is not versioned via registry, use of this expression is invalid.
    Version,
    /// An argument that is passed verbatim, as is.
    Verbatim(String),
}

impl Expr {
    fn display(&self, braced: bool) -> DisplayExpr<'_> {
        DisplayExpr { expr: self, braced }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display(false))
    }
}

struct DisplayExpr<'a> {
    expr: &'a Expr,
    braced: bool,
}

impl fmt::Display for DisplayExpr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use core::fmt::Write;

        fn write_escaped(f: &mut fmt::Formatter<'_>, text: &str, path: bool) -> fmt::Result {
            use core::fmt::Write;

            for c in text.chars() {
                if c == '%' || (path && c == ')') {
                    f.write_char('%')?;
                }
                f.write_char(c)?;
            }
            Ok(())
        }

        fn write_path(f: &mut fmt::Formatter<'_>, keyword: &str, path: &str) -> fmt::Result {
            write!(f, "{keyword}(")?;
            write_escaped(f, path, true)?;
            f.write_str(")")
        }

        match self.expr {
            Expr::Verbatim(arg) => {
                return write_escaped(f, arg, false);
            },
            Expr::Template(fragments) => {
                for fragment in fragments {
                    // Braces keep substitutions separate from neighboring literal text, including
                    // identifier suffixes and parentheses that are not expression arguments.
                    write!(f, "{}", fragment.display(true))?;
                }
                return Ok(());
            },
            _ => {},
        }

        if self.braced {
            f.write_str("%{")?;
        } else {
            f.write_char('%')?;
        }
        match self.expr {
            Expr::Executable => f.write_str("installed-executable"),
            Expr::LibPath(None) => f.write_str("lib"),
            Expr::LibPath(Some(name)) => write_path(f, "lib", name),
            Expr::VarPath(None) => f.write_str("var"),
            Expr::VarPath(Some(name)) => write_path(f, "var", name),
            Expr::EtcPath(name) => write_path(f, "etc", name),
            Expr::Version => f.write_str("version"),
            Expr::Template(_) | Expr::Verbatim(_) => unreachable!(),
        }?;

        if self.braced { f.write_char('}') } else { Ok(()) }
    }
}

impl FromStr for Expr {
    type Err = InvalidExecutable;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        fn parse_delimited(
            chars: &mut core::iter::Peekable<core::str::CharIndices>,
            open: char,
            close: char,
        ) -> Result<Option<String>, usize> {
            let Some((start, _)) = chars.next_if(|(_, c)| *c == open) else {
                return Ok(None);
            };

            let mut buf = String::with_capacity(64);

            while let Some((_, c)) = chars.next() {
                if c == close {
                    return Ok(Some(buf));
                }
                if c == '%'
                    && let Some((_, escaped)) = chars.next_if(|(_, c)| *c == '%' || *c == close)
                {
                    buf.push(escaped);
                    continue;
                }
                buf.push(c);
            }

            Err(start)
        }

        fn parse_word(chars: &mut core::iter::Peekable<core::str::CharIndices>) -> Option<String> {
            let mut buf = String::with_capacity(64);

            while let Some((_, c)) = chars.peek() {
                match *c {
                    c if c.is_alphanumeric() || c == '_' => {
                        chars.next();
                        buf.push(c);
                    },
                    '-' if matches!(buf.as_str(), "installed") => {
                        chars.next();
                        buf.push('-');
                    },
                    _other => break,
                }
            }

            if buf.is_empty() { None } else { Some(buf) }
        }

        fn parse_fragment(
            input: &str,
            chars: &mut core::iter::Peekable<core::str::CharIndices>,
        ) -> Result<Option<Expr>, InvalidExecutable> {
            let braced = chars.next_if(|(_, c)| *c == '{').map(|(i, _)| i);

            let Some(word) = parse_word(chars) else {
                return Ok(None);
            };

            let expr = match word.as_str() {
                "installed-executable" => Expr::Executable,
                "version" => Expr::Version,
                "lib" => match parse_delimited(chars, '(', ')') {
                    Ok(arg) => Expr::LibPath(arg),
                    Err(start) => {
                        return Err(InvalidExecutable::UnclosedParen {
                            input: input.to_string(),
                            start,
                        });
                    },
                },
                "var" => match parse_delimited(chars, '(', ')') {
                    Ok(arg) => Expr::VarPath(arg),
                    Err(start) => {
                        return Err(InvalidExecutable::UnclosedParen {
                            input: input.to_string(),
                            start,
                        });
                    },
                },
                "etc" => match parse_delimited(chars, '(', ')') {
                    Ok(Some(arg)) => Expr::EtcPath(arg),
                    Ok(None) => return Err(InvalidExecutable::MissingEtcPath),
                    Err(start) => {
                        return Err(InvalidExecutable::UnclosedParen {
                            input: input.to_string(),
                            start,
                        });
                    },
                },
                other => {
                    return Err(InvalidExecutable::UnknownFragmentKind {
                        input: input.to_string(),
                        fragment: other.to_string(),
                    });
                },
            };

            if let Some(brace_start) = braced {
                // We expect a trailing '}' if we've reached this point
                if chars.next_if(|(_, c)| *c == '}').is_none() {
                    return Err(InvalidExecutable::UnclosedBrace {
                        input: input.to_string(),
                        start: brace_start,
                    });
                }
            }
            Ok(Some(expr))
        }

        let mut fragments = Vec::new();
        let mut chars = value.char_indices().peekable();
        let mut buf = String::new();

        while let Some((i, c)) = chars.next() {
            match c {
                '%' => {
                    if chars.next_if(|(_, c)| *c == '%').is_some() {
                        // The fragment was escaped, so expand as verbatim
                        buf.push('%');
                        continue;
                    }

                    match parse_fragment(value, &mut chars)? {
                        Some(fragment) => {
                            if !buf.is_empty() {
                                fragments.push(Expr::Verbatim(core::mem::take(&mut buf)));
                            }
                            fragments.push(fragment);
                        },
                        None => {
                            return Err(InvalidExecutable::ExpectedExprFragment {
                                input: value.to_string(),
                                start: i,
                            });
                        },
                    }
                },
                c => {
                    buf.push(c);
                },
            }
        }

        match fragments.len() {
            0 => Ok(Expr::Verbatim(buf)),
            1 if buf.is_empty() => Ok(fragments.pop().unwrap()),
            _ => {
                if !buf.is_empty() {
                    fragments.push(Expr::Verbatim(buf));
                }
                Ok(Expr::Template(fragments.into_boxed_slice()))
            },
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidExecutable {
    #[error("invalid executable: empty executable expression")]
    Empty,
    #[error("invalid executable: unclosed parentheses opened at column {start} in expr '{input}'")]
    UnclosedParen { input: String, start: usize },
    #[error("invalid executable: unclosed brace opened at column {start} in expr '{input}'")]
    UnclosedBrace { input: String, start: usize },
    #[error("invalid executable: unrecognized fragment kind '{fragment}' in expr '{input}'")]
    UnknownFragmentKind { input: String, fragment: String },
    #[error(
        "invalid executable: expected expression fragment at column {start} in template '{input}'"
    )]
    ExpectedExprFragment { input: String, start: usize },
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
    #[error(
        "component '{component}' refers to `%version` in '{expression}', but it has no registry \
         version"
    )]
    VersionUnavailable { component: String, expression: String },
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
    /// `$MIDENUP_HOME/var/<selector>`: mutable state, deliberately *outside* the publication, so
    /// it survives every republication of the toolchain (spec section 3.2).
    var: PathBuf,
}

impl Resolver {
    /// `selector` is what the user chose -- a network name or a pinned version -- not the channel
    /// it resolves to. Two networks sharing a channel must still reach their own `%var`, and a
    /// network's `%var` must not change when its pointer moves. See [`crate::paths::var_dir`].
    pub fn new(
        sysroot: impl Into<PathBuf>,
        home: &std::path::Path,
        selector: &crate::channel::UserChannel,
    ) -> Self {
        Self {
            sysroot: sysroot.into(),
            var: crate::paths::var_dir(home, selector),
        }
    }

    /// Resolves one expression on behalf of `component`.
    pub fn resolve(
        &self,
        expr: &Expr,
        component: &Component,
    ) -> Result<OsString, InvalidExecutable> {
        self.resolve_inner(expr, component).map_err(|err| match err {
            InvalidExecutable::VersionUnavailable { component, .. } => {
                InvalidExecutable::VersionUnavailable { component, expression: expr.to_string() }
            },
            err => err,
        })
    }

    fn resolve_inner(
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
            // state the component owns and creates -- e.g. `%var(data)` could be the client's
            // database directory, which does not exist until the client makes it, so requiring it
            // to exist would fail on every fresh installation.
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
            Expr::Template(fragments) => {
                let mut buf = OsString::new();
                for fragment in fragments {
                    buf.push(self.resolve_inner(fragment, component)?);
                }
                Ok(buf)
            },
            Expr::Version => {
                let Authority::Registry { version } = &component.version else {
                    return Err(InvalidExecutable::VersionUnavailable {
                        component: component.name.to_string(),
                        expression: expr.to_string(),
                    });
                };
                Ok(version.to_string().into())
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
/// The `format` prefix is preserved when a subcommand matches, rather than replaced by it: a
/// component that declares both -- `format: ["docker", "compose", "-f", "%etc(...)"]` plus
/// `subcommands: {up: ["up", "-d"]}` -- means "run docker compose with `up -d`", and dropping the
/// prefix would execute `up -d` as though it were a program.
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
            Resolver::new(
                self.sysroot.clone(),
                &self.home,
                &crate::channel::UserChannel::Version(CHANNEL),
            )
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
    /// The matching subcommand extends the `format` prefix rather than replacing it: without the
    /// prefix, `miden node up` would try to execute `up` as a program.
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

    /// `%version` inside a verbatim word resolves to the registry version of the component, so a
    /// manifest can name a release-specific value such as an image tag without hard-coding it.
    #[test]
    fn version_substitution_inside_a_verbatim_word_uses_the_component_registry_version() {
        let env = Env::new();
        let node = node();

        let argv = executable(&["env", "MIDEN_NODE_IMAGE=ghcr.io/0xmiden/miden-node:v%version"])
            .to_argv(&node, &env.resolver())
            .expect("should resolve");

        assert_eq!(argv, args(&["env", "MIDEN_NODE_IMAGE=ghcr.io/0xmiden/miden-node:v0.1.0"]));
    }

    /// A component without a registry version has nothing for `%version` to resolve to, and the
    /// error names the component and the offending word rather than passing `%version` through.
    #[test]
    fn version_substitution_without_a_registry_version_is_an_error() {
        let env = Env::new();
        let mut node = node();
        node.version = crate::version::Authority::Path {
            path: PathBuf::from("/some/checkout"),
            last_modification: None,
        };

        let err = executable(&["env", "TAG=v%version"])
            .to_argv(&node, &env.resolver())
            .expect_err("should fail");

        let message = err.to_string();
        assert!(message.contains("node"), "names the component: {message}");
        assert!(message.contains("TAG=v%{version}"), "names the canonical word: {message}");
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

    /// Two networks routinely name one channel -- all three do in the shipped manifest -- and their
    /// state must stay apart regardless. `%var` keys on what the user selected, so a user working
    /// on mainnet and on testnet has two client databases, not one holding both.
    #[test]
    fn two_networks_naming_one_channel_do_not_share_var() {
        let env = Env::new();
        let node = node();
        let var_of = |name: &'static str| {
            let resolver = Resolver::new(
                env.sysroot.clone(),
                &env.home,
                &crate::channel::UserChannel::Named(std::borrow::Cow::Borrowed(name)),
            );
            resolver.resolve(&Expr::VarPath(Some("data".into())), &node).unwrap()
        };

        assert_eq!(var_of("mainnet"), OsString::from(env.home.join("var/mainnet/data")));
        assert_eq!(var_of("testnet"), OsString::from(env.home.join("var/testnet/data")));
        assert_ne!(
            var_of("mainnet"),
            var_of("testnet"),
            "the sysroot is identical here: only the selector keeps the stores apart"
        );
    }

    /// The other half of the same rule: a pinned selector keys on the version the user pinned.
    #[test]
    fn a_pinned_selector_keys_var_on_its_version() {
        let env = Env::new();
        assert_eq!(
            env.resolver().resolve(&Expr::VarPath(Some("data".into())), &node()).unwrap(),
            OsString::from(env.home.join("var/0.15.0/data"))
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
        let channel = crate::manifest::Channel::new(CHANNEL, vec![]);
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

        let exit_status = config
            .execute_command(&channel, probe.as_os_str(), &[])
            .expect("should execute");
        assert!(exit_status.success());

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

    #[test]
    fn legacy_verbatim_arguments_keep_literal_percent_syntax() {
        use crate::manifest::v1::CliCommand;

        let env = Env::new();
        let words = ["printf", "%s", "%lib", "100%%", "%version"];
        // Legacy substitutions are separate enum variants; these are all literal words.
        let command = Executable::try_from(
            words
                .into_iter()
                .map(|word| CliCommand::Verbatim(word.to_string()))
                .collect::<Vec<_>>(),
        )
        .expect("legacy verbatim arguments must remain valid");
        assert_eq!(command.to_argv(&node(), &env.resolver()).unwrap(), args(&words));
        let json = serde_json::to_string(&command).unwrap();
        let reparsed: Executable = serde_json::from_str(&json).unwrap();
        assert_eq!(reparsed.to_argv(&node(), &env.resolver()).unwrap(), args(&words));
    }
}
