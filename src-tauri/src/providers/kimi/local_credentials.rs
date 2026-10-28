use std::path::{Path, PathBuf};

use serde_json::Value;

use chrono::Utc;

// Local OAuth credentials avoid requiring an API key. The Kimi CLI file carries no issuing-host
// marker, so a China-region token fails on api.kimi.ai with 401; CodexBar scopes its reuse the
// same way. Tokens are reused read-only: refreshing would rotate the refresh token out from
// under the owning client.
const FRESH_MARGIN_MILLIS: i64 = 60_000;
const MAX_FILE_BYTES: u64 = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum LocalKimiCredential {
    Fresh(String),
    Expired,
}

pub(super) fn load_from_home(home: &Path) -> Option<LocalKimiCredential> {
    cli_credential(home).or_else(|| pi_credential(home))
}

pub(super) fn env_home() -> Option<PathBuf> {
    // MSYS/Git-Bash style HOME values are not native Windows paths.
    let (preferred, fallback) = if cfg!(windows) {
        ("USERPROFILE", "HOME")
    } else {
        ("HOME", "USERPROFILE")
    };
    std::env::var_os(preferred)
        .or_else(|| std::env::var_os(fallback))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn cli_credential(home: &Path) -> Option<LocalKimiCredential> {
    let value = read_json(&home.join(".kimi-code/credentials/kimi-code.json"))?;
    let access = string_field(&value, "access_token")?;
    let expires_at = unix_seconds(value.get("expires_at"))?;
    Some(state(expires_at, access))
}

fn pi_credential(home: &Path) -> Option<LocalKimiCredential> {
    let value = read_json(&home.join(".pi/agent/auth.json"))?;
    let entry = value.get("kimi-coding")?;
    let access = string_field(entry, "access")?;
    let expires_at = unix_millis(entry.get("expires"))?;
    Some(state(expires_at, access))
}

fn state(expires_at_millis: i64, access: String) -> LocalKimiCredential {
    if expires_at_millis > Utc::now().timestamp_millis() + FRESH_MARGIN_MILLIS {
        LocalKimiCredential::Fresh(access)
    } else {
        LocalKimiCredential::Expired
    }
}

fn read_json(path: &Path) -> Option<Value> {
    use std::io::Read;

    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES).read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn unix_seconds(value: Option<&Value>) -> Option<i64> {
    epoch_number(value)?.saturating_mul(1_000).into()
}

fn unix_millis(value: Option<&Value>) -> Option<i64> {
    epoch_number(value)
}

fn epoch_number(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .filter(|number| number.is_finite())
        .map(|number| number as i64)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::{load_from_home, LocalKimiCredential, FRESH_MARGIN_MILLIS};

    fn test_now_plus_millis(millis: i64) -> i64 {
        chrono::Utc::now().timestamp_millis() + millis
    }

    fn write_credential(home: &Path, relative_path: &str, payload: &str) {
        let path = home.join(relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, payload).unwrap();
    }

    #[test]
    fn cli_file_and_pi_store_supply_fresh_tokens() {
        let future = test_now_plus_millis(3_600_000);

        let cli = tempdir().unwrap();
        write_credential(
            cli.path(),
            ".kimi-code/credentials/kimi-code.json",
            &format!(
                r#"{{"access_token":"cli-access","refresh_token":"cli-refresh","expires_at":{}}}"#,
                future / 1_000
            ),
        );
        assert_eq!(
            load_from_home(cli.path()),
            Some(LocalKimiCredential::Fresh("cli-access".into()))
        );

        let pi = tempdir().unwrap();
        write_credential(
            pi.path(),
            ".pi/agent/auth.json",
            &format!(
                r#"{{"kimi-coding":{{"type":"oauth","access":"pi-access","refresh":"pi-refresh","expires":{future}}}}}"#
            ),
        );
        assert_eq!(
            load_from_home(pi.path()),
            Some(LocalKimiCredential::Fresh("pi-access".into()))
        );
    }

    #[test]
    fn expired_credentials_are_reported_not_hidden() {
        let stale = test_now_plus_millis(FRESH_MARGIN_MILLIS - 1_000);
        let home = tempdir().unwrap();
        write_credential(
            home.path(),
            ".kimi-code/credentials/kimi-code.json",
            &format!(
                r#"{{"access_token":"cli-access","expires_at":{}}}"#,
                stale / 1_000
            ),
        );
        assert_eq!(
            load_from_home(home.path()),
            Some(LocalKimiCredential::Expired)
        );

        let pi = tempdir().unwrap();
        write_credential(
            pi.path(),
            ".pi/agent/auth.json",
            &format!(
                r#"{{"kimi-coding":{{"access":"pi-access","expires":{}}}}}"#,
                stale
            ),
        );
        assert_eq!(
            load_from_home(pi.path()),
            Some(LocalKimiCredential::Expired)
        );
    }

    #[test]
    fn missing_expiry_or_files_are_ignored() {
        let blank = tempdir().unwrap();
        write_credential(
            blank.path(),
            ".kimi-code/credentials/kimi-code.json",
            r#"{"access_token":"cli-access"}"#,
        );
        assert!(load_from_home(blank.path()).is_none());

        let empty_home = tempdir().unwrap();
        assert!(load_from_home(empty_home.path()).is_none());
    }

    #[test]
    fn string_timestamps_are_accepted() {
        let future = (test_now_plus_millis(3_600_000) / 1_000).to_string();
        let home = tempdir().unwrap();
        write_credential(
            home.path(),
            ".kimi-code/credentials/kimi-code.json",
            &format!(r#"{{"access_token":"cli-access","expires_at":"{future}"}}"#),
        );
        assert!(matches!(
            load_from_home(home.path()),
            Some(LocalKimiCredential::Fresh(_))
        ));
    }

    #[test]
    fn extreme_timestamps_do_not_wrap_into_the_future() {
        let home = tempdir().unwrap();
        write_credential(
            home.path(),
            ".kimi-code/credentials/kimi-code.json",
            r#"{"access_token":"cli-access","expires_at":"9223372036854775807"}"#,
        );
        assert!(matches!(
            load_from_home(home.path()),
            Some(LocalKimiCredential::Fresh(_))
        ));

        let near_overflow = tempdir().unwrap();
        write_credential(
            near_overflow.path(),
            ".kimi-code/credentials/kimi-code.json",
            r#"{"access_token":"cli-access","expires_at":9223372036854775807}"#,
        );
        // i64::MAX seconds saturates instead of wrapping negative when scaled to millis.
        assert!(matches!(
            load_from_home(near_overflow.path()),
            Some(LocalKimiCredential::Fresh(_))
        ));
    }
}
