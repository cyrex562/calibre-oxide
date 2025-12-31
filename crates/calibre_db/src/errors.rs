use thiserror::Error;

#[derive(Error, Debug)]
pub enum DBError {
    #[error("No book with id: {0} in database")]
    NoSuchBook(i32),
    #[error("No such format: {0}")]
    NoSuchFormat(String),
}
