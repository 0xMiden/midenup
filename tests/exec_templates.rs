use std::{borrow::Cow, ffi::OsString};

use midenup::{
    channel::UserChannel,
    exec::{Executable, Expr, InvalidExecutable, Resolver},
    manifest::{Component, ComponentKind},
    version::Authority,
};

fn component() -> Component {
    Component {
        name: Cow::Borrowed("node"),
        version: Authority::Registry { version: semver::Version::new(0, 1, 0) },
        kind: ComponentKind::Command {
            command_name: None,
            format: Executable::default(),
            subcommands: Default::default(),
            aliases: Default::default(),
        },
        profiles: vec![],
        requires: vec![],
        artifacts: Default::default(),
        extra: Default::default(),
    }
}

fn executable(argument: &str) -> Executable {
    Executable::try_from(vec!["probe".to_string(), argument.to_string()]).unwrap()
}

fn resolver(home: &std::path::Path) -> Resolver {
    Resolver::new(
        home.join("publication"),
        home,
        &UserChannel::Version(semver::Version::new(0, 15, 0)),
    )
}

fn assert_round_trip(argument: &str) {
    let env = tempdir::TempDir::new("exec-template").unwrap();
    let resolver = resolver(env.path());
    let original = executable(argument);
    let json = serde_json::to_string(&original).unwrap();
    let reparsed: Executable = serde_json::from_str(&json)
        .unwrap_or_else(|err| panic!("cannot read serialized {argument:?}: {json}: {err}"));
    assert_eq!(
        reparsed.to_argv(&component(), &resolver).unwrap(),
        original.to_argv(&component(), &resolver).unwrap(),
        "serialization must preserve argument semantics: {argument:?} -> {json}"
    );
}

#[test]
fn serialization_preserves_escaped_expression() {
    assert_round_trip("%%version");
}

#[test]
fn serialization_preserves_literal_percent() {
    assert_round_trip("100%%");
}

#[test]
fn serialization_preserves_plain_arguments() {
    let command = executable("--help");
    assert_eq!(serde_json::to_value(&command).unwrap(), serde_json::json!(["probe", "--help"]));
}

#[test]
fn serialization_preserves_path_argument_delimiters() {
    for path in [
        "data)cache",
        "data%cache",
        "data%%cache",
        "data%)cache",
        "%version",
        "data(cache)",
    ] {
        for original in [
            Expr::LibPath(Some(path.to_string())),
            Expr::VarPath(Some(path.to_string())),
            Expr::EtcPath(path.to_string()),
        ] {
            let serialized = original.to_string();
            let reparsed = serialized.parse::<Expr>().unwrap();
            assert_eq!(reparsed, original, "path content changed: {serialized:?}");
        }
    }
}

#[test]
fn serialization_preserves_standalone_braced_expression() {
    let env = tempdir::TempDir::new("exec-template").unwrap();
    let resolver = resolver(env.path());
    let original = "%{version}".parse::<Expr>().unwrap();
    let serialized = original.to_string();
    let reparsed = serialized.parse::<Expr>().unwrap();
    assert_eq!(
        resolver.resolve(&reparsed, &component()).unwrap(),
        resolver.resolve(&original, &component()).unwrap(),
        "standalone braced expression changed meaning after serialization: {serialized:?}"
    );
}

#[test]
fn serialization_preserves_adjacent_expression_fragments() {
    let env = tempdir::TempDir::new("exec-template").unwrap();
    let resolver = resolver(env.path());
    let original = "%version%version".parse::<Expr>().unwrap();
    let serialized = original.to_string();
    let reparsed = serialized.parse::<Expr>().unwrap();
    assert_eq!(
        resolver.resolve(&reparsed, &component()).unwrap(),
        resolver.resolve(&original, &component()).unwrap(),
        "adjacent expressions changed meaning after serialization: {serialized:?}"
    );
}

#[test]
fn braces_do_not_allow_directory_expression_as_program() {
    let env = tempdir::TempDir::new("exec-template").unwrap();
    for input in ["%lib", "%{lib}", "%var", "%{var}"] {
        let command = Executable::try_from(vec![input.to_string()]).unwrap();
        let result = command.to_argv(&component(), &resolver(env.path()));
        assert!(
            matches!(result, Err(InvalidExecutable::NotExecutable)),
            "directory expression {input:?} was accepted as the program: {result:?}"
        );
    }
}

