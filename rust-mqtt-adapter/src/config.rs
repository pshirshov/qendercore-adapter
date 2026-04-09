use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

use crate::error::{AppError, AppResult};

const DEFAULT_MQTT_PORT: u16 = 1883;
const DEFAULT_INTERVAL_SECONDS: u64 = 60;
const DEFAULT_HTTP_TIMEOUT_MILLIS: u64 = 15_000;
const DEFAULT_RECONNECT_INITIAL_DELAY_MILLIS: u64 = 2_000;
const DEFAULT_RECONNECT_MAX_DELAY_MILLIS: u64 = 60_000;
const DEFAULT_STATS_INTERVAL_MILLIS: u64 = 300_000;
const DEFAULT_DISCOVERY_PREFIX: &str = "homeassistant";
const DEFAULT_TOPIC_PREFIX: &str = "qendercore";
const DEFAULT_CLIENT_ID: &str = "qendercore-rs-mqtt-adapter";
const DEFAULT_CACHE_DIR: &str = ".cache";
const DEFAULT_API_URL: &str = "https://auth.qendercore.com:8000/v1";

#[derive(Debug, Parser)]
#[command(about = "Standalone Rust MQTT adapter for the Qendercore inverter cloud")]
pub struct CliArgs {
    #[arg(long, help = "Qendercore account login (email)")]
    pub qc_login: String,
    #[arg(long, help = "Qendercore account password")]
    pub qc_password: Option<String>,
    #[arg(long, help = "Qendercore password file")]
    pub qc_password_file: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_API_URL, help = "Qendercore API base URL")]
    pub qc_api_url: String,
    #[arg(long, default_value = DEFAULT_CACHE_DIR, help = "Directory used to cache the auth token")]
    pub cache_dir: PathBuf,
    #[arg(long, default_value_t = DEFAULT_HTTP_TIMEOUT_MILLIS, help = "HTTP request timeout in milliseconds")]
    pub http_timeout_millis: u64,

    #[arg(long, help = "MQTT broker host")]
    pub mqtt_host: String,
    #[arg(long, default_value_t = DEFAULT_MQTT_PORT, help = "MQTT broker port")]
    pub mqtt_port: u16,
    #[arg(long, help = "MQTT username")]
    pub mqtt_user: Option<String>,
    #[arg(long, help = "MQTT password")]
    pub mqtt_password: Option<String>,
    #[arg(long, help = "MQTT password file")]
    pub mqtt_password_file: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_DISCOVERY_PREFIX, help = "Home Assistant MQTT discovery prefix")]
    pub discovery_prefix: String,
    #[arg(long, default_value = DEFAULT_TOPIC_PREFIX, help = "Base topic prefix for state, command and availability topics")]
    pub topic_prefix: String,
    #[arg(long, default_value = DEFAULT_CLIENT_ID, help = "MQTT client id")]
    pub client_id: String,

    #[arg(long, default_value_t = DEFAULT_INTERVAL_SECONDS, help = "Polling interval in seconds")]
    pub interval_seconds: u64,
    #[arg(long, default_value_t = DEFAULT_RECONNECT_INITIAL_DELAY_MILLIS, help = "Initial reconnect delay in milliseconds")]
    pub reconnect_initial_delay_millis: u64,
    #[arg(long, default_value_t = DEFAULT_RECONNECT_MAX_DELAY_MILLIS, help = "Maximum reconnect delay in milliseconds")]
    pub reconnect_max_delay_millis: u64,
    #[arg(long, default_value_t = DEFAULT_STATS_INTERVAL_MILLIS, help = "Periodic liveness stats interval in milliseconds")]
    pub stats_interval_millis: u64,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub qcore: QcoreConfig,
    pub mqtt: MqttConfig,
    pub polling: PollingConfig,
    pub reconnect: ReconnectConfig,
    pub stats: StatsConfig,
}

#[derive(Debug, Clone)]
pub struct QcoreConfig {
    pub api_url: String,
    pub login: String,
    pub password: String,
    pub cache_dir: PathBuf,
    pub http_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct MqttConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub discovery_prefix: String,
    pub topic_prefix: String,
    pub client_id: String,
}

#[derive(Debug, Clone)]
pub struct PollingConfig {
    pub interval: Duration,
}

#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    pub initial_delay: Duration,
    pub max_delay: Duration,
}

#[derive(Debug, Clone)]
pub struct StatsConfig {
    pub interval: Duration,
}

