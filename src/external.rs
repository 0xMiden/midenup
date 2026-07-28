// This module holds functions which are used in the cargo install script, after being imported
// via include_str.
//
// Since these functions are intended to be used in the install script, they should _NOT_ import
// utilities from any crate besides the standard library and they should also prioritize qualifying
// over importing, in order to avoid duplicate `use` declarations.

#[allow(dead_code)]
pub fn install_artifact(
    uri: &str,
    to: impl AsRef<std::path::Path>,
    target_is_file: bool,
) -> Result<(), String> {
    use std::{io::Write, path::Path};

    let to = to.as_ref();

    if target_is_file {
        let to_dir = to.parent().unwrap();
        std::fs::create_dir_all(to_dir).map_err(|error| {
            format!("failed to create parent directory for artifact '{}': {error}", to.display())
        })?;
    } else {
        std::fs::create_dir_all(to).map_err(|error| {
            format!("failed to create target directory for artifact '{}': {error}", to.display())
        })?;
    }

    if let Some(binary_path) = uri.strip_prefix("file://") {
        let target_path = if target_is_file {
            to.to_path_buf()
        } else {
            let binary_path = Path::new(binary_path);
            let file_name = binary_path
                .file_name()
                .map(Path::new)
                .expect("expected source path to have file name");
            to.join(file_name)
        };
        std::fs::copy(binary_path, &target_path).map_err(|err| {
            format!("failed to copy {binary_path} -> {}: {err}", target_path.display())
        })?;
    } else if uri.starts_with("https://") {
        let mut data = Vec::new();
        let mut handle = curl::easy::Easy::new();
        {
            handle.follow_location(true).map_err(|_| String::from("failed to setup curl"))?;
            handle.max_redirections(10).map_err(|_| String::from("failed to setup curl"))?;
            handle.url(uri).map_err(|error| {
                format!("invalid artifact uri '{uri}': {}", error.description())
            })?;

            let mut transfer = handle.transfer();
            transfer
                .write_function(|new_data| {
                    data.extend_from_slice(new_data);
                    Ok(new_data.len())
                })
                .unwrap();
            transfer
                .perform()
                .map_err(|error| format!("transfer failed for '{uri}': {error}"))?
        }

        // *After* the transfer. Read beforehand, curl has no response yet and always reports 0,
        // so the check never fired and the body of a 404 was written out as the artifact.
        let response_code = handle
            .response_code()
            .map_err(|err| format!("request failed for '{uri}' with unknown status: {err}"))?;
        if !(200..300).contains(&response_code) {
            return Err(format!("request failed for '{uri}' with status {response_code}"));
        }
        if data.is_empty() {
            return Err(format!("invalid artifact: content downloaded from '{uri}' is empty"));
        }

        let to = if target_is_file {
            to.to_path_buf()
        } else {
            // `rsplit_once` yields (before, after); binding the first element took the URL
            // *prefix* rather than the final path segment.
            let (_, file_name) =
                uri.rsplit_once('/').expect("expected source uri to have a file component");
            to.join(file_name)
        };
        // Unique, so two artifacts sharing a stem cannot stage through the same path.
        let tmp = to.with_extension(format!("{}.part", std::process::id()));
        let mut file = std::fs::File::create(&tmp).map_err(|error| {
            format!("failed to create temporary file '{}' for artifact: {error}", to.display())
        })?;
        // We set the same flags that cargo uses when producing an executable.
        file.set_permissions(
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        )
        .map_err(|error| format!("failed to set permissions on '{}': {error}", to.display()))?;
        file.write_all(&data)
            .map_err(|error| format!("failed to write artifact to '{}': {error}", to.display()))?;
        std::fs::rename(&tmp, &to).map_err(|error| {
            format!("failed to rename {} -> {}: {error}", tmp.display(), to.display())
        })?;
    } else {
        return Err(format!("unsupported uri scheme for '{uri}', must be one of: 'https', 'file'"));
    }

    Ok(())
}

#[allow(dead_code)]
pub fn install_from_source(
    toolchain_flag: &str,
    chosen_profile: &[&str],
    verbosity_flag: &str,
    args: &[&str],
    root_directory: impl AsRef<std::path::Path>,
) -> Result<(), String> {
    let root_directory = root_directory.as_ref();
    let mut command = std::process::Command::new("cargo");
    if !toolchain_flag.is_empty() {
        command.arg(toolchain_flag);
    }
    command
                .arg("install")
                .arg("--locked")
                .args(chosen_profile)
                .arg(verbosity_flag)
                .args(args)
                // Force the install target directory to be $MIDEN_SYSROOT/bin
                .arg("--root")
                .arg(root_directory)
                // Spawn command
                .stderr(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit());
    let argv = command.get_args().map(|arg| arg.display().to_string()).collect::<Vec<_>>();
    let mut child = command.spawn().map_err(|error| error.to_string())?;

    // Await results
    let status = child
        .wait()
        .map_err(|error| format!("failed to execute `cargo {}`: {error}", argv.join(" ")))?;

    if !status.success() {
        return Err(format!("command `cargo {}` exited with non-zero status", argv.join(" ")));
    }

    Ok(())
}
