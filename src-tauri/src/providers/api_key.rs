use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use crate::models::ApiKeyStatus;
use zeroize::Zeroizing;

use super::credential_store::{delete_owned_password, read_owned_password, write_owned_password};

const SERVICE: &str = "xyz.actitud.quotagauge.api-key";
const LEGACY_SERVICE: &str = "io.github.deviffyy.openquota.api-key";
const COMBINED_ACCOUNT: &str = "api-keys";
// Windows Credential Manager caps the credential blob at 2560 bytes; enforce
// the same limit on every platform so oversize keys fail the same way.
const MAX_ITEM_BYTES: usize = 2560;

static SYSTEM_BACKEND: OnceLock<Arc<dyn SecretBackend>> = OnceLock::new();

fn system_backend() -> Arc<dyn SecretBackend> {
    SYSTEM_BACKEND
        .get_or_init(|| Arc::new(LegacyImportBackend::system()))
        .clone()
}

pub struct SecretBytes(Zeroizing<Vec<u8>>);

impl SecretBytes {
    pub fn new(value: Vec<u8>) -> Self {
        Self(Zeroizing::new(value))
    }

    fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }
}

pub struct SecretString(Zeroizing<String>);

impl SecretString {
    fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

pub trait SecretBackend: Send + Sync {
    fn read(&self, account: &str) -> Result<Option<SecretBytes>, String>;
    fn write(&self, account: &str, value: &[u8]) -> Result<(), String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

struct ServiceSecretBackend(&'static str);

impl SecretBackend for ServiceSecretBackend {
    fn read(&self, account: &str) -> Result<Option<SecretBytes>, String> {
        read_owned_password(self.0, account).map(|value| value.map(SecretBytes::new))
    }

    fn write(&self, account: &str, value: &[u8]) -> Result<(), String> {
        write_owned_password(self.0, account, value)
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        delete_owned_password(self.0, account)
    }
}

/// Stores every provider key as one JSON map inside a single credential-store
/// item, so an ad-hoc signed build triggers one access prompt per launch
/// instead of one per provider. On a map miss it folds in that account's
/// per-account item and removes it.
struct CombinedSecretBackend {
    inner: Arc<dyn SecretBackend>,
    lock: Mutex<()>,
}

impl CombinedSecretBackend {
    fn new(inner: Arc<dyn SecretBackend>) -> Self {
        Self {
            inner,
            lock: Mutex::new(()),
        }
    }

    fn guard(&self, account: &str) -> Result<(), String> {
        if account == COMBINED_ACCOUNT {
            return Err(format!(
                "The account name {COMBINED_ACCOUNT:?} is reserved."
            ));
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn load(&self) -> Result<BTreeMap<String, String>, String> {
        let Some(bytes) = self.inner.read(COMBINED_ACCOUNT)? else {
            return Ok(BTreeMap::new());
        };
        serde_json::from_slice(bytes.as_slice())
            .map_err(|_| "The combined API key item could not be read.".to_owned())
    }

    fn store(&self, map: &BTreeMap<String, String>) -> Result<(), String> {
        let serialized = Zeroizing::new(
            serde_json::to_vec(map)
                .map_err(|_| "The combined API key item could not be encoded.".to_owned())?,
        );
        if serialized.len() > MAX_ITEM_BYTES {
            return Err(format!(
                "Storing all API keys together would exceed the {MAX_ITEM_BYTES}-byte limit of the system credential store."
            ));
        }
        self.inner.write(COMBINED_ACCOUNT, serialized.as_slice())
    }
}

impl SecretBackend for CombinedSecretBackend {
    fn read(&self, account: &str) -> Result<Option<SecretBytes>, String> {
        self.guard(account)?;
        let _guard = self.lock();
        let mut map = self.load()?;
        if let Some(value) = map.get(account) {
            return Ok(Some(SecretBytes::new(value.clone().into_bytes())));
        }
        let Some(old) = self.inner.read(account)? else {
            return Ok(None);
        };
        let value = String::from_utf8(old.as_slice().to_vec())
            .map_err(|_| "The saved API key has an unsupported encoding.".to_owned())?;
        map.insert(account.to_owned(), value.clone());
        match self.store(&map) {
            Ok(()) => {
                if let Err(error) = self.inner.delete(account) {
                    crate::app_warn!(
                        &format!("auth:{account}"),
                        "old credential item not removed ({error})"
                    );
                }
            }
            Err(error) => {
                crate::app_warn!(
                    &format!("auth:{account}"),
                    "combined credential item not updated ({error})"
                );
            }
        }
        Ok(Some(SecretBytes::new(value.into_bytes())))
    }

    fn write(&self, account: &str, value: &[u8]) -> Result<(), String> {
        self.guard(account)?;
        let text = std::str::from_utf8(value)
            .map_err(|_| "The API key has an unsupported encoding.".to_owned())?;
        let _guard = self.lock();
        let mut map = match self.load() {
            Ok(map) => map,
            Err(error) => {
                crate::app_warn!(
                    &format!("auth:{account}"),
                    "unreadable combined credential item replaced ({error})"
                );
                BTreeMap::new()
            }
        };
        map.insert(account.to_owned(), text.to_owned());
        self.store(&map)
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        self.guard(account)?;
        // Remove the per-account item first so a failed sweep can never
        // resurrect a deleted key through the fold path.
        self.inner.delete(account)?;
        let _guard = self.lock();
        let mut map = match self.load() {
            Ok(map) => map,
            Err(error) => {
                crate::app_warn!(
                    &format!("auth:{account}"),
                    "unreadable combined credential item cleared ({error})"
                );
                BTreeMap::new()
            }
        };
        map.remove(account);
        if map.is_empty() {
            self.inner.delete(COMBINED_ACCOUNT)
        } else {
            self.store(&map)
        }
    }
}

/// Serves entries from the current keychain service and, on first access,
/// copies them over from the pre-rename service so OpenQuota users keep
/// their saved API keys.
struct LegacyImportBackend {
    current: Arc<dyn SecretBackend>,
    legacy: Arc<dyn SecretBackend>,
}

impl LegacyImportBackend {
    fn system() -> Self {
        Self {
            current: Arc::new(CombinedSecretBackend::new(Arc::new(ServiceSecretBackend(
                SERVICE,
            )))),
            legacy: Arc::new(ServiceSecretBackend(LEGACY_SERVICE)),
        }
    }
}

impl SecretBackend for LegacyImportBackend {
    fn read(&self, account: &str) -> Result<Option<SecretBytes>, String> {
        match self.current.read(account)? {
            Some(value) => Ok(Some(value)),
            None => match self.legacy.read(account) {
                Ok(Some(value)) => {
                    if let Err(error) = self.current.write(account, value.as_slice()) {
                        crate::app_warn!(
                            &format!("auth:{account}"),
                            "legacy API key copy failed ({error})"
                        );
                    }
                    Ok(Some(value))
                }
                Ok(None) => Ok(None),
                Err(error) => {
                    crate::app_warn!(
                        &format!("auth:{account}"),
                        "legacy keychain read failed ({error})"
                    );
                    Ok(None)
                }
            },
        }
    }

    fn write(&self, account: &str, value: &[u8]) -> Result<(), String> {
        self.current.write(account, value)
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        // Clear both services so a saved-key reset leaves nothing behind.
        let current = self.current.delete(account);
        let _ = self.legacy.delete(account);
        current
    }
}

pub trait EnvironmentReader: Send + Sync {
    fn value(&self, name: &str) -> Option<String>;
}

#[derive(Default)]
struct ProcessEnvironment;

impl EnvironmentReader for ProcessEnvironment {
    fn value(&self, name: &str) -> Option<String> {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    }
}

pub trait ConfigFileReader: Send + Sync {
    fn read(&self, path: &str) -> Option<SecretBytes>;
}

#[derive(Default)]
struct ProcessConfigFiles;

impl ConfigFileReader for ProcessConfigFiles {
    fn read(&self, path: &str) -> Option<SecretBytes> {
        fs::read(expand_home(path)).ok().map(SecretBytes::new)
    }
}

#[derive(Clone)]
pub struct ApiKeyStore {
    provider_id: String,
    environment_names: Vec<String>,
    config_paths: Vec<String>,
    secrets: Arc<dyn SecretBackend>,
    environment: Arc<dyn EnvironmentReader>,
    config_files: Arc<dyn ConfigFileReader>,
}

impl ApiKeyStore {
    pub fn new_with_sources(
        provider_id: &str,
        environment_names: &[&str],
        config_paths: &[&str],
    ) -> Self {
        Self {
            provider_id: provider_id.to_owned(),
            environment_names: environment_names
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            config_paths: config_paths
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            secrets: system_backend(),
            environment: Arc::new(ProcessEnvironment),
            config_files: Arc::new(ProcessConfigFiles),
        }
    }

    #[cfg(test)]
    pub fn with_backends(
        provider_id: &str,
        environment_name: &str,
        secrets: Arc<dyn SecretBackend>,
        environment: Arc<dyn EnvironmentReader>,
    ) -> Self {
        Self {
            provider_id: provider_id.to_owned(),
            environment_names: vec![environment_name.to_owned()],
            config_paths: Vec::new(),
            secrets,
            environment,
            config_files: Arc::new(ProcessConfigFiles),
        }
    }

    #[cfg(test)]
    pub fn with_source_backends(
        provider_id: &str,
        environment_names: &[&str],
        config_paths: &[&str],
        secrets: Arc<dyn SecretBackend>,
        environment: Arc<dyn EnvironmentReader>,
        config_files: Arc<dyn ConfigFileReader>,
    ) -> Self {
        Self {
            provider_id: provider_id.to_owned(),
            environment_names: environment_names
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            config_paths: config_paths
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            secrets,
            environment,
            config_files,
        }
    }

    pub fn load(&self) -> Result<Option<SecretString>, String> {
        match self.saved_key() {
            Ok(Some(value)) => Ok(Some(value)),
            Ok(None) => Ok(self.external_key().map(|(value, _)| value)),
            Err(error) => match self.external_key() {
                Some((value, _)) => {
                    report_external_fallback(&self.provider_id, &error);
                    Ok(Some(value))
                }
                None => Err(error),
            },
        }
    }

    pub fn status(&self) -> Result<ApiKeyStatus, String> {
        let external = self.external_key().map(|(_, status)| status);
        let saved = match self.saved_key() {
            Ok(value) => value.is_some(),
            Err(error) if external.is_some() => {
                report_external_fallback(&self.provider_id, &error);
                return Ok(external.expect("external source checked above"));
            }
            Err(error) => return Err(error),
        };
        Ok(if saved {
            if external.is_some() {
                ApiKeyStatus::OverrideActive
            } else {
                ApiKeyStatus::Saved
            }
        } else {
            external.unwrap_or(ApiKeyStatus::NotSet)
        })
    }

    pub fn save(&self, value: &str) -> Result<(), String> {
        let value = value.trim();
        if value.is_empty() {
            return Err("Enter an API key before saving.".into());
        }
        self.secrets.write(&self.provider_id, value.as_bytes())
    }

    pub fn delete(&self) -> Result<(), String> {
        self.secrets.delete(&self.provider_id)
    }

    fn saved_key(&self) -> Result<Option<SecretString>, String> {
        let Some(value) = self.secrets.read(&self.provider_id)? else {
            return Ok(None);
        };
        let value = std::str::from_utf8(value.as_slice())
            .map_err(|_| "The saved API key has an unsupported encoding.".to_owned())?;
        Ok(non_empty(value.to_owned()))
    }

    fn environment_key(&self) -> Option<SecretString> {
        self.environment_names
            .iter()
            .find_map(|name| self.environment.value(name).and_then(non_empty))
    }

    fn config_key(&self) -> Option<SecretString> {
        self.config_paths.iter().find_map(|path| {
            self.config_files
                .read(path)
                .and_then(|value| key_from_config(value.as_slice()))
        })
    }

    fn external_key(&self) -> Option<(SecretString, ApiKeyStatus)> {
        self.config_key()
            .map(|value| (value, ApiKeyStatus::FromConfig))
            .or_else(|| {
                self.environment_key()
                    .map(|value| (value, ApiKeyStatus::FromEnvironment))
            })
    }
}

fn report_external_fallback(provider_id: &str, error: &str) {
    crate::app_warn!(
        &format!("auth:{provider_id}"),
        "system credential store unavailable; using external API key source ({error})"
    );
}

fn key_from_config(bytes: &[u8]) -> Option<SecretString> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    if text.starts_with('{') {
        let object = serde_json::from_str::<serde_json::Value>(text)
            .ok()?
            .as_object()?
            .clone();
        return ["apiKey", "api_key", "key"]
            .iter()
            .find_map(|name| object.get(*name)?.as_str().map(str::to_owned))
            .and_then(non_empty);
    }
    non_empty(text.to_owned())
}

fn expand_home(path: &str) -> PathBuf {
    let Some(relative) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) else {
        return PathBuf::from(path);
    };
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(relative)
}

fn non_empty(value: String) -> Option<SecretString> {
    let value = SecretString::new(value);
    let trimmed = value.as_str().trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() == value.len() {
        Some(value)
    } else {
        Some(SecretString::new(trimmed.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use crate::models::ApiKeyStatus;

    use super::{
        ApiKeyStore, ConfigFileReader, EnvironmentReader, LegacyImportBackend, SecretBackend,
        SecretBytes,
    };

    #[derive(Default)]
    struct MemorySecrets(Mutex<HashMap<String, Vec<u8>>>);

    impl SecretBackend for MemorySecrets {
        fn read(&self, account: &str) -> Result<Option<SecretBytes>, String> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(account)
                .cloned()
                .map(SecretBytes::new))
        }

        fn write(&self, account: &str, value: &[u8]) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .insert(account.to_owned(), value.to_vec());
            Ok(())
        }

        fn delete(&self, account: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(account);
            Ok(())
        }
    }

    struct MemoryEnvironment(HashMap<String, String>);

    impl EnvironmentReader for MemoryEnvironment {
        fn value(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    struct MemoryConfigFiles(HashMap<String, Vec<u8>>);

    impl ConfigFileReader for MemoryConfigFiles {
        fn read(&self, path: &str) -> Option<SecretBytes> {
            self.0.get(path).cloned().map(SecretBytes::new)
        }
    }

    struct ReadErrorSecrets;

    impl SecretBackend for ReadErrorSecrets {
        fn read(&self, _account: &str) -> Result<Option<SecretBytes>, String> {
            Err("System credential store unavailable.".into())
        }

        fn write(&self, _account: &str, _value: &[u8]) -> Result<(), String> {
            Err("System credential store unavailable.".into())
        }

        fn delete(&self, _account: &str) -> Result<(), String> {
            Err("System credential store unavailable.".into())
        }
    }

    fn store(secrets: Arc<MemorySecrets>, environment: &[(&str, &str)]) -> ApiKeyStore {
        ApiKeyStore::with_backends(
            "provider",
            "PROVIDER_API_KEY",
            secrets,
            Arc::new(MemoryEnvironment(
                environment
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                    .collect(),
            )),
        )
    }

    #[test]
    fn saved_key_overrides_environment_and_delete_falls_back() {
        let secrets = Arc::new(MemorySecrets::default());
        let store = store(secrets, &[("PROVIDER_API_KEY", " environment-key ")]);
        assert_eq!(store.status().unwrap(), ApiKeyStatus::FromEnvironment);
        assert_eq!(
            store.load().unwrap().as_ref().map(|value| value.as_str()),
            Some("environment-key")
        );

        store.save(" saved-key ").unwrap();
        assert_eq!(store.status().unwrap(), ApiKeyStatus::OverrideActive);
        assert_eq!(
            store.load().unwrap().as_ref().map(|value| value.as_str()),
            Some("saved-key")
        );

        store.delete().unwrap();
        assert_eq!(store.status().unwrap(), ApiKeyStatus::FromEnvironment);
        assert_eq!(
            store.load().unwrap().as_ref().map(|value| value.as_str()),
            Some("environment-key")
        );
    }

    #[test]
    fn saved_only_and_empty_states_are_reported_without_exposing_the_key() {
        let secrets = Arc::new(MemorySecrets::default());
        let store = store(secrets, &[]);
        assert_eq!(store.status().unwrap(), ApiKeyStatus::NotSet);
        assert!(store.save("  ").is_err());

        store.save("secret").unwrap();
        assert_eq!(store.status().unwrap(), ApiKeyStatus::Saved);
    }

    #[test]
    fn environment_key_remains_available_when_the_system_store_cannot_be_read() {
        let store = ApiKeyStore::with_backends(
            "provider",
            "PROVIDER_API_KEY",
            Arc::new(ReadErrorSecrets),
            Arc::new(MemoryEnvironment(HashMap::from([(
                "PROVIDER_API_KEY".into(),
                " environment-key ".into(),
            )]))),
        );

        assert_eq!(store.status().unwrap(), ApiKeyStatus::FromEnvironment);
        assert_eq!(
            store.load().unwrap().as_ref().map(|value| value.as_str()),
            Some("environment-key")
        );
    }

    #[test]
    fn credential_store_errors_are_not_hidden_without_an_environment_key() {
        let store = ApiKeyStore::with_backends(
            "provider",
            "PROVIDER_API_KEY",
            Arc::new(ReadErrorSecrets),
            Arc::new(MemoryEnvironment(HashMap::new())),
        );

        assert_eq!(
            store.status().unwrap_err(),
            "System credential store unavailable."
        );
        assert_eq!(
            store.load().err().as_deref(),
            Some("System credential store unavailable.")
        );
    }

    #[test]
    fn config_file_precedes_environment_and_saved_key_can_override_it() {
        let secrets = Arc::new(MemorySecrets::default());
        let store = ApiKeyStore::with_source_backends(
            "provider",
            &["PRIMARY_KEY", "ALTERNATE_KEY"],
            &["~/first.json", "~/second.json"],
            secrets,
            Arc::new(MemoryEnvironment(HashMap::from([
                ("PRIMARY_KEY".into(), "environment-key".into()),
                ("ALTERNATE_KEY".into(), "alternate-key".into()),
            ]))),
            Arc::new(MemoryConfigFiles(HashMap::from([(
                "~/second.json".into(),
                br#"{"api_key":"config-key"}"#.to_vec(),
            )]))),
        );

        assert_eq!(store.status().unwrap(), ApiKeyStatus::FromConfig);
        assert_eq!(
            store.load().unwrap().as_ref().map(|value| value.as_str()),
            Some("config-key")
        );

        store.save("saved-key").unwrap();
        assert_eq!(store.status().unwrap(), ApiKeyStatus::OverrideActive);
        assert_eq!(
            store.load().unwrap().as_ref().map(|value| value.as_str()),
            Some("saved-key")
        );
    }

    #[test]
    fn alternate_environment_name_and_plain_text_config_are_supported() {
        let secrets = Arc::new(MemorySecrets::default());
        let environment_store = ApiKeyStore::with_source_backends(
            "provider",
            &["PRIMARY_KEY", "ALTERNATE_KEY"],
            &[],
            secrets.clone(),
            Arc::new(MemoryEnvironment(HashMap::from([(
                "ALTERNATE_KEY".into(),
                " alternate-key ".into(),
            )]))),
            Arc::new(MemoryConfigFiles(HashMap::new())),
        );
        assert_eq!(
            environment_store
                .load()
                .unwrap()
                .as_ref()
                .map(|value| value.as_str()),
            Some("alternate-key")
        );

        let config_store = ApiKeyStore::with_source_backends(
            "provider",
            &[],
            &["~/key.txt"],
            secrets,
            Arc::new(MemoryEnvironment(HashMap::new())),
            Arc::new(MemoryConfigFiles(HashMap::from([(
                "~/key.txt".into(),
                b" plain-text-key \n".to_vec(),
            )]))),
        );
        assert_eq!(config_store.status().unwrap(), ApiKeyStatus::FromConfig);
        assert_eq!(
            config_store
                .load()
                .unwrap()
                .as_ref()
                .map(|value| value.as_str()),
            Some("plain-text-key")
        );
    }

    fn legacy_store(
        current: Arc<dyn SecretBackend>,
        legacy: Arc<dyn SecretBackend>,
    ) -> ApiKeyStore {
        ApiKeyStore::with_source_backends(
            "provider",
            &["PROVIDER_API_KEY"],
            &[],
            Arc::new(LegacyImportBackend { current, legacy }),
            Arc::new(MemoryEnvironment(HashMap::new())),
            Arc::new(MemoryConfigFiles(HashMap::new())),
        )
    }

    #[test]
    fn falls_back_to_legacy_entry_and_copies_it_under_current_service() {
        let current = Arc::new(MemorySecrets::default());
        let legacy = Arc::new(MemorySecrets::default());
        legacy.write("provider", b"legacy-key").unwrap();

        let loaded = legacy_store(current.clone(), legacy)
            .load()
            .unwrap()
            .map(|value| value.as_str().to_owned());

        assert_eq!(loaded.as_deref(), Some("legacy-key"));
        assert_eq!(
            current.read("provider").unwrap().unwrap().as_slice(),
            b"legacy-key"
        );
    }

    #[test]
    fn current_entry_takes_precedence_over_legacy() {
        let current = Arc::new(MemorySecrets::default());
        let legacy = Arc::new(MemorySecrets::default());
        current.write("provider", b"current-key").unwrap();
        legacy.write("provider", b"legacy-key").unwrap();

        let loaded = legacy_store(current, legacy)
            .load()
            .unwrap()
            .map(|value| value.as_str().to_owned());

        assert_eq!(loaded.as_deref(), Some("current-key"));
    }

    #[test]
    fn delete_clears_legacy_entry_too() {
        let current = Arc::new(MemorySecrets::default());
        let legacy = Arc::new(MemorySecrets::default());
        current.write("provider", b"current-key").unwrap();
        legacy.write("provider", b"legacy-key").unwrap();

        legacy_store(current.clone(), legacy.clone())
            .delete()
            .unwrap();

        assert!(current.read("provider").unwrap().is_none());
        assert!(legacy.read("provider").unwrap().is_none());
    }

    #[test]
    fn legacy_read_error_behaves_as_missing_entry() {
        let loaded = legacy_store(
            Arc::new(MemorySecrets::default()),
            Arc::new(ReadErrorSecrets),
        )
        .load()
        .unwrap();

        assert!(loaded.is_none());
    }

    struct ReadOnlySecrets;

    impl SecretBackend for ReadOnlySecrets {
        fn read(&self, _account: &str) -> Result<Option<SecretBytes>, String> {
            Ok(None)
        }

        fn write(&self, _account: &str, _value: &[u8]) -> Result<(), String> {
            Err("System credential store rejected the write.".into())
        }

        fn delete(&self, _account: &str) -> Result<(), String> {
            Err("System credential store rejected the write.".into())
        }
    }

    #[test]
    fn legacy_key_still_serves_when_the_copy_write_fails() {
        let legacy = Arc::new(MemorySecrets::default());
        legacy.write("provider", b"legacy-key").unwrap();

        let loaded = legacy_store(Arc::new(ReadOnlySecrets), legacy)
            .load()
            .unwrap()
            .map(|value| value.as_str().to_owned());

        assert_eq!(loaded.as_deref(), Some("legacy-key"));
    }

    struct InnerWithFailingAccount {
        inner: MemorySecrets,
        fail_write_for: &'static str,
        fail_delete_for: &'static str,
        combined_write_armed: std::sync::atomic::AtomicBool,
    }

    impl InnerWithFailingAccount {
        fn new(fail_write_for: &'static str, fail_delete_for: &'static str) -> Self {
            Self {
                inner: MemorySecrets::default(),
                fail_write_for,
                fail_delete_for,
                combined_write_armed: std::sync::atomic::AtomicBool::new(false),
            }
        }

        fn arm_combined_write_failure(&self) {
            self.combined_write_armed
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl SecretBackend for InnerWithFailingAccount {
        fn read(&self, account: &str) -> Result<Option<SecretBytes>, String> {
            self.inner.read(account)
        }

        fn write(&self, account: &str, value: &[u8]) -> Result<(), String> {
            if account == self.fail_write_for
                || (account == super::COMBINED_ACCOUNT
                    && self
                        .combined_write_armed
                        .swap(false, std::sync::atomic::Ordering::SeqCst))
            {
                return Err("The combined item could not be stored.".into());
            }
            self.inner.write(account, value)
        }

        fn delete(&self, account: &str) -> Result<(), String> {
            if account == self.fail_delete_for {
                return Err("The old item could not be removed.".into());
            }
            self.inner.delete(account)
        }
    }

    fn combined(inner: Arc<dyn SecretBackend>) -> super::CombinedSecretBackend {
        super::CombinedSecretBackend::new(inner)
    }

    fn entries(backend: &super::CombinedSecretBackend) -> Vec<String> {
        match backend.inner.read(super::COMBINED_ACCOUNT).unwrap() {
            Some(value) => serde_json::from_slice::<std::collections::BTreeMap<String, String>>(
                value.as_slice(),
            )
            .unwrap()
            .into_keys()
            .collect(),
            None => Vec::new(),
        }
    }

    #[test]
    fn combined_writes_keep_one_item_and_other_accounts() {
        let inner = Arc::new(MemorySecrets::default());
        let backend = combined(inner.clone());

        backend.write("openrouter", b"first").unwrap();
        backend.write("zai", b"second").unwrap();
        backend.write("openrouter", b"third").unwrap();

        assert_eq!(
            entries(&backend),
            ["openrouter".to_owned(), "zai".to_owned()]
        );
        assert_eq!(
            backend.read("openrouter").unwrap().unwrap().as_slice(),
            b"third"
        );
        assert_eq!(backend.read("zai").unwrap().unwrap().as_slice(), b"second");
        assert!(inner.read("openrouter").unwrap().is_none());
        assert!(inner.read("zai").unwrap().is_none());
    }

    #[test]
    fn delete_keeps_other_accounts_and_the_last_delete_empties_inner() {
        let inner = Arc::new(MemorySecrets::default());
        let backend = combined(inner.clone());
        backend.write("openrouter", b"first").unwrap();
        backend.write("zai", b"second").unwrap();

        backend.delete("openrouter").unwrap();
        assert!(backend.read("openrouter").unwrap().is_none());
        assert_eq!(backend.read("zai").unwrap().unwrap().as_slice(), b"second");

        backend.delete("zai").unwrap();
        assert!(inner.read(super::COMBINED_ACCOUNT).unwrap().is_none());
    }

    #[test]
    fn missing_accounts_are_served_without_creating_items() {
        let inner = Arc::new(MemorySecrets::default());
        let backend = combined(inner.clone());

        assert!(backend.read("openrouter").unwrap().is_none());
        backend.delete("openrouter").unwrap();

        assert!(inner.read(super::COMBINED_ACCOUNT).unwrap().is_none());
    }

    #[test]
    fn the_reserved_account_name_is_rejected() {
        let backend = combined(Arc::new(MemorySecrets::default()));

        assert!(backend.read(super::COMBINED_ACCOUNT).is_err());
        assert!(backend.write(super::COMBINED_ACCOUNT, b"value").is_err());
        assert!(backend.delete(super::COMBINED_ACCOUNT).is_err());
    }

    #[test]
    fn a_corrupt_map_blocks_reads_until_a_write_replaces_it() {
        let inner = Arc::new(MemorySecrets::default());
        let backend = combined(inner.clone());
        inner.write(super::COMBINED_ACCOUNT, b"not-json").unwrap();

        assert!(backend.read("openrouter").is_err());

        backend.write("openrouter", b"value").unwrap();
        assert_eq!(
            backend.read("openrouter").unwrap().unwrap().as_slice(),
            b"value"
        );
    }

    #[test]
    fn an_oversize_write_is_rejected_and_leaves_the_previous_map() {
        let inner = Arc::new(MemorySecrets::default());
        let backend = combined(inner.clone());
        backend.write("openrouter", b"kept").unwrap();

        let oversize = vec![b'x'; super::MAX_ITEM_BYTES];
        assert!(backend.write("zai", &oversize).is_err());
        assert_eq!(
            backend.read("openrouter").unwrap().unwrap().as_slice(),
            b"kept"
        );
        assert!(backend.read("zai").unwrap().is_none());
    }

    #[test]
    fn a_non_utf8_write_is_rejected() {
        let backend = combined(Arc::new(MemorySecrets::default()));

        assert!(backend.write("openrouter", &[0xff, 0xfe]).is_err());
    }

    #[test]
    fn an_inner_write_failure_leaves_the_previous_map() {
        let inner = Arc::new(InnerWithFailingAccount::new("", ""));
        let backend = combined(inner.clone());
        backend.write("openrouter", b"kept").unwrap();
        inner.arm_combined_write_failure();

        assert!(backend.write("zai", b"value").is_err());
        assert_eq!(
            backend.read("openrouter").unwrap().unwrap().as_slice(),
            b"kept"
        );
        assert_eq!(entries(&backend), ["openrouter".to_owned()]);
    }

    #[test]
    fn a_miss_folds_the_old_item_into_the_map_and_removes_it() {
        let inner = Arc::new(MemorySecrets::default());
        inner.write("openrouter", b"old-key").unwrap();
        let backend = combined(inner.clone());

        assert_eq!(
            backend.read("openrouter").unwrap().unwrap().as_slice(),
            b"old-key"
        );
        assert!(inner.read("openrouter").unwrap().is_none());
        assert_eq!(entries(&backend), ["openrouter".to_owned()]);
    }

    #[test]
    fn folding_one_account_leaves_other_old_items_alone() {
        let inner = Arc::new(MemorySecrets::default());
        inner.write("openrouter", b"old-a").unwrap();
        inner.write("zai", b"old-b").unwrap();
        let backend = combined(inner.clone());

        backend.read("openrouter").unwrap();

        assert_eq!(inner.read("zai").unwrap().unwrap().as_slice(), b"old-b");
    }

    #[test]
    fn the_map_entry_wins_over_a_stray_old_item() {
        let inner = Arc::new(MemorySecrets::default());
        let backend = combined(inner.clone());
        backend.write("openrouter", b"map-key").unwrap();
        inner.write("openrouter", b"old-key").unwrap();

        assert_eq!(
            backend.read("openrouter").unwrap().unwrap().as_slice(),
            b"map-key"
        );
    }

    #[test]
    fn delete_removes_the_old_item_too_and_a_failed_old_delete_stops_it() {
        let failing = Arc::new(InnerWithFailingAccount::new("", "openrouter"));
        failing.write("openrouter", b"old-key").unwrap();
        let backend = combined(failing.clone());
        backend.read("openrouter").unwrap();
        // simulate the crash window: the fold could not remove the old item
        failing.inner.write("openrouter", b"old-key").unwrap();

        assert!(backend.delete("openrouter").is_err());
        assert_eq!(entries(&backend), ["openrouter".to_owned()]);

        let inner = Arc::new(MemorySecrets::default());
        inner.write("openrouter", b"old-key").unwrap();
        let backend = combined(inner.clone());
        backend.read("openrouter").unwrap();
        backend.delete("openrouter").unwrap();
        assert!(backend.read("openrouter").unwrap().is_none());
        assert!(inner.read("openrouter").unwrap().is_none());
    }

    #[test]
    fn a_failed_old_item_read_on_a_miss_is_an_error() {
        let backend = combined(Arc::new(ReadErrorSecrets));

        assert!(backend.read("openrouter").is_err());
    }

    #[test]
    fn an_oversize_fold_serves_the_value_without_persisting_it() {
        let inner = Arc::new(MemorySecrets::default());
        let oversize = vec![b'x'; super::MAX_ITEM_BYTES];
        inner.write("openrouter", &oversize).unwrap();
        let backend = combined(inner.clone());

        let served = backend.read("openrouter").unwrap().unwrap();
        assert_eq!(served.as_slice().len(), super::MAX_ITEM_BYTES);
        assert!(inner.read(super::COMBINED_ACCOUNT).unwrap().is_none());
    }

    struct SleepySecrets {
        inner: MemorySecrets,
    }

    impl SecretBackend for SleepySecrets {
        fn read(&self, account: &str) -> Result<Option<SecretBytes>, String> {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.inner.read(account)
        }

        fn write(&self, account: &str, value: &[u8]) -> Result<(), String> {
            self.inner.write(account, value)
        }

        fn delete(&self, account: &str) -> Result<(), String> {
            self.inner.delete(account)
        }
    }

    #[test]
    fn concurrent_writes_and_folds_all_land_without_duplication() {
        let inner = Arc::new(SleepySecrets {
            inner: MemorySecrets::default(),
        });
        for account in ["a", "b", "c", "d"] {
            inner
                .write(account, format!("old-{account}").as_bytes())
                .unwrap();
        }
        let backend = Arc::new(combined(inner.clone()));

        let handles: Vec<_> = ["a", "b", "c", "d"]
            .into_iter()
            .map(|account| {
                let backend = backend.clone();
                std::thread::spawn(move || {
                    backend
                        .write(account, format!("new-{account}").as_bytes())
                        .unwrap();
                    assert_eq!(
                        backend.read(account).unwrap().unwrap().as_slice(),
                        format!("new-{account}").as_bytes()
                    );
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(
            entries(&backend),
            [
                "a".to_owned(),
                "b".to_owned(),
                "c".to_owned(),
                "d".to_owned()
            ]
        );
        for account in ["a", "b", "c", "d"] {
            assert_eq!(
                backend.read(account).unwrap().unwrap().as_slice(),
                format!("new-{account}").as_bytes()
            );
        }
    }

    #[test]
    fn the_system_chain_is_a_single_shared_instance() {
        assert!(Arc::ptr_eq(
            &super::system_backend(),
            &super::system_backend()
        ));
    }

    #[test]
    fn a_legacy_hit_lands_in_the_combined_map_not_a_per_account_item() {
        let inner = Arc::new(MemorySecrets::default());
        let legacy = Arc::new(MemorySecrets::default());
        legacy.write("provider", b"legacy-key").unwrap();
        let backend = Arc::new(super::LegacyImportBackend {
            current: Arc::new(combined(inner.clone())),
            legacy,
        });

        let loaded = {
            let store = ApiKeyStore::with_source_backends(
                "provider",
                &["PROVIDER_API_KEY"],
                &[],
                backend,
                Arc::new(MemoryEnvironment(HashMap::new())),
                Arc::new(MemoryConfigFiles(HashMap::new())),
            );
            store.load().unwrap().map(|value| value.as_str().to_owned())
        };

        assert_eq!(loaded.as_deref(), Some("legacy-key"));
        assert_eq!(
            {
                let combined = super::CombinedSecretBackend::new(inner.clone());
                combined
                    .read("provider")
                    .unwrap()
                    .map(|value| value.as_slice().to_owned())
            },
            Some(b"legacy-key".to_vec())
        );
        assert!(inner.read("provider").unwrap().is_none());
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    #[test]
    fn combined_store_round_trip_and_fold_when_requested() {
        if std::env::var("QUOTAGAUGE_TEST_CREDENTIAL_STORE").as_deref() != Ok("1") {
            return;
        }

        let leaked: &'static str = Box::leak(
            format!(
                "xyz.actitud.quotagauge.combined-test.{}",
                std::process::id()
            )
            .into_boxed_str(),
        );
        let inner = Arc::new(super::ServiceSecretBackend(leaked));
        let result = (|| -> Result<(), String> {
            super::ServiceSecretBackend(leaked).write("openrouter", b"old-value")?;
            let backend = combined(inner.clone());
            let served = backend
                .read("openrouter")?
                .ok_or("The folded key was missing.")?;
            if served.as_slice() != b"old-value" {
                return Err("The folded key did not match.".into());
            }
            if inner.read("openrouter")?.is_some() {
                return Err("The old item survived the fold.".into());
            }
            backend.write("zai", b"new-value")?;
            let served = backend.read("zai")?;
            if served.map(|value| value.as_slice().to_vec()) != Some(b"new-value".to_vec()) {
                return Err("The combined write did not round-trip.".into());
            }
            backend.delete("openrouter")?;
            backend.delete("zai")?;
            if inner.read(super::COMBINED_ACCOUNT)?.is_some() {
                return Err("The combined item survived the last delete.".into());
            }
            Ok(())
        })();
        let cleanup = inner.delete(super::COMBINED_ACCOUNT);

        result.unwrap();
        cleanup.unwrap();
    }
}