impl CliArgs {
    pub fn into_config(self) -> AppResult<AppConfig> {
        if self.qc_login.trim().is_empty() {
            return Err(AppError::InvalidConfig(
                "qc_login cannot be empty".to_string(),
            ));
        }
        if self.qc_api_url.trim().is_empty() {
            return Err(AppError::InvalidConfig(
                "qc_api_url cannot be empty".to_string(),
            ));
        }
        if self.mqtt_host.trim().is_empty() {
            return Err(AppError::InvalidConfig(
                "mqtt_host cannot be empty".to_string(),
            ));
        }
        if self.http_timeout_millis == 0 {
            return Err(AppError::InvalidConfig(
                "http_timeout_millis must be greater than zero".to_string(),
            ));
        }
        if self.interval_seconds == 0 {
            return Err(AppError::InvalidConfig(
                "interval_seconds must be greater than zero".to_string(),
            ));
        }
        if self.reconnect_initial_delay_millis == 0 {
            return Err(AppError::InvalidConfig(
                "reconnect_initial_delay_millis must be greater than zero".to_string(),
            ));
        }
        if self.reconnect_max_delay_millis == 0 {
            return Err(AppError::InvalidConfig(
                "reconnect_max_delay_millis must be greater than zero".to_string(),
            ));
        }
        if self.reconnect_initial_delay_millis > self.reconnect_max_delay_millis {
            return Err(AppError::InvalidConfig(format!(
                "reconnect initial delay {} cannot be greater than reconnect max delay {}",
                self.reconnect_initial_delay_millis, self.reconnect_max_delay_millis
            )));
        }
        if self.stats_interval_millis == 0 {
            return Err(AppError::InvalidConfig(
                "stats_interval_millis must be greater than zero".to_string(),
            ));
        }

        let qc_password = load_secret("qc_password", self.qc_password, self.qc_password_file)?
            .ok_or_else(|| {
                AppError::InvalidConfig(
                    "either --qc-password or --qc-password-file must be provided".to_string(),
                )
            })?;

        let mqtt_password = load_secret("mqtt_password", self.mqtt_password, self.mqtt_password_file)?;
        if mqtt_password.is_some() && self.mqtt_user.is_none() {
            return Err(AppError::InvalidConfig(
                "mqtt password requires --mqtt-user".to_string(),
            ));
        }

        Ok(AppConfig {
            qcore: QcoreConfig {
                api_url: self.qc_api_url.trim().trim_end_matches('/').to_string(),
                login: self.qc_login.trim().to_string(),
                password: qc_password,
                cache_dir: self.cache_dir,
                http_timeout: Duration::from_millis(self.http_timeout_millis),
            },
            mqtt: MqttConfig {
                host: self.mqtt_host.trim().to_string(),
                port: self.mqtt_port,
                username: self.mqtt_user.map(|value| value.trim().to_string()),
                password: mqtt_password,
                discovery_prefix: normalize_topic_segment(&self.discovery_prefix, "discovery prefix")?,
                topic_prefix: normalize_topic_segment(&self.topic_prefix, "topic prefix")?,
                client_id: self.client_id.trim().to_string(),
            },
            polling: PollingConfig {
                interval: Duration::from_secs(self.interval_seconds),
            },
            reconnect: ReconnectConfig {
                initial_delay: Duration::from_millis(self.reconnect_initial_delay_millis),
                max_delay: Duration::from_millis(self.reconnect_max_delay_millis),
            },
            stats: StatsConfig {
                interval: Duration::from_millis(self.stats_interval_millis),
            },
        })
    }
}

fn normalize_topic_segment(value: &str, field_name: &str) -> AppResult<String> {
    let normalized = value.trim().trim_matches('/').to_string();
    if normalized.is_empty() {
        return Err(AppError::InvalidConfig(format!(
            "{field_name} cannot be empty"
        )));
    }
    Ok(normalized)
}

fn load_secret(
    field_name: &str,
    inline: Option<String>,
    file: Option<PathBuf>,
) -> AppResult<Option<String>> {
    match (inline, file) {
        (Some(_), Some(_)) => Err(AppError::InvalidConfig(format!(
            "use either --{field}/--{field}-file but not both",
            field = field_name.replace('_', "-")
        ))),
        (Some(value), None) => Ok(Some(value)),
        (None, Some(path)) => {
            let value = fs::read_to_string(&path)?;
            let value = value.trim().to_string();
            if value.is_empty() {
                return Err(AppError::InvalidConfig(format!(
                    "{field_name} file {} was empty",
                    path.display()
                )));
            }
            Ok(Some(value))
        }
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::CliArgs;

    fn base_args() -> CliArgs {
        CliArgs {
            qc_login: "user@example.com".to_string(),
            qc_password: Some("secret".to_string()),
            qc_password_file: None,
            qc_api_url: super::DEFAULT_API_URL.to_string(),
            cache_dir: ".cache".into(),
            http_timeout_millis: 15_000,
            mqtt_host: "mqtt.local".to_string(),
            mqtt_port: 1883,
            mqtt_user: None,
            mqtt_password: None,
            mqtt_password_file: None,
            discovery_prefix: "homeassistant".to_string(),
            topic_prefix: "qendercore".to_string(),
            client_id: "qendercore-rs-mqtt-adapter".to_string(),
            interval_seconds: 60,
            reconnect_initial_delay_millis: 2_000,
            reconnect_max_delay_millis: 60_000,
            stats_interval_millis: 300_000,
        }
    }

    #[test]
    fn into_config_rejects_reconnect_initial_above_max() {
        let mut args = base_args();
        args.reconnect_initial_delay_millis = 90_000;
        args.reconnect_max_delay_millis = 30_000;

        let error = args.into_config().unwrap_err();
        assert!(error.to_string().contains(
            "reconnect initial delay 90000 cannot be greater than reconnect max delay 30000"
        ));
    }

    #[test]
    fn into_config_rejects_missing_qc_password() {
        let mut args = base_args();
        args.qc_password = None;
        let error = args.into_config().unwrap_err();
        assert!(error.to_string().contains("qc-password"));
    }

    #[test]
    fn into_config_rejects_password_without_user() {
        let mut args = base_args();
        args.mqtt_password = Some("hunter2".to_string());
        let error = args.into_config().unwrap_err();
        assert!(error.to_string().contains("mqtt password requires --mqtt-user"));
    }
}
