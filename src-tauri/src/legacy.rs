//! One-time import of data written by the pre-rename build (OpenQuota).
//!
//! The app data directory is keyed by the bundle identifier, so the new
//! identifier starts with an empty directory. The legacy database lives in a
//! sibling directory named after the legacy identifier.

use std::fs;
use std::path::{Path, PathBuf};

pub const LEGACY_APP_IDENTIFIER: &str = "io.github.deviffyy.openquota";
pub const LEGACY_DB_FILENAME: &str = "openquota.db";

/// Outcome of a legacy database import attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum LegacyDbImport {
    /// New database already existed; nothing was touched.
    NotNeeded,
    /// No legacy installation found.
    LegacyMissing,
    /// Legacy database copied to the new location.
    Imported { bytes: u64 },
}

fn legacy_database_path(app_data_dir: &Path) -> Option<PathBuf> {
    let data_root = app_data_dir.parent()?;
    Some(
        data_root
            .join(LEGACY_APP_IDENTIFIER)
            .join(LEGACY_DB_FILENAME),
    )
}

/// Copies the legacy database into place when the new one does not exist yet.
///
/// The database schema is produced by the same code line, so a file copy is a
/// complete import. Failures are returned so the caller can log and continue
/// with a fresh database.
pub fn import_legacy_database(
    app_data_dir: &Path,
    database_path: &Path,
) -> Result<LegacyDbImport, String> {
    if database_path.exists() {
        return Ok(LegacyDbImport::NotNeeded);
    }
    let Some(legacy_path) = legacy_database_path(app_data_dir) else {
        return Ok(LegacyDbImport::LegacyMissing);
    };
    if !legacy_path.is_file() {
        return Ok(LegacyDbImport::LegacyMissing);
    }
    let bytes = copy_with_wal_sidecars(&legacy_path, database_path)
        .map_err(|error| format!("copying {} failed: {error}", legacy_path.display()))?;
    Ok(LegacyDbImport::Imported { bytes })
}

/// The source database runs in WAL mode, so the -wal/-shm sidecar files are part of
/// the database state; copying the main file alone imports a torn, half-checkpointed
/// database. The main file goes through a temp name so a failed import never leaves a
/// partial quotagauge.db behind (its existence would suppress any retry).
fn copy_with_wal_sidecars(source: &Path, target: &Path) -> std::io::Result<u64> {
    let temporary = target.with_extension("db.import-tmp");
    let bytes = fs::copy(source, &temporary)?;
    let mut sidecars = Vec::new();
    for sidecar in ["-wal", "-shm"] {
        let source_sidecar = appended_path(source, sidecar);
        if source_sidecar.is_file() {
            let temporary_sidecar = appended_path(&temporary, sidecar);
            fs::copy(&source_sidecar, &temporary_sidecar)?;
            sidecars.push((temporary_sidecar, appended_path(target, sidecar)));
        }
    }
    match fs::rename(&temporary, target) {
        Ok(()) => {
            for (from, to) in sidecars {
                let _ = fs::rename(from, to);
            }
            Ok(bytes)
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            for (from, _) in sidecars {
                let _ = fs::remove_file(from);
            }
            Err(error)
        }
    }
}

fn appended_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    let mut joined = file_name;
    joined.push(suffix);
    path.with_file_name(joined)
}

