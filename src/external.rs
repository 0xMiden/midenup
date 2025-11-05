// This file holds functions which are used in the cargo install script, after
// being imported via include_str.
//
// Since these functions are intended to be used in the install script,
// they should *not* import utilities from any crate besides the standard library.

pub const HTTP_ERROR_CODES: std::ops::Range<u32> = 400..500;

#[allow(dead_code)]
pub fn install_artifact(uri: &str, to: &std::path::Path) -> Result<(), String> {
    if uri.is_empty() {
        return Err("No artifact found".into());
    }
    if let Some(binary_path) = uri.strip_prefix("file://") {
        std::fs::copy(binary_path, to).map_err(|err| {
            format!("Failed to copy binary file to {} because of {}", to.display(), err)
        })?;
    } else if uri.starts_with("https://") {
        let mut data = Vec::new();
        let mut handle = curl::easy::Easy::new();
        handle
            .follow_location(true)
            .map_err(|_| String::from("Failed to set curl up"))?;
        handle.url(uri).map_err(|error| {
            format!("Error while trying to fetch binary: {}", error.description())
        })?;
        {
            let response_code = handle.response_code().map_err(|_| {
                String::from("Failed to get response code from webpage; despite HTTP protocol supporting it.")
            })?;
            if HTTP_ERROR_CODES.contains(&response_code) {
                return Err(format!("Webpage returned error. Does {} exist?", uri));
            }

            let mut transfer = handle.transfer();
            transfer
                .write_function(|new_data| {
                    data.extend_from_slice(new_data);
                    Ok(new_data.len())
                })
                .unwrap();
            transfer.perform().map_err(|error| {
                format!("Error while trying to fetch binary: {}", error.description())
            })?
        }
        if data.is_empty() {
            return Err(format!("Found webpage {} to be empty.", uri));
        }
        let mut file = std::fs::File::create(to).map_err(|error| {
            format!("Failed to create download file in {} because of {}", to.display(), error)
        })?;
        // We set the same flags that cargo uses when producing an executable.
        file.set_permissions(
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        )
        .map_err(|error| {
            format!("Failed to set permissions in {} because of {}", to.display(), error)
        })?;
        std::io::Write::write_all(&mut file, &data).map_err(|error| {
            format!("Failed to write download file to {} because of {}", to.display(), error)
        })?;
    } else {
        return Err(format!(
            "Unrecognized URI type: {}. Supported URI's are 'https://' and 'file//'",
            uri
        ));
    }

    Ok(())
}
