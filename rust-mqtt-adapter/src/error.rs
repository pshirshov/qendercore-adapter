use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
    #[error("mqtt disconnected: {0}")]
    MqttDisconnected(String),
    #[error("qendercore api error: {0}")]
    QcoreApi(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("mqtt client error: {0}")]
    MqttClient(#[from] rumqttc::ClientError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}
