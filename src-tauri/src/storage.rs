use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use rusqlite::{params, Connection, OptionalExtension};
use thiserror::Error;

use crate::{
    models::{AppSettings, ProviderSnapshot},
    providers::CacheIdentity,
};

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("QuotaGauge data directory could not be created")]
    CreateDirectory(#[source] std::io::Error),
    #[error("QuotaGauge database is unavailable")]
    Database(#[from] rusqlite::Error),
    #[error("Cached QuotaGauge data is invalid")]
    InvalidCache(#[from] serde_json::Error),
    #[error("QuotaGauge database lock is unavailable")]
    Poisoned,
    #[error("Corrupt QuotaGauge database could not be quarantined")]
    Quarantine(#[source] std::io::Error),
}

pub struct Storage {
    path: PathBuf,
    connection: Mutex<Connection>,
}

pub struct ProviderAccountUpdate {
    pub provider_family: String,
    pub identity_key: String,
    pub provider_id: String,
    pub payload: String,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(StorageError::CreateDirectory)?;
        }
        match Self::open_connection(path) {
            Ok(connection) => Ok(Self {
                path: path.to_path_buf(),
                connection: Mutex::new(connection),
            }),
            Err(error) => {
                // An unopenable database (corrupt header, partial copy) must not brick
                // every launch; quarantine the file and start fresh.
                crate::app_error!(
                    "storage",
                    "database unopenable ({error}); quarantining and starting fresh"
                );
                let quarantine = path.with_extension(format!(
                    "db.corrupt-{}",
                    chrono::Utc::now().format("%Y%m%d%H%M%S")
                ));
                fs::rename(path, &quarantine).map_err(StorageError::Quarantine)?;
                Ok(Self {
                    path: path.to_path_buf(),
                    connection: Mutex::new(Self::open_connection(path)?),
                })
            }
        }
    }

    fn open_connection(path: &Path) -> Result<Connection, StorageError> {
        let connection = Connection::open(path)?;
        // Snapshot writes and settings saves can collide across the async runtime;
        // wait briefly instead of failing with SQLITE_BUSY.
        connection.execute_batch("PRAGMA busy_timeout = 5000;")?;
        // Spending and local-usage tables were removed with that feature; old databases keep
        // their rows forever unless dropped here.
        connection.execute_batch(
            "DROP TABLE IF EXISTS daily_usage;
             DROP TABLE IF EXISTS log_file_cache;
             PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS provider_snapshots (
               provider_id TEXT PRIMARY KEY,
               payload TEXT NOT NULL,
               refreshed_at TEXT NOT NULL,
               identity_key TEXT
             );
             CREATE TABLE IF NOT EXISTS app_settings (
               id INTEGER PRIMARY KEY CHECK (id = 1),
               payload TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS panel_state (
               id INTEGER PRIMARY KEY CHECK (id = 1),
               height INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS provider_environment (
               id INTEGER PRIMARY KEY CHECK (id = 1),
               payload TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS provider_account_records (
               provider_family TEXT NOT NULL,
               identity_key TEXT NOT NULL,
               provider_id TEXT NOT NULL,
               payload TEXT NOT NULL,
               PRIMARY KEY(provider_family, identity_key),
               UNIQUE(provider_family, provider_id)
             );",
        )?;
        if !Self::has_column(&connection, "provider_snapshots", "identity_key")? {
            connection.execute(
                "ALTER TABLE provider_snapshots ADD COLUMN identity_key TEXT",
                [],
            )?;
        }
        Ok(connection)
    }

    #[cfg(test)]
    pub fn load_snapshot(
        &self,
        provider_id: &str,
    ) -> Result<Option<ProviderSnapshot>, StorageError> {
        self.load_snapshot_for_identity(provider_id, CacheIdentity::Unscoped)
    }

    pub fn load_snapshot_for_identity(
        &self,
        provider_id: &str,
        identity: CacheIdentity<'_>,
    ) -> Result<Option<ProviderSnapshot>, StorageError> {
        let connection = self.connection()?;
        let cached: Option<(String, Option<String>)> = connection
            .query_row(
                "SELECT payload, identity_key FROM provider_snapshots WHERE provider_id = ?1",
                [provider_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(cached
            .filter(|(_, cached_identity)| match identity {
                CacheIdentity::Unscoped => cached_identity.is_none(),
                CacheIdentity::Resolved(expected) => cached_identity.as_deref() == Some(expected),
                CacheIdentity::Unresolved => true,
            })
            .and_then(|(json, _)| {
                // A corrupt cache row must never fail the provider: decode problems read as absent
                // so the next successful refresh heals the row instead of wedging the panel.
                match serde_json::from_str::<ProviderSnapshot>(&json) {
                    Ok(mut snapshot) => {
                        // Count quotas predate the unit field. The only count quota persisted by older
                        // releases was request-based, so normalize it once at the cache boundary instead of
                        // teaching every presentation surface to infer provider semantics.
                        for quota in &mut snapshot.quotas {
                            if quota.format == crate::models::QuotaFormat::Count
                                && quota.unit.is_none()
                            {
                                quota.unit = Some("requests".into());
                            }
                        }
                        Some(snapshot)
                    }
                    Err(error) => {
                        crate::app_warn!("cache", "cached snapshot payload is corrupt: {error}");
                        None
                    }
                }
            }))
    }

    #[cfg(test)]
    pub fn save_snapshot(&self, snapshot: &ProviderSnapshot) -> Result<(), StorageError> {
        self.save_snapshot_for_identity(snapshot, None)
    }

    pub fn save_snapshot_for_identity(
        &self,
        snapshot: &ProviderSnapshot,
        identity_key: Option<&str>,
    ) -> Result<(), StorageError> {
        let payload = serde_json::to_string(snapshot)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO provider_snapshots(provider_id, payload, refreshed_at, identity_key)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(provider_id) DO UPDATE SET
               payload = excluded.payload,
               refreshed_at = excluded.refreshed_at,
               identity_key = excluded.identity_key",
            params![
                snapshot.provider_id,
                payload,
                snapshot.refreshed_at.to_rfc3339(),
                identity_key,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn load_settings(&self) -> Result<Option<AppSettings>, StorageError> {
        let connection = self.connection()?;
        let payload: Option<String> = connection
            .query_row("SELECT payload FROM app_settings WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(json) = payload else {
            return Ok(None);
        };
        match serde_json::from_str(&json) {
            Ok(settings) => Ok(Some(settings)),
            Err(error) => {
                // A downgrade or a torn write must not brick startup; keep the raw
                // payload next to the database and fall back to defaults.
                crate::app_error!(
                    "settings",
                    "stored settings undecodable ({error}); backing up and falling back to defaults"
                );
                let backup = self.path.with_extension("db.settings-backup.json");
                if let Err(backup_error) = fs::write(&backup, &json) {
                    crate::app_warn!(
                        "settings",
                        "settings backup could not be written ({backup_error})"
                    );
                }
                connection.execute("DELETE FROM app_settings WHERE id = 1", [])?;
                Ok(None)
            }
        }
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), StorageError> {
        self.save_settings_with_account_updates(settings, &[])
    }

    pub fn save_settings_with_account_updates(
        &self,
        settings: &AppSettings,
        account_updates: &[ProviderAccountUpdate],
    ) -> Result<(), StorageError> {
        let payload = serde_json::to_string(settings)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        for update in account_updates {
            transaction.execute(
                "INSERT INTO provider_account_records(
                   provider_family, identity_key, provider_id, payload
                 ) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(provider_family, identity_key) DO UPDATE SET
                   provider_id = excluded.provider_id,
                   payload = excluded.payload",
                params![
                    update.provider_family,
                    update.identity_key,
                    update.provider_id,
                    update.payload,
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO app_settings(id, payload) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET payload = excluded.payload",
            [payload],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn load_provider_environment(
        &self,
    ) -> Result<Option<HashMap<String, String>>, StorageError> {
        let connection = self.connection()?;
        let payload: Option<String> = connection
            .query_row(
                "SELECT payload FROM provider_environment WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(payload.and_then(|json| match serde_json::from_str(&json) {
            Ok(environment) => Some(environment),
            Err(error) => {
                crate::app_warn!("cache", "stored environment payload is corrupt: {error}");
                None
            }
        }))
    }

    #[cfg(unix)]
    pub fn save_provider_environment(
        &self,
        environment: &HashMap<String, String>,
    ) -> Result<(), StorageError> {
        let payload = serde_json::to_string(environment)?;
        self.connection()?.execute(
            "INSERT INTO provider_environment(id, payload) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET payload = excluded.payload",
            [payload],
        )?;
        Ok(())
    }

    pub fn load_provider_account_records(
        &self,
        provider_family: &str,
    ) -> Result<Vec<(String, String, String)>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT identity_key, provider_id, payload
             FROM provider_account_records
             WHERE provider_family = ?1
             ORDER BY provider_id",
        )?;
        let records = statement
            .query_map([provider_family], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)?;
        Ok(records)
    }

    pub fn load_all_provider_account_records(
        &self,
    ) -> Result<Vec<(String, String, String, String)>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT provider_family, identity_key, provider_id, payload
             FROM provider_account_records
             ORDER BY provider_family, provider_id, identity_key",
        )?;
        let records = statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)?;
        Ok(records)
    }

    pub fn load_observed_account_provider_ids(&self) -> Result<Vec<String>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT DISTINCT provider_id
             FROM provider_account_records
             ORDER BY provider_id",
        )?;
        let provider_ids = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)?;
        Ok(provider_ids)
    }

    pub fn save_provider_account_record(
        &self,
        provider_family: &str,
        identity_key: &str,
        provider_id: &str,
        payload: &str,
    ) -> Result<(), StorageError> {
        self.connection()?.execute(
            "INSERT INTO provider_account_records(
               provider_family, identity_key, provider_id, payload
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(provider_family, identity_key) DO UPDATE SET
               provider_id = excluded.provider_id,
               payload = excluded.payload",
            params![provider_family, identity_key, provider_id, payload],
        )?;
        Ok(())
    }

    pub fn load_panel_height(&self) -> Result<Option<u32>, StorageError> {
        let connection = self.connection()?;
        let height = connection
            .query_row("SELECT height FROM panel_state WHERE id = 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?;
        Ok(height.and_then(|value| u32::try_from(value).ok()))
    }

    pub fn save_panel_height(&self, height: u32) -> Result<(), StorageError> {
        self.connection()?.execute(
            "INSERT INTO panel_state(id, height) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET height = excluded.height",
            [i64::from(height)],
        )?;
        Ok(())
    }

    pub fn clear_panel_height(&self) -> Result<(), StorageError> {
        self.connection()?
            .execute("DELETE FROM panel_state WHERE id = 1", [])?;
        Ok(())
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, StorageError> {
        self.connection.lock().map_err(|_| StorageError::Poisoned)
    }

    fn has_column(
        connection: &Connection,
        table: &str,
        column: &str,
    ) -> Result<bool, rusqlite::Error> {
        let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(columns.iter().any(|name| name == column))
    }
}

#[cfg(test)]
mod tests {

    use chrono::Utc;
    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::{ProviderAccountUpdate, Storage};
    use crate::models::{AppSettings, ProviderSnapshot, QuotaFormat, QuotaWindow};
    use crate::providers::CacheIdentity;

    #[test]
    fn undecodable_settings_fall_back_to_defaults_and_back_up_the_payload() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("quotagauge.db");
        let storage = Storage::open(&database).unwrap();
        {
            let connection = storage.connection().unwrap();
            connection
                .execute(
                    "INSERT INTO app_settings(id, payload) VALUES (1, '{\"theme\":\"future-variant\"}')",
                    [],
                )
                .unwrap();
        }

        let loaded = storage.load_settings().unwrap();

        assert!(loaded.is_none());
        let backup = std::fs::read_to_string(database.with_extension("db.settings-backup.json"))
            .expect("raw payload must be preserved for forensics");
        assert!(backup.contains("future-variant"));
        assert!(storage.load_settings().unwrap().is_none());
    }

    #[test]
    fn unopenable_database_is_quarantined_and_recreated() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("quotagauge.db");
        std::fs::write(&database, b"this is not a sqlite database").unwrap();

        let storage = Storage::open(&database).unwrap();

        assert!(database.exists());
        assert!(storage.load_settings().unwrap().is_none());
        let quarantined = std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("quotagauge.db.corrupt-"))
            .count();
        assert_eq!(quarantined, 1);
    }

    #[test]
    fn corrupt_cache_payloads_read_as_absent_instead_of_erroring() {
        let directory = tempdir().unwrap();
        let storage = Storage::open(&directory.path().join("quotagauge.db")).unwrap();
        {
            let connection = storage.connection().unwrap();
            connection
                .execute(
                    "INSERT INTO provider_snapshots(provider_id, payload, refreshed_at) \
                     VALUES ('codex', '{not json', '2026-10-27T00:00:00Z')",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO provider_environment(id, payload) VALUES (1, '{not json')",
                    [],
                )
                .unwrap();
        }
        assert!(storage.load_snapshot("codex").unwrap().is_none());
        assert!(storage.load_provider_environment().unwrap().is_none());
    }

    #[test]
    fn snapshot_round_trip_contains_no_credentials() {
        let directory = tempdir().unwrap();
        let storage = Storage::open(&directory.path().join("quotagauge.db")).unwrap();
        let snapshot = ProviderSnapshot {
            provider_id: "codex".into(),
            plan: Some("Plus".into()),
            quotas: Vec::new(),
            value_metrics: Vec::new(),
            status_metrics: Vec::new(),
            notices: Vec::new(),
            warnings: Vec::new(),
            refreshed_at: Utc::now(),
        };

        storage.save_snapshot(&snapshot).unwrap();

        assert_eq!(storage.load_snapshot("codex").unwrap(), Some(snapshot));
        let bytes = std::fs::read(directory.path().join("quotagauge.db")).unwrap();
        let database = String::from_utf8_lossy(&bytes);
        assert!(!database.contains("access_token"));
        assert!(!database.contains("refresh_token"));
    }

    #[test]
    fn account_scoped_snapshot_is_visible_only_to_the_same_identity() {
        let directory = tempdir().unwrap();
        let storage = Storage::open(&directory.path().join("quotagauge.db")).unwrap();
        let snapshot = ProviderSnapshot {
            provider_id: "claude".into(),
            plan: Some("Max".into()),
            quotas: Vec::new(),
            value_metrics: Vec::new(),
            status_metrics: Vec::new(),
            notices: Vec::new(),
            warnings: Vec::new(),
            refreshed_at: Utc::now(),
        };

        storage
            .save_snapshot_for_identity(&snapshot, Some("identity-a"))
            .unwrap();

        assert_eq!(
            storage
                .load_snapshot_for_identity("claude", CacheIdentity::Resolved("identity-a"))
                .unwrap(),
            Some(snapshot.clone())
        );
        assert!(storage
            .load_snapshot_for_identity("claude", CacheIdentity::Resolved("identity-b"))
            .unwrap()
            .is_none());
        assert!(storage.load_snapshot("claude").unwrap().is_none());
        assert_eq!(
            storage
                .load_snapshot_for_identity("claude", CacheIdentity::Unresolved)
                .unwrap(),
            Some(snapshot)
        );
    }

    #[test]
    fn provider_account_records_round_trip_by_family() {
        let directory = tempdir().unwrap();
        let storage = Storage::open(&directory.path().join("quotagauge.db")).unwrap();

        storage
            .save_provider_account_record(
                "claude",
                "identity-a",
                "claude@12345678",
                r#"{"displayName":"Claude — Work"}"#,
            )
            .unwrap();

        assert_eq!(
            storage.load_provider_account_records("claude").unwrap(),
            [(
                "identity-a".to_owned(),
                "claude@12345678".to_owned(),
                r#"{"displayName":"Claude — Work"}"#.to_owned(),
            )]
        );
        assert!(storage
            .load_provider_account_records("codex")
            .unwrap()
            .is_empty());
        assert_eq!(
            storage.load_observed_account_provider_ids().unwrap(),
            ["claude@12345678"]
        );
    }

    #[test]
    fn account_records_and_settings_roll_back_together() {
        let directory = tempdir().unwrap();
        let storage = Storage::open(&directory.path().join("quotagauge.db")).unwrap();
        let original = AppSettings {
            theme: crate::models::ThemePreference::Light,
            ..AppSettings::default()
        };
        storage.save_settings(&original).unwrap();
        storage
            .save_provider_account_record("codex", "identity-a", "codex", r#"{"name":"old"}"#)
            .unwrap();
        let changed = AppSettings {
            theme: crate::models::ThemePreference::Dark,
            ..original.clone()
        };
        let updates = [
            ProviderAccountUpdate {
                provider_family: "codex".into(),
                identity_key: "identity-a".into(),
                provider_id: "codex".into(),
                payload: r#"{"name":"changed"}"#.into(),
            },
            ProviderAccountUpdate {
                provider_family: "codex".into(),
                identity_key: "identity-b".into(),
                provider_id: "codex".into(),
                payload: "{}".into(),
            },
        ];

        assert!(storage
            .save_settings_with_account_updates(&changed, &updates)
            .is_err());
        assert_eq!(storage.load_settings().unwrap(), Some(original));
        let records = storage.load_provider_account_records("codex").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].2, r#"{"name":"old"}"#);
    }

    #[test]
    fn legacy_snapshot_table_gains_the_account_identity_column() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("quotagauge.db");
        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE provider_snapshots (
                   provider_id TEXT PRIMARY KEY,
                   payload TEXT NOT NULL,
                   refreshed_at TEXT NOT NULL
                 );",
            )
            .unwrap();
        drop(legacy);

        let storage = Storage::open(&path).unwrap();
        let connection = storage.connection().unwrap();

        assert!(Storage::has_column(&connection, "provider_snapshots", "identity_key").unwrap());
    }

    #[test]
    fn legacy_count_quota_cache_is_normalized_at_load_boundary() {
        let directory = tempdir().unwrap();
        let storage = Storage::open(&directory.path().join("quotagauge.db")).unwrap();
        let snapshot = ProviderSnapshot {
            provider_id: "cursor".into(),
            plan: None,
            quotas: vec![QuotaWindow {
                id: "requests".into(),
                label: "Requests".into(),
                used_percent: 25.0,
                resets_at: None,
                period_seconds: 2_592_000,
                format: QuotaFormat::Count,
                used_value: Some(25.0),
                limit_value: Some(100.0),
                unit: None,
                estimated: false,
                source_note: None,
            }],
            value_metrics: Vec::new(),
            status_metrics: Vec::new(),
            notices: Vec::new(),
            warnings: Vec::new(),
            refreshed_at: Utc::now(),
        };

        storage.save_snapshot(&snapshot).unwrap();

        assert_eq!(
            storage.load_snapshot("cursor").unwrap().unwrap().quotas[0].unit,
            Some("requests".into())
        );
    }

    #[test]
    fn settings_round_trip_uses_the_same_disk_database() {
        let directory = tempdir().unwrap();
        let storage = Storage::open(&directory.path().join("quotagauge.db")).unwrap();
        let settings = AppSettings {
            always_show_pacing: true,
            ..AppSettings::default()
        };
        storage.save_settings(&settings).unwrap();
        assert_eq!(storage.load_settings().unwrap(), Some(settings));
    }

    #[test]
    fn panel_height_round_trip_is_independent_from_app_settings() {
        let directory = tempdir().unwrap();
        let storage = Storage::open(&directory.path().join("quotagauge.db")).unwrap();
        let settings = AppSettings::default();
        storage.save_settings(&settings).unwrap();

        assert_eq!(storage.load_panel_height().unwrap(), None);
        storage.save_panel_height(734).unwrap();

        assert_eq!(storage.load_panel_height().unwrap(), Some(734));
        assert_eq!(storage.load_settings().unwrap(), Some(settings));

        storage.clear_panel_height().unwrap();
        assert_eq!(storage.load_panel_height().unwrap(), None);
    }
}