/// Runs the legacy import and records the outcome; startup continues with a
/// fresh database when the import fails.
pub fn import_and_log(app_data_dir: &Path, database_path: &Path) {
    match import_legacy_database(app_data_dir, database_path) {
        Ok(LegacyDbImport::Imported { bytes }) => {
            crate::app_info!(
                "lifecycle",
                "imported legacy OpenQuota database ({} bytes)",
                bytes
            );
        }
        Ok(_) => {}
        Err(error) => crate::app_warn!("lifecycle", "legacy database import skipped ({error})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_copy_failure_when_target_directory_is_missing() {
        let base = tempfile::tempdir().unwrap();
        let legacy_dir = new_dir(base.path(), LEGACY_APP_IDENTIFIER);
        let new_dir = new_dir(base.path(), "xyz.actitud.quotagauge");

        fs::write(legacy_dir.join(LEGACY_DB_FILENAME), b"legacy").unwrap();

        let outcome =
            import_legacy_database(&new_dir, &new_dir.join("missing").join("quotagauge.db"));

        assert!(outcome.is_err());
    }

    #[test]
    fn import_carries_wal_sidecars_for_a_consistent_database() {
        let base = tempfile::tempdir().unwrap();
        let source = base.path().join("openquota.db");
        let target = base.path().join("quotagauge.db");
        fs::write(&source, b"main").unwrap();
        fs::write(appended_path(&source, "-wal"), b"wal").unwrap();
        fs::write(appended_path(&source, "-shm"), b"shm").unwrap();

        let bytes = copy_with_wal_sidecars(&source, &target).unwrap();

        assert_eq!(bytes, 4);
        assert_eq!(fs::read_to_string(&target).unwrap(), "main");
        assert_eq!(
            fs::read_to_string(appended_path(&target, "-wal")).unwrap(),
            "wal"
        );
        assert_eq!(
            fs::read_to_string(appended_path(&target, "-shm")).unwrap(),
            "shm"
        );
        assert!(!target.with_extension("db.import-tmp").exists());
    }

    fn new_dir(base: &Path, identifier: &str) -> PathBuf {
        let dir = base.join(identifier);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn imports_legacy_database_when_new_one_is_absent() {
        let base = tempfile::tempdir().unwrap();
        let legacy_dir = new_dir(base.path(), LEGACY_APP_IDENTIFIER);
        let new_dir = new_dir(base.path(), "xyz.actitud.quotagauge");

        let legacy_db = legacy_dir.join(LEGACY_DB_FILENAME);
        fs::write(&legacy_db, b"sqlite-payload").unwrap();

        let database_path = new_dir.join("quotagauge.db");
        let outcome = import_legacy_database(&new_dir, &database_path).unwrap();

        assert_eq!(outcome, LegacyDbImport::Imported { bytes: 14 });
        assert_eq!(fs::read(&database_path).unwrap(), b"sqlite-payload");
        // The legacy file stays in place for the old app.
        assert!(legacy_db.is_file());
    }

    #[test]
    fn keeps_existing_new_database_untouched() {
        let base = tempfile::tempdir().unwrap();
        let legacy_dir = new_dir(base.path(), LEGACY_APP_IDENTIFIER);
        let new_dir = new_dir(base.path(), "xyz.actitud.quotagauge");

        fs::write(legacy_dir.join(LEGACY_DB_FILENAME), b"legacy").unwrap();
        let database_path = new_dir.join("quotagauge.db");
        fs::write(&database_path, b"current").unwrap();

        let outcome = import_legacy_database(&new_dir, &database_path).unwrap();

        assert_eq!(outcome, LegacyDbImport::NotNeeded);
        assert_eq!(fs::read(&database_path).unwrap(), b"current");
    }

    #[test]
    fn reports_missing_legacy_install_as_no_op() {
        let base = tempfile::tempdir().unwrap();
        let new_dir = new_dir(base.path(), "xyz.actitud.quotagauge");
        let database_path = new_dir.join("quotagauge.db");

        let outcome = import_legacy_database(&new_dir, &database_path).unwrap();

        assert_eq!(outcome, LegacyDbImport::LegacyMissing);
        assert!(!database_path.exists());
    }

    #[test]
    fn ignores_legacy_entry_that_is_not_a_file() {
        let base = tempfile::tempdir().unwrap();
        let legacy_dir = new_dir(base.path(), LEGACY_APP_IDENTIFIER);
        let new_dir = new_dir(base.path(), "xyz.actitud.quotagauge");

        fs::create_dir(legacy_dir.join(LEGACY_DB_FILENAME)).unwrap();

        let outcome = import_legacy_database(&new_dir, &new_dir.join("quotagauge.db")).unwrap();

        assert_eq!(outcome, LegacyDbImport::LegacyMissing);
    }
}