#[test]
fn serialization_preserves_braced_word_boundary() {
    assert_round_trip("v%{version}ed");
}

#[test]
fn serialization_preserves_braced_path_followed_by_parentheses() {
    // The parentheses here are literal text, not an argument to `%lib`.
    assert_round_trip("%{lib}(suffix)");
}

#[test]
fn expression_keywords_require_a_word_boundary() {
    for input in ["%version2", "%version_suffix", "%versioné"] {
        assert!(matches!(
            input.parse::<Expr>(),
            Err(InvalidExecutable::UnknownFragmentKind { .. })
        ));
    }
}

#[test]
fn empty_argument_is_preserved() {
    let env = tempdir::TempDir::new("exec-template").unwrap();
    let argv = executable("").to_argv(&component(), &resolver(env.path())).unwrap();
    assert_eq!(argv, [OsString::from("probe"), OsString::new()]);
}

#[test]
fn serialization_preserves_empty_argument() {
    assert_round_trip("");
}

#[cfg(unix)]
#[test]
fn template_paths_preserve_non_unicode_bytes() {
    use std::os::unix::ffi::OsStringExt;

    let env = tempdir::TempDir::new("exec-template").unwrap();
    let sysroot = env.path().join(OsString::from_vec(b"publication-\xff".to_vec()));
    let resolver =
        Resolver::new(&sysroot, env.path(), &UserChannel::Version(semver::Version::new(0, 15, 0)));
    let argv = executable("LIB=%lib,VERSION=%version")
        .to_argv(&component(), &resolver)
        .unwrap();
    let mut expected = OsString::from("LIB=");
    expected.push(sysroot.join("lib"));
    expected.push(",VERSION=0.1.0");
    assert_eq!(argv[1], expected);
}

#[test]
fn mixed_fragments_resolve_into_one_argument() {
    let env = tempdir::TempDir::new("exec-template").unwrap();
    let etc = env.path().join("publication/etc/node/config");
    std::fs::create_dir_all(etc.parent().unwrap()).unwrap();
    std::fs::write(&etc, b"config").unwrap();
    let argv = executable("v%version,config=%etc(node/config),data=%var(data),%%version")
        .to_argv(&component(), &resolver(env.path()))
        .unwrap();
    let mut expected = OsString::from("v0.1.0,config=");
    expected.push(etc);
    expected.push(",data=");
    expected.push(env.path().join("var/0.15.0/data"));
    expected.push(",%version");
    assert_eq!(argv, [OsString::from("probe"), expected]);
}

#[test]
fn braced_keywords_allow_identifier_suffixes() {
    let env = tempdir::TempDir::new("exec-template").unwrap();
    let argv = executable("%{version}2/%{version}_suffix/%{version}é")
        .to_argv(&component(), &resolver(env.path()))
        .unwrap();
    assert_eq!(argv[1], OsString::from("0.1.02/0.1.0_suffix/0.1.0é"));
}

#[test]
fn adjacent_fragments_and_escaped_percent_resolve_once() {
    let env = tempdir::TempDir::new("exec-template").unwrap();
    let argv = executable("%version%version/%%%version/%%%%version")
        .to_argv(&component(), &resolver(env.path()))
        .unwrap();
    assert_eq!(argv[1], OsString::from("0.1.00.1.0/%0.1.0/%%version"));
}

#[test]
fn malformed_fragments_report_parse_errors() {
    assert!(matches!(
        "%{version".parse::<Expr>(),
        Err(InvalidExecutable::UnclosedBrace { .. })
    ));
    assert!(matches!(
        "%etc(config".parse::<Expr>(),
        Err(InvalidExecutable::UnclosedParen { .. })
    ));
    assert!(matches!("%etc".parse::<Expr>(), Err(InvalidExecutable::MissingEtcPath)));
    assert!(matches!(
        "%unknown".parse::<Expr>(),
        Err(InvalidExecutable::UnknownFragmentKind { .. })
    ));
    assert!(matches!(
        "%".parse::<Expr>(),
        Err(InvalidExecutable::ExpectedExprFragment { .. })
    ));
}
