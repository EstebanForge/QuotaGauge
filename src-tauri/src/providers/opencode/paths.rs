use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

use super::OpenCodeError;

const MAX_AUTH_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct OpenCodePaths {
    data_directory: PathBuf,
}

impl OpenCodePaths {
    pub(crate) fn new() -> Self {
        Self {
            data_directory: data_directory(|name| std::env::var(name).ok(), &home_directory()),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_data_directory(data_directory: PathBuf) -> Self {
        Self { data_directory }
    }

    pub(crate) fn go_api_key(&self) -> Result<Option<Zeroizing<String>>, OpenCodeError> {
        let path = self.data_directory.join("auth.json");
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(OpenCodeError::CredentialsUnreadable),
        };
        let metadata = file
            .metadata()
            .map_err(|_| OpenCodeError::CredentialsUnreadable)?;
        if !metadata.is_file() || metadata.len() > MAX_AUTH_FILE_BYTES {
            return Err(OpenCodeError::CredentialsUnreadable);
        }
        let mut content = Zeroizing::new(String::with_capacity(metadata.len() as usize));
        file.take(MAX_AUTH_FILE_BYTES + 1)
            .read_to_string(&mut content)
            .map_err(|_| OpenCodeError::CredentialsUnreadable)?;
        if content.len() as u64 > MAX_AUTH_FILE_BYTES {
            return Err(OpenCodeError::CredentialsUnreadable);
        }
        let mut value = serde_json::from_str::<Value>(&content)
            .map_err(|_| OpenCodeError::CredentialsUnreadable)?;
        let credentials = value
            .as_object_mut()
            .ok_or(OpenCodeError::CredentialsUnreadable)?;
        let Some(key_value) = credentials
            .get_mut("opencode-go")
            .and_then(Value::as_object_mut)
            .and_then(|entry| entry.get_mut("key"))
        else {
            return Ok(None);
        };
        let key = key_value
            .as_str()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(|key| Zeroizing::new(key.to_owned()));
        if let Value::String(value) = key_value {
            value.zeroize();
        }
        Ok(key)
    }
}

fn data_directory(environment: impl Fn(&str) -> Option<String>, home_directory: &Path) -> PathBuf {
    if let Some(configured) = environment("OPENCODE_DATA_DIR").and_then(non_empty) {
        return expand_home(&configured, home_directory);
    }
    if let Some(xdg_data_home) = environment("XDG_DATA_HOME").and_then(non_empty) {
        return expand_home(&xdg_data_home, home_directory).join("opencode");
    }
    home_directory.join(".local").join("share").join("opencode")
}

fn home_directory() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn expand_home(value: &str, home_directory: &Path) -> PathBuf {
    if value == "~" {
        return home_directory.to_path_buf();
    }
    if let Some(relative) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        return home_directory.join(relative);
    }
    PathBuf::from(value)
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs};

    use tempfile::tempdir;

    use super::{data_directory, OpenCodePaths, MAX_AUTH_FILE_BYTES};
    use crate::providers::opencode::OpenCodeError;

    #[test]
    fn data_directory_precedence_and_home_expansion_are_cross_platform() {
        let home = std::path::Path::new("/users/me");
        let environment = HashMap::from([
            ("OPENCODE_DATA_DIR", " ~/custom/opencode "),
            ("XDG_DATA_HOME", "/xdg/data"),
        ]);
        assert_eq!(
            data_directory(|name| environment.get(name).map(ToString::to_string), home),
            home.join("custom/opencode")
        );

        let xdg = HashMap::from([("XDG_DATA_HOME", "~/xdg")]);
        assert_eq!(
            data_directory(|name| xdg.get(name).map(ToString::to_string), home),
            home.join("xdg/opencode")
        );
        assert_eq!(
            data_directory(|_| None, home),
            home.join(".local/share/opencode")
        );
    }

    #[test]
    fn go_key_loader_tolerates_unrelated_entries_and_rejects_bad_storage() {
        let directory = tempdir().unwrap();
        let paths = OpenCodePaths::for_data_directory(directory.path().to_path_buf());
        assert_eq!(paths.go_api_key().unwrap(), None);

        fs::write(
            directory.path().join("auth.json"),
            r#"{"$schema":"v1","other":[],"opencode-go":{"type":"api","key":"  key-1  "}}"#,
        )
        .unwrap();
        let key = paths.go_api_key().unwrap().unwrap();
        assert_eq!(key.as_str(), "key-1");

        fs::write(directory.path().join("auth.json"), "not json").unwrap();
        assert_eq!(
            paths.go_api_key().unwrap_err(),
            OpenCodeError::CredentialsUnreadable
        );

        fs::write(directory.path().join("auth.json"), "[]").unwrap();
        assert_eq!(
            paths.go_api_key().unwrap_err(),
            OpenCodeError::CredentialsUnreadable
        );

        fs::write(
            directory.path().join("auth.json"),
            vec![b' '; MAX_AUTH_FILE_BYTES as usize + 1],
        )
        .unwrap();
        assert_eq!(
            paths.go_api_key().unwrap_err(),
            OpenCodeError::CredentialsUnreadable
        );
    }
}
