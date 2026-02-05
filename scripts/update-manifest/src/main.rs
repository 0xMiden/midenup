use midenup_lib::manifest::ManifestError;
use std::env;
use std::io::Write;

fn main() -> Result<(), ManifestError> {
    let args: Vec<String> = env::args().collect();

    if args.len() != 2 {
        panic!("no manifest file path provided.\nUsage: update-manifest <manifest-file-uri>");
    }

    let uri = &args[1];

    let mut manifest =
        midenup_lib::manifest::Manifest::load_from(&uri).unwrap_or_else(|e| panic!("{}", e));

    let update_result = manifest.update()?;

    let mut updated_manifest_file = std::fs::File::create("manifest/channel-manifest.json")
        .expect("Failed to create new manifest file");

    updated_manifest_file
        .write_all(
            serde_json::to_string_pretty(&manifest)
                .expect("Failed to serialize manifest")
                .as_bytes(),
        )
        .unwrap_or_else(|e| panic!("{}", e));

    {
        let changed_packages = update_result.changed_packages;
        // Print the name of the branch that's going to be used.
        let branch_suffix = changed_packages.into_iter().collect::<Vec<_>>().join("+");
        std::println!("{}", branch_suffix);
    }

    Ok(())
}
