//! Runs the acceptance scenarios for the toolchains a manifest change affects.

mod runner;
mod scenarios;
mod selection;

use std::path::Path;

use anyhow::Context;
use midenup::manifest::{VersionedManifest, validate};

use self::runner::{Home, Outcome};

/// Prints the report and returns whether the change is accepted.
pub fn run(
    manifest_path: &Path,
    against: Option<&str>,
    scenarios_path: &Path,
    midenup: &Path,
) -> anyhow::Result<bool> {
    let manifest_path = std::fs::canonicalize(manifest_path)
        .with_context(|| format!("failed to find {}", manifest_path.display()))?;
    let manifest = VersionedManifest::load_from_file(&manifest_path)?;
    let previous = against
        .map(|uri| {
            VersionedManifest::load_from(uri)
                .with_context(|| format!("failed to load the manifest at '{uri}'"))
        })
        .transpose()?;
    let scenarios = scenarios::load(scenarios_path)?;
    let mut accepted = true;

    println!("Verifying {}..\n", manifest_path.display());
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

        let home = match Home::install(midenup, &manifest_uri, target.toolchain) {
            Ok(home) => home,
            Err(err) => {
                println!("* Install toolchain {} .. FAIL\n\n{err:#}\n", target.toolchain.name);
                accepted = false;
                continue;
            },
        };
        for scenario in selected {
            match home.run(scenario, scenarios_path)? {
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
