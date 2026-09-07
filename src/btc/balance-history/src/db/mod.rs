mod address;
#[allow(clippy::module_inception)]
mod db;
mod helper;
mod snapshot;
mod split_snapshot;

pub use address::{AddressDB, AddressDBRef};
pub use db::*;
pub use snapshot::*;
pub use split_snapshot::*;

#[cfg(test)]
mod security_tests {
    /// Reject FTS5 even when an imported database tries to introduce it.
    #[test]
    fn bundled_sqlite_excludes_unused_fts5() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        let enabled: bool = db
            .query_row(
                "SELECT sqlite_compileoption_used('ENABLE_FTS5')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!enabled, "Release SQLite must be built without FTS5");
        let error = db
            .execute_batch("CREATE VIRTUAL TABLE untrusted_search USING fts5(content)")
            .unwrap_err();
        assert!(error.to_string().contains("no such module: fts5"));
    }
}
