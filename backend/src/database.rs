use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

use crate::domain::TrackerError;

pub async fn connect_database(path: &Path) -> Result<SqlitePool, TrackerError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| TrackerError::Internal(format!("无法创建数据库目录: {error}")))?;
    }

    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    sqlx::raw_sql(include_str!("../assets/schema.sql"))
        .execute(&pool)
        .await
        .map_err(|error| TrackerError::Internal(format!("数据库 schema 初始化失败: {error}")))?;
    Ok(pool)
}

#[derive(Debug)]
pub struct DatabaseLock {
    file: File,
    path: PathBuf,
}

impl DatabaseLock {
    pub fn acquire(database_path: &Path) -> Result<Self, TrackerError> {
        if let Some(parent) = database_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .map_err(|error| TrackerError::Internal(format!("无法创建数据库目录: {error}")))?;
        }
        let path = lock_path(database_path);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| TrackerError::Internal(format!("无法打开数据库锁: {error}")))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file, path }),
            Err(std::fs::TryLockError::WouldBlock) => Err(TrackerError::DatabaseBusy),
            Err(std::fs::TryLockError::Error(error)) => {
                Err(TrackerError::Internal(format!("无法取得数据库锁: {error}")))
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DatabaseLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn lock_path(database_path: &Path) -> PathBuf {
    let mut value = database_path.as_os_str().to_os_string();
    value.push(".lock");
    PathBuf::from(value)
}

pub async fn database_health(pool: &SqlitePool) -> Result<(), TrackerError> {
    sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::Row;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn database_lock_rejects_a_second_owner_and_releases_on_drop() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("tracker.sqlite3");
        let first = DatabaseLock::acquire(&database).unwrap();
        assert!(matches!(
            DatabaseLock::acquire(&database),
            Err(TrackerError::DatabaseBusy)
        ));
        drop(first);
        DatabaseLock::acquire(&database).unwrap();
    }

    #[tokio::test]
    async fn database_enables_wal_foreign_keys_and_busy_timeout() {
        let directory = tempdir().unwrap();
        let pool = connect_database(&directory.path().join("tracker.sqlite3"))
            .await
            .unwrap();

        let journal_mode: String = sqlx::query("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        let foreign_keys: i64 = sqlx::query("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        let busy_timeout: i64 = sqlx::query("PRAGMA busy_timeout")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);

        assert_eq!(journal_mode, "wal");
        assert_eq!(foreign_keys, 1);
        assert!(busy_timeout >= 5_000);

        let sqlx_table_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let compatibility_column_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('events') \
             WHERE name IN ('metadata_json', 'source_kind', 'migration_run_id', 'migration_row_number')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(sqlx_table_count, 0);
        assert_eq!(compatibility_column_count, 0);
    }
}
