#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("chunk not found: {0}")]
    ChunkNotFound(String),

    #[error("embedding error: {0}")]
    Embedding(String),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
}
