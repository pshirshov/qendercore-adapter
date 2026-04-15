use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use log::{error, info, warn};

use qendercore_mqtt_adapter::config::{AppConfig, CliArgs};
use qendercore_mqtt_adapter::error::{AppError, AppResult};
use qendercore_mqtt_adapter::mqtt::{CommandMessage, MqttPublisher};
use qendercore_mqtt_adapter::qcore::QcoreClient;
use qendercore_mqtt_adapter::stats::RuntimeStats;

fn main() -> Result<(), AppError> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .format_timestamp_secs()
    .init();
    let config = CliArgs::parse().into_config()?;
    run(config)
}

fn run(config: AppConfig) -> AppResult<()> {
    let stats = RuntimeStats::new_shared();
    stats.spawn_reporter(config.stats.interval);

    let mut backoff =
        ReconnectBackoff::new(config.reconnect.initial_delay, config.reconnect.max_delay);

    loop {
        match run_session(&config, &stats) {
            Ok(()) => {
                backoff.reset();
                return Ok(());
            }
            Err(error) => {
                stats.record_recovery();
                let delay = backoff.next_delay();
                error!("session failed: {error}; reconnecting in {delay:?}");
                thread::sleep(delay);
            }
        }
    }
}

fn run_session(config: &AppConfig, stats: &Arc<RuntimeStats>) -> AppResult<()> {
    let qcore = QcoreClient::new(config.qcore.clone())?;
    let (publisher, command_rx) = MqttPublisher::connect(&config.mqtt, Arc::clone(stats))?;
    let result = run_session_inner(config, &qcore, &publisher, &command_rx, stats);
    if result.is_err() {
        publisher.publish_offline_best_effort();
    }
    result
}

fn run_session_inner(
    config: &AppConfig,
    qcore: &Arc<QcoreClient>,
    publisher: &MqttPublisher,
    command_rx: &Receiver<CommandMessage>,
    stats: &Arc<RuntimeStats>,
) -> AppResult<()> {
    publisher.publish_discovery()?;
    info!(
        "published HA discovery; entering polling loop (interval={:?})",
        config.polling.interval
    );

    loop {
        publisher.ensure_healthy()?;

        match qcore.fetch_qc_data() {
            Ok(data) => {
                publisher.publish_qc_data(&data)?;
                stats.record_successful_poll();
            }
            Err(error) => {
                warn!("failed to fetch qc data: {error}");
                stats.record_failed_poll();
            }
        }

        match qcore.fetch_daily_schedule() {
            Ok(schedule) => {
                publisher.publish_schedule(&schedule)?;
                stats.record_schedule_fetch();
            }
            Err(error) => {
                warn!("failed to fetch daily schedule: {error}");
            }
        }

        wait_or_handle_commands(
            config.polling.interval,
            command_rx,
            qcore,
            publisher,
            stats,
        )?;
    }
}

/// Sleeps up to `interval`, processing any inbound MQTT commands as they
/// arrive. Each command immediately mutates the cloud state and republishes
/// the schedule so the HA UI reflects the change.
fn wait_or_handle_commands(
    interval: Duration,
    command_rx: &Receiver<CommandMessage>,
    qcore: &Arc<QcoreClient>,
    publisher: &MqttPublisher,
    stats: &Arc<RuntimeStats>,
) -> AppResult<()> {
    let deadline = Instant::now() + interval;
    loop {
        publisher.ensure_healthy()?;
        let now = Instant::now();
        if now >= deadline {
            return Ok(());
        }
        let remaining = deadline - now;
        match command_rx.recv_timeout(remaining) {
            Ok(CommandMessage::HomeAssistantOnline) => {
                if let Err(error) = handle_ha_online(qcore, publisher, stats) {
                    warn!("failed to handle HA online event: {error}");
                }
            }
            Ok(command) => {
                stats.record_command_received();
                if let Err(error) = handle_command(command, qcore, publisher, stats) {
                    warn!("failed to apply command: {error}");
                }
            }
            Err(RecvTimeoutError::Timeout) => return Ok(()),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(AppError::MqttDisconnected(
                    "command channel closed".to_string(),
                ));
            }
        }
    }
}

fn handle_ha_online(
    qcore: &Arc<QcoreClient>,
    publisher: &MqttPublisher,
    stats: &Arc<RuntimeStats>,
) -> AppResult<()> {
    info!("home assistant came online, republishing discovery and state");
    publisher.publish_discovery()?;
    match qcore.fetch_qc_data() {
        Ok(data) => {
            publisher.publish_qc_data(&data)?;
            stats.record_successful_poll();
        }
        Err(error) => warn!("failed to fetch qc data during HA rediscovery: {error}"),
    }
    match qcore.fetch_daily_schedule() {
        Ok(schedule) => {
            publisher.publish_schedule(&schedule)?;
            stats.record_schedule_fetch();
        }
        Err(error) => warn!("failed to fetch schedule during HA rediscovery: {error}"),
    }
    Ok(())
}

fn handle_command(
    command: CommandMessage,
    qcore: &Arc<QcoreClient>,
    publisher: &MqttPublisher,
    stats: &Arc<RuntimeStats>,
) -> AppResult<()> {
    let mut config = qcore.fetch_daily_schedule()?;
    stats.record_schedule_fetch();

    match command {
        CommandMessage::ScheduleEnabled(enabled) => {
            config.state_enabled = enabled;
        }
        CommandMessage::MinSoc(value) => {
            config.min_soc = value;
        }
        CommandMessage::MaxSoc(value) => {
            config.max_soc = value;
        }
        CommandMessage::ScheduleMode { slot, mode } => {
            config.ensure_slot(slot)?;
            config.schedules[slot - 1].mode = mode;
        }
        CommandMessage::ScheduleStart { slot, value } => {
            config.ensure_slot(slot)?;
            config.schedules[slot - 1].start_time = value;
        }
        CommandMessage::ScheduleEnd { slot, value } => {
            config.ensure_slot(slot)?;
            config.schedules[slot - 1].end_time = value;
        }
        CommandMessage::HomeAssistantOnline => {
            unreachable!("HomeAssistantOnline handled by caller")
        }
    }

    qcore.set_daily_schedule(&config)?;
    stats.record_schedule_write();
    publisher.publish_schedule(&config)?;
    info!("applied command and republished schedule state");
    Ok(())
}

#[derive(Debug, Clone)]
struct ReconnectBackoff {
    current: Duration,
    initial: Duration,
    max: Duration,
}

impl ReconnectBackoff {
    fn new(initial: Duration, max: Duration) -> Self {
        Self {
            current: initial,
            initial,
            max,
        }
    }

    fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        self.current = self.current.saturating_mul(2).min(self.max);
        delay
    }

    fn reset(&mut self) {
        self.current = self.initial;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::ReconnectBackoff;

    #[test]
    fn reconnect_backoff_doubles_and_caps() {
        let mut backoff = ReconnectBackoff::new(Duration::from_secs(2), Duration::from_secs(10));
        assert_eq!(backoff.next_delay(), Duration::from_secs(2));
        assert_eq!(backoff.next_delay(), Duration::from_secs(4));
        assert_eq!(backoff.next_delay(), Duration::from_secs(8));
        assert_eq!(backoff.next_delay(), Duration::from_secs(10));
        assert_eq!(backoff.next_delay(), Duration::from_secs(10));
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_secs(2));
    }
}
