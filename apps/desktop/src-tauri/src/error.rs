use thiserror::Error;

#[derive(Debug, Error)]
pub enum NovaError {
    #[error("配置错误：{0}")]
    Config(String),
    #[error("凭据错误：{0}")]
    Credential(String),
    #[error("数据库错误：{0}")]
    Database(String),
    #[error("网络错误：{0}")]
    Network(String),
    #[error("接口错误：{0}")]
    Api(String),
}

impl NovaError {
    pub fn public_message(&self) -> String {
        match self {
            Self::Config(message) => format!("配置错误：{message}"),
            Self::Credential(message) => format!("凭据错误：{message}"),
            Self::Database(_) => "本地数据读写失败，请稍后重试".to_string(),
            Self::Network(message) => format!("网络错误：{message}"),
            Self::Api(message) => message.clone(),
        }
    }
}

impl From<rusqlite::Error> for NovaError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value.to_string())
    }
}

impl From<std::io::Error> for NovaError {
    fn from(value: std::io::Error) -> Self {
        Self::Database(value.to_string())
    }
}

impl From<crate::storage::migrations::MigrationError> for NovaError {
    fn from(value: crate::storage::migrations::MigrationError) -> Self {
        Self::Database(value.to_string())
    }
}

pub type NovaResult<T> = Result<T, NovaError>;

pub fn public_error(error: NovaError) -> String {
    error.public_message()
}
