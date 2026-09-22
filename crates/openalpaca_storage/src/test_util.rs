use crate::database::Database;

pub(crate) fn test_db() -> Database {
    Database::open(std::path::Path::new(":memory:")).unwrap()
}
