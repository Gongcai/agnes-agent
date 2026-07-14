use serde::Serialize;
use thiserror::Error;

/// 应用统一错误。实现 `Serialize` 以便直接作为 Tauri 命令错误返回给前端。
#[derive(Debug, Error)]
pub enum AppError {
    #[error("db error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("agent error: {0}")]
    Agent(String),
    #[error("ws error: {0}")]
    Ws(String),
    #[error("{0}")]
    Other(String),
}

impl Serialize for AppError {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        s.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
