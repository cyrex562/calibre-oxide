use thiserror::Error;

#[derive(Error, Debug)]
pub enum DBError {
    #[error("No book with id: {0} in database")]
    NoSuchBook(i32),
    #[error("No such format: {0}")]
    NoSuchFormat(String),
}

/// Port of `old_src/src/calibre/db/__init__.py`'s `FTSQueryError`
/// (issue #218): raised when an FTS5 query fails to parse. Not wired
/// into anything yet -- this crate has no FTS subsystem to raise it
/// from (tracked separately, issue #226); ported now as a real type
/// so it's ready when that lands, rather than invented ad hoc then.
#[derive(Error, Debug)]
#[error("Failed to parse search query: {query} with error: {apsw_error}")]
pub struct FtsQueryError {
    pub query: String,
    pub sql_statement: String,
    pub apsw_error: String,
}
