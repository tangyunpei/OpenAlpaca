use crate::database::Database;
use tempfile::tempdir;

pub(crate) fn test_db() -> Database {
    let dir = tempdir().unwrap();
    Database::open(&dir.path().join("test.db")).unwrap()
}
