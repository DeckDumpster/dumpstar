#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("payload is not a catalog: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("source: {0}")]
    Source(String),
    #[error("unsupported payload format {0:?}")]
    Format(String),
}

pub type Result<T> = std::result::Result<T, CatalogError>;
