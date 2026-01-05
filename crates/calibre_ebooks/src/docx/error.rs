use thiserror::Error;

#[derive(Error, Debug)]
pub enum DocxError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Zip Error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("XML Parse Error: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("Invalid DOCX: {0}")]
    InvalidDocx(String),
}
