mod runner;
mod scenarios;
mod selection;

use std::{path::PathBuf, process::ExitCode};

use anyhow::Context;
use clap::Parser;
use midenup::manifest::{VersionedManifest, validate};

use crate::runner::{Home, Outcome};

/// Runs the acceptance scenarios for the toolchains a manifest change affects.
#[derive(Debug, Parser)]
#[command(name = "acceptance")]
struct Cli {
    /// The candidate manifest
    #[arg(long, value_name = "PATH", default_value = "manifest/v3/manifest.json")]
    manifest_path: PathBuf,
    /// The manifest this one replaces; only toolchains changed since then are verified, otherwise
    /// every network's toolchain is
    #[arg(long, value_name = "URI")]
    against: Option<String>,
    /// The scenario file; lit suites live in directories next to it
    #[arg(long, value_name = "PATH", default_value = "acceptance/scenarios.toml")]
    scenarios: PathBuf,
    /// The `midenup` binary that installs the toolchains, put on the scenarios' `PATH`
    #[arg(long, value_name = "PATH", required(true))]
    midenup: PathBuf,
}

fn main() -> ExitCode {
    match run(&Cli::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        },
    }
}

/// Prints the report and returns whether the change is accepted.
fn run(cli: &Cli) -> anyhow::Result<bool> {
    let manifest_path = std::fs::canonicalize(&cli.manifest_path)
        .with_context(|| format!("failed to find {}", cli.manifest_path.display()))?;
    let manifest = VersionedManifest::load_from_file(&manifest_path)?;
    let previous = cli
        .against
        .as_ref()
        .map(|uri| {
            VersionedManifest::load_from(uri)
                .with_context(|| format!("failed to load the manifest at '{uri}'"))
        })
        .transpose()?;
    let scenarios = scenarios::load(&cli.scenarios)?;
    let mut accepted = true;

    println!("Verifying {}..\n", cli.manifest_path.display());
    println!("Pre-requisites:\n");

    match validate::validate_manifest(&manifest) {
        Ok(()) => println!("* Manifest is valid .. PASS"),
        Err(errors) => {
            println!("* Manifest is valid .. FAIL");
            for error in errors {
                println!("    {error}");
            }
            println!("\nThe change should be REJECTED: the manifest is invalid");
            return Ok(false);
        },
    }

    let targets = selection::targets(&manifest, previous.as_ref());
    let mut gaps = Vec::new();
    for target in &targets {
        let uncovered = selection::uncovered(target, &scenarios);
        if !uncovered.is_empty() {
            gaps.push((target, uncovered));
        }
    }
    if gaps.is_empty() {
        println!("* All toolchain components have acceptance tests .. PASS");
    } else {
        accepted = false;
        println!("* All toolchain components have acceptance tests .. FAIL");
        for (target, uncovered) in &gaps {
            println!("    {}: no scenario for {}", target.toolchain.name, uncovered.join(", "));
        }
    }

    println!("\nAffected toolchains (and components):\n");
    if targets.is_empty() {
        println!("* none");
    }
    for target in &targets {
        let changed: Vec<&str> = target.changed.iter().map(String::as_str).collect();
        println!("* {} ({})", target.toolchain.name, changed.join(", "));
    }

    let manifest_uri = format!("file://{}", manifest_path.display());
    for target in &targets {
        let selected = selection::selected(target, &scenarios);
        if selected.is_empty() {
            continue;
        }
        println!(
            "\nRunning matching acceptance tests for toolchain {}..\n",
            target.toolchain.name
        );

        let home = match Home::install(&cli.midenup, &manifest_uri, target.toolchain) {
            Ok(home) => home,
            Err(err) => {
                println!("* Install toolchain {} .. FAIL\n\n{err:#}\n", target.toolchain.name);
                accepted = false;
                continue;
            },
        };
        for scenario in selected {
            match home.run(scenario, &cli.scenarios)? {
                Outcome::Pass => println!("* {} .. PASS", scenario.title),
                Outcome::Fail(output) => {
                    println!("* {} .. FAIL\n\n{output}\n", scenario.title);
                    accepted = false;
                },
                Outcome::Skipped(reason) => println!("* {} .. SKIPPED ({reason})", scenario.title),
            }
        }
    }

    let verdict = if accepted { "ACCEPTED" } else { "REJECTED" };
    println!("\nThe change should be {verdict}");
    Ok(accepted)
}
