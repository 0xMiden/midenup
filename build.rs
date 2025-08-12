use std::process::Command;

fn main() {
    // We delete the old build/ directory in order to avoid mixing files up.
    if std::path::PathBuf::from("build").exists() {
        std::fs::remove_dir_all("build")
            .unwrap_or_else(|err| panic!("Failed to delete build/ directory: {err}"));
    }

    // We'll place all the generated files in this build/ directory
    std::fs::create_dir("build")
        .unwrap_or_else(|err| panic!("Failed to create build/ directory: {err}"));

    write_command_to_file(&["cargo", "--version"], "build/cargo_version.in");
    write_command_to_file(&["git", "rev-parse", "--verify", "HEAD"], "build/git_revision.in");
}

fn write_command_to_file(command: &[&str], file: &str) {
    let full_command =
        command.iter().fold(String::new(), |acc, argument| format!("{acc} {argument}"));

    let output = {
        let command =
            Command::new(command.first().expect("command must have at leaste one element"))
                .args(command.iter().skip(1))
                .output()
                .unwrap_or_else(|err| {
                    panic!("Couldn't run {full_command} because of: {err}");
                })
                .stdout;

        String::from_utf8(command).unwrap_or_else(|err| {
            panic!("failed to parse {full_command} output as string, because of {err}")
        })
    };

    std::fs::write(file, output.trim())
        .unwrap_or_else(|err| panic!("Failed to write to {file}: {err}"));
}
