use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use log::{debug, warn};
use rumqttc::{Client, Event, LastWill, MqttOptions, Packet, Publish, QoS};
use serde_json::{Value, json};

use crate::config::MqttConfig;
use crate::error::{AppError, AppResult};
use crate::model::{DailyScheduleConfig, QcData, SCHEDULE_COUNT, ScheduleMode};
use crate::stats::RuntimeStats;

const MQTT_KEEPALIVE_SECONDS: u64 = 30;
const MQTT_REQUEST_CAPACITY: usize = 64;

const DEVICE_NAME: &str = "Qendercore Inverter";
const DEVICE_IDENTIFIER: &str = "qendercore_inverter";
const DEVICE_MANUFACTURER: &str = "Qendercore";

const SCHEDULE_ENABLED_PAYLOAD_ON: &str = "ON";
const SCHEDULE_ENABLED_PAYLOAD_OFF: &str = "OFF";

const STATUS_SENSORS: &[StatusSensorDef] = &[
    StatusSensorDef {
        api_key: "grid_export_wh",
        unique_id: "qc_grid_export",
        name: "Grid Export",
        device_class: "power",
        unit: "W",
    },
    StatusSensorDef {
        api_key: "battery_discharge_wh",
        unique_id: "qc_battery_discharge",
        name: "Battery Discharge",
        device_class: "power",
        unit: "W",
    },
];

const ENERGY_SENSORS: &[EnergySensorDef] = &[
    EnergySensorDef {
        api_key: "current_battery_soc",
        unique_id: "qc_battery_soc",
        name: "Battery SoC",
        device_class: "battery",
        unit: "%",
    },
    EnergySensorDef {
        api_key: "import_energy_kwh",
        unique_id: "qc_import_energy",
        name: "Import Energy",
        device_class: "energy",
        unit: "kWh",
    },
    EnergySensorDef {
        api_key: "export_energy_kwh",
        unique_id: "qc_export_energy",
        name: "Export Energy",
        device_class: "energy",
        unit: "kWh",
    },
    EnergySensorDef {
        api_key: "self_consumption_energy_kwh",
        unique_id: "qc_self_consumption",
        name: "Self Consumption",
        device_class: "energy",
        unit: "kWh",
    },
];

const FORCE_CHARGE_CURRENT_KEY: &str = "force_charge_current";
const FORCE_DISCHARGE_CURRENT_KEY: &str = "force_discharge_current";
const SCHEDULE_ENABLED_KEY: &str = "schedule_enabled";
const MIN_SOC_KEY: &str = "min_soc";
const MAX_SOC_KEY: &str = "max_soc";

#[derive(Clone, Copy)]
struct StatusSensorDef {
    api_key: &'static str,
    unique_id: &'static str,
    name: &'static str,
    device_class: &'static str,
    unit: &'static str,
}

#[derive(Clone, Copy)]
struct EnergySensorDef {
    api_key: &'static str,
    unique_id: &'static str,
    name: &'static str,
    device_class: &'static str,
    unit: &'static str,
}

/// Decoded MQTT message addressed to the adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandMessage {
    ScheduleEnabled(bool),
    MinSoc(u8),
    MaxSoc(u8),
    ScheduleMode { slot: usize, mode: ScheduleMode },
    ScheduleStart { slot: usize, value: String },
    ScheduleEnd { slot: usize, value: String },
    /// Home Assistant announced itself online; republish discovery and state.
    HomeAssistantOnline,
}

pub struct MqttPublisher {
    client: Client,
    discovery_prefix: String,
    topic_prefix: String,
    healthy: Arc<AtomicBool>,
    last_error: Arc<Mutex<Option<String>>>,
    stats: Arc<RuntimeStats>,
}

impl MqttPublisher {
    pub fn connect(
        config: &MqttConfig,
        stats: Arc<RuntimeStats>,
    ) -> AppResult<(Self, Receiver<CommandMessage>)> {
        let availability_topic = format!("{}/status", config.topic_prefix);
        let mut options = MqttOptions::new(&config.client_id, &config.host, config.port);
        options.set_keep_alive(Duration::from_secs(MQTT_KEEPALIVE_SECONDS));
        options.set_last_will(LastWill::new(
            availability_topic.clone(),
            "offline",
            QoS::AtLeastOnce,
            true,
        ));

        if let Some(username) = &config.username {
            let password = config.password.clone().unwrap_or_default();
            options.set_credentials(username, password);
        }

        let (client, mut connection) = Client::new(options, MQTT_REQUEST_CAPACITY);

        let healthy = Arc::new(AtomicBool::new(true));
        let last_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let (command_tx, command_rx): (Sender<CommandMessage>, Receiver<CommandMessage>) =
            mpsc::channel();

        let healthy_for_thread = Arc::clone(&healthy);
        let last_error_for_thread = Arc::clone(&last_error);
        let topic_prefix = config.topic_prefix.clone();
        let ha_status_topic = format!("{}/status", config.discovery_prefix);
        thread::spawn(move || {
            let mut failure_message = "mqtt event loop stopped unexpectedly".to_string();
            for notification in connection.iter() {
                match notification {
                    Ok(Event::Incoming(Packet::Publish(publish))) => {
                        let message = if publish.topic == ha_status_topic
                            && publish.payload.as_ref() == b"online"
                        {
                            Some(CommandMessage::HomeAssistantOnline)
                        } else {
                            match decode_command(&topic_prefix, &publish) {
                                Ok(cmd) => cmd,
                                Err(error) => {
                                    warn!(
                                        "ignoring malformed command on {}: {}",
                                        publish.topic, error
                                    );
                                    None
                                }
                            }
                        };
                        if let Some(command) = message {
                            if command_tx.send(command).is_err() {
                                failure_message = "command receiver dropped".to_string();
                                break;
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        failure_message = format!("mqtt event loop stopped: {error}");
                        eprintln!("{failure_message}");
                        break;
                    }
                }
            }
            *last_error_for_thread
                .lock()
                .expect("mqtt last_error mutex must not be poisoned") = Some(failure_message);
            healthy_for_thread.store(false, Ordering::SeqCst);
        });

        let publisher = Self {
            client,
            discovery_prefix: config.discovery_prefix.clone(),
            topic_prefix: config.topic_prefix.clone(),
            healthy,
            last_error,
            stats,
        };
        publisher.publish_text(&publisher.availability_topic(), true, "online")?;
        publisher.subscribe_command_topics()?;
        Ok((publisher, command_rx))
    }

    pub fn ensure_healthy(&self) -> AppResult<()> {
        if self.healthy.load(Ordering::SeqCst) {
            return Ok(());
        }
        let error_message = self
            .last_error
            .lock()
            .expect("mqtt last_error mutex must not be poisoned")
            .clone()
            .unwrap_or_else(|| "mqtt event loop stopped unexpectedly".to_string());
        Err(AppError::MqttDisconnected(error_message))
    }

    pub fn publish_offline_best_effort(&self) {
        if self
            .client
            .publish(self.availability_topic(), QoS::AtLeastOnce, true, "offline")
            .is_ok()
        {
            self.stats.record_mqtt_message_sent();
        }
    }

    pub fn publish_discovery(&self) -> AppResult<()> {
        let device = self.device_payload();
        let availability = self.availability_topic();
        let state_topic = self.state_topic();
        let schedule_state_topic = self.schedule_state_topic();

        for sensor in STATUS_SENSORS {
            let payload = sensor_discovery_payload(
                sensor.name,
                sensor.unique_id,
                &state_topic,
                sensor.api_key,
                Some(sensor.device_class),
                Some(sensor.unit),
                Some("measurement"),
                &availability,
                &device,
            );
            self.publish_json(&self.discovery_topic("sensor", sensor.unique_id), true, &payload)?;
        }
        for sensor in ENERGY_SENSORS {
            let state_class = if sensor.device_class == "energy" {
                "total_increasing"
            } else {
                "measurement"
            };
            let payload = sensor_discovery_payload(
                sensor.name,
                sensor.unique_id,
                &state_topic,
                sensor.api_key,
                Some(sensor.device_class),
                Some(sensor.unit),
                Some(state_class),
                &availability,
                &device,
            );
            self.publish_json(&self.discovery_topic("sensor", sensor.unique_id), true, &payload)?;
        }

        let force_charge_payload = sensor_discovery_payload(
            "Force Charge Current",
            "qc_force_charge_current",
            &schedule_state_topic,
            FORCE_CHARGE_CURRENT_KEY,
            Some("current"),
            Some("A"),
            Some("measurement"),
            &availability,
            &device,
        );
        self.publish_json(
            &self.discovery_topic("sensor", "qc_force_charge_current"),
            true,
            &force_charge_payload,
        )?;
        let force_discharge_payload = sensor_discovery_payload(
            "Force Discharge Current",
            "qc_force_discharge_current",
            &schedule_state_topic,
            FORCE_DISCHARGE_CURRENT_KEY,
            Some("current"),
            Some("A"),
            Some("measurement"),
            &availability,
            &device,
        );
        self.publish_json(
            &self.discovery_topic("sensor", "qc_force_discharge_current"),
            true,
            &force_discharge_payload,
        )?;

        // Switch: schedule enabled
        let switch_payload = switch_discovery_payload(
            "Schedule Enabled",
            "qc_schedule_enabled",
            &schedule_state_topic,
            SCHEDULE_ENABLED_KEY,
            &self.command_topic(SCHEDULE_ENABLED_KEY),
            &availability,
            &device,
        );
        self.publish_json(
            &self.discovery_topic("switch", "qc_schedule_enabled"),
            true,
            &switch_payload,
        )?;

        // Numbers: min/max SoC
        let min_payload = number_discovery_payload(
            "Min SoC",
            "qc_min_soc",
            &schedule_state_topic,
            MIN_SOC_KEY,
            &self.command_topic(MIN_SOC_KEY),
            0.0,
            100.0,
            1.0,
            "%",
            &availability,
            &device,
        );
        self.publish_json(
            &self.discovery_topic("number", "qc_min_soc"),
            true,
            &min_payload,
        )?;
        let max_payload = number_discovery_payload(
            "Max SoC",
            "qc_max_soc",
            &schedule_state_topic,
            MAX_SOC_KEY,
            &self.command_topic(MAX_SOC_KEY),
            0.0,
            100.0,
            1.0,
            "%",
            &availability,
            &device,
        );
        self.publish_json(
            &self.discovery_topic("number", "qc_max_soc"),
            true,
            &max_payload,
        )?;

        // Per-slot select + text entities
        for slot in 1..=SCHEDULE_COUNT {
            let mode_unique = format!("qc_schedule_{slot}_mode");
            let mode_key = schedule_mode_state_key(slot);
            let mode_payload = select_discovery_payload(
                &format!("Schedule {slot} Mode"),
                &mode_unique,
                &schedule_state_topic,
                &mode_key,
                &self.command_topic(&mode_key),
                &ScheduleMode::ALL
                    .iter()
                    .map(|m| m.display_name().to_string())
                    .collect::<Vec<_>>(),
                &availability,
                &device,
            );
            self.publish_json(&self.discovery_topic("select", &mode_unique), true, &mode_payload)?;

            let start_unique = format!("qc_schedule_{slot}_start");
            let start_key = schedule_start_state_key(slot);
            let start_payload = text_discovery_payload(
                &format!("Schedule {slot} Start"),
                &start_unique,
                &schedule_state_topic,
                &start_key,
                &self.command_topic(&start_key),
                Some(8),
                Some(8),
                Some(r"^\d{2}:\d{2}:\d{2}$"),
                &availability,
                &device,
            );
            self.publish_json(&self.discovery_topic("text", &start_unique), true, &start_payload)?;

            let end_unique = format!("qc_schedule_{slot}_end");
            let end_key = schedule_end_state_key(slot);
            let end_payload = text_discovery_payload(
                &format!("Schedule {slot} End"),
                &end_unique,
                &schedule_state_topic,
                &end_key,
                &self.command_topic(&end_key),
                Some(8),
                Some(8),
                Some(r"^\d{2}:\d{2}:\d{2}$"),
                &availability,
                &device,
            );
            self.publish_json(&self.discovery_topic("text", &end_unique), true, &end_payload)?;
        }

        Ok(())
    }

    fn subscribe_command_topics(&self) -> AppResult<()> {
        // HA birth topic: republish discovery when Home Assistant restarts
        self.client.subscribe(
            format!("{}/status", self.discovery_prefix),
            QoS::AtLeastOnce,
        )?;
        self.client
            .subscribe(self.command_topic(SCHEDULE_ENABLED_KEY), QoS::AtLeastOnce)?;
        self.client
            .subscribe(self.command_topic(MIN_SOC_KEY), QoS::AtLeastOnce)?;
        self.client
            .subscribe(self.command_topic(MAX_SOC_KEY), QoS::AtLeastOnce)?;
        for slot in 1..=SCHEDULE_COUNT {
            self.client.subscribe(
                self.command_topic(&schedule_mode_state_key(slot)),
                QoS::AtLeastOnce,
            )?;
            self.client.subscribe(
                self.command_topic(&schedule_start_state_key(slot)),
                QoS::AtLeastOnce,
            )?;
            self.client.subscribe(
                self.command_topic(&schedule_end_state_key(slot)),
                QoS::AtLeastOnce,
            )?;
        }
        Ok(())
    }

    pub fn publish_qc_data(&self, data: &QcData) -> AppResult<()> {
        let mut payload = serde_json::Map::new();
        for sensor in STATUS_SENSORS {
            if let Some(value) = data.status.get(sensor.api_key) {
                payload.insert(sensor.api_key.to_string(), json!(value));
            } else {
                debug!("status key '{}' missing from API response", sensor.api_key);
            }
        }
        for sensor in ENERGY_SENSORS {
            if let Some(value) = data.energy.get(sensor.api_key) {
                payload.insert(sensor.api_key.to_string(), json!(value));
            } else {
                debug!("energy key '{}' missing from API response", sensor.api_key);
            }
        }
        let extra_status = data
            .status
            .keys()
            .filter(|k| !STATUS_SENSORS.iter().any(|s| s.api_key == k.as_str()))
            .collect::<Vec<_>>();
        if !extra_status.is_empty() {
            debug!("unmapped status keys: {extra_status:?}");
        }
        let extra_energy = data
            .energy
            .keys()
            .filter(|k| !ENERGY_SENSORS.iter().any(|s| s.api_key == k.as_str()))
            .collect::<Vec<_>>();
        if !extra_energy.is_empty() {
            debug!("unmapped energy keys: {extra_energy:?}");
        }

        let topic = self.state_topic();
        self.publish_json(&topic, true, &Value::Object(payload))
    }

    pub fn publish_schedule(&self, config: &DailyScheduleConfig) -> AppResult<()> {
        let payload = schedule_state_payload(config);
        let topic = self.schedule_state_topic();
        self.publish_json(&topic, true, &payload)
    }

    fn publish_json(&self, topic: &str, retain: bool, payload: &Value) -> AppResult<()> {
        self.publish_text(topic, retain, payload.to_string())
    }

    fn publish_text(
        &self,
        topic: &str,
        retain: bool,
        payload: impl Into<Vec<u8>>,
    ) -> AppResult<()> {
        self.ensure_healthy()?;
        self.client
            .publish(topic, QoS::AtLeastOnce, retain, payload)?;
        self.stats.record_mqtt_message_sent();
        Ok(())
    }

    fn availability_topic(&self) -> String {
        format!("{}/status", self.topic_prefix)
    }

    fn state_topic(&self) -> String {
        format!("{}/state", self.topic_prefix)
    }

    fn schedule_state_topic(&self) -> String {
        format!("{}/schedule/state", self.topic_prefix)
    }

    fn command_topic(&self, suffix: &str) -> String {
        format!("{}/cmd/{suffix}", self.topic_prefix)
    }

    fn discovery_topic(&self, component: &str, unique_id: &str) -> String {
        format!(
            "{}/{}/{}/config",
            self.discovery_prefix, component, unique_id
        )
    }

    fn device_payload(&self) -> Value {
        json!({
            "name": DEVICE_NAME,
            "identifiers": [DEVICE_IDENTIFIER],
            "manufacturer": DEVICE_MANUFACTURER,
        })
    }
}

fn schedule_mode_state_key(slot: usize) -> String {
    format!("schedule_{slot}_mode")
}

fn schedule_start_state_key(slot: usize) -> String {
    format!("schedule_{slot}_start")
}

fn schedule_end_state_key(slot: usize) -> String {
    format!("schedule_{slot}_end")
}

pub fn schedule_state_payload(config: &DailyScheduleConfig) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert(
        SCHEDULE_ENABLED_KEY.to_string(),
        json!(if config.state_enabled {
            SCHEDULE_ENABLED_PAYLOAD_ON
        } else {
            SCHEDULE_ENABLED_PAYLOAD_OFF
        }),
    );
    payload.insert(MIN_SOC_KEY.to_string(), json!(config.min_soc));
    payload.insert(MAX_SOC_KEY.to_string(), json!(config.max_soc));
    payload.insert(
        FORCE_CHARGE_CURRENT_KEY.to_string(),
        json!(config.force_charge_current),
    );
    payload.insert(
        FORCE_DISCHARGE_CURRENT_KEY.to_string(),
        json!(config.force_discharge_current),
    );
    for (idx, schedule) in config.schedules.iter().enumerate() {
        let slot = idx + 1;
        payload.insert(
            schedule_mode_state_key(slot),
            json!(schedule.mode.display_name()),
        );
        payload.insert(
            schedule_start_state_key(slot),
            json!(schedule.start_time.clone()),
        );
        payload.insert(
            schedule_end_state_key(slot),
            json!(schedule.end_time.clone()),
        );
    }
    Value::Object(payload)
}

fn sensor_discovery_payload(
    name: &str,
    unique_id: &str,
    state_topic: &str,
    state_key: &str,
    device_class: Option<&str>,
    unit_of_measurement: Option<&str>,
    state_class: Option<&str>,
    availability_topic: &str,
    device: &Value,
) -> Value {
    let mut payload = base_discovery_payload(name, unique_id, state_topic, state_key, availability_topic, device);
    if let Some(value) = device_class {
        payload.insert("device_class".to_string(), json!(value));
    }
    if let Some(value) = unit_of_measurement {
        payload.insert("unit_of_measurement".to_string(), json!(value));
    }
    if let Some(value) = state_class {
        payload.insert("state_class".to_string(), json!(value));
    }
    Value::Object(payload)
}

fn switch_discovery_payload(
    name: &str,
    unique_id: &str,
    state_topic: &str,
    state_key: &str,
    command_topic: &str,
    availability_topic: &str,
    device: &Value,
) -> Value {
    let mut payload =
        base_discovery_payload(name, unique_id, state_topic, state_key, availability_topic, device);
    payload.insert("command_topic".to_string(), json!(command_topic));
    payload.insert("payload_on".to_string(), json!(SCHEDULE_ENABLED_PAYLOAD_ON));
    payload.insert("payload_off".to_string(), json!(SCHEDULE_ENABLED_PAYLOAD_OFF));
    payload.insert("state_on".to_string(), json!(SCHEDULE_ENABLED_PAYLOAD_ON));
    payload.insert("state_off".to_string(), json!(SCHEDULE_ENABLED_PAYLOAD_OFF));
    payload.insert("optimistic".to_string(), json!(false));
    Value::Object(payload)
}

#[allow(clippy::too_many_arguments)]
fn number_discovery_payload(
    name: &str,
    unique_id: &str,
    state_topic: &str,
    state_key: &str,
    command_topic: &str,
    min: f64,
    max: f64,
    step: f64,
    unit_of_measurement: &str,
    availability_topic: &str,
    device: &Value,
) -> Value {
    let mut payload =
        base_discovery_payload(name, unique_id, state_topic, state_key, availability_topic, device);
    payload.insert("command_topic".to_string(), json!(command_topic));
    payload.insert("min".to_string(), json!(min));
    payload.insert("max".to_string(), json!(max));
    payload.insert("step".to_string(), json!(step));
    payload.insert("mode".to_string(), json!("slider"));
    payload.insert("unit_of_measurement".to_string(), json!(unit_of_measurement));
    payload.insert("optimistic".to_string(), json!(false));
    Value::Object(payload)
}

fn select_discovery_payload(
    name: &str,
    unique_id: &str,
    state_topic: &str,
    state_key: &str,
    command_topic: &str,
    options: &[String],
    availability_topic: &str,
    device: &Value,
) -> Value {
    let mut payload =
        base_discovery_payload(name, unique_id, state_topic, state_key, availability_topic, device);
    payload.insert("command_topic".to_string(), json!(command_topic));
    payload.insert("options".to_string(), json!(options));
    payload.insert("optimistic".to_string(), json!(false));
    Value::Object(payload)
}

#[allow(clippy::too_many_arguments)]
fn text_discovery_payload(
    name: &str,
    unique_id: &str,
    state_topic: &str,
    state_key: &str,
    command_topic: &str,
    min: Option<u32>,
    max: Option<u32>,
    pattern: Option<&str>,
    availability_topic: &str,
    device: &Value,
) -> Value {
    let mut payload =
        base_discovery_payload(name, unique_id, state_topic, state_key, availability_topic, device);
    payload.insert("command_topic".to_string(), json!(command_topic));
    if let Some(value) = min {
        payload.insert("min".to_string(), json!(value));
    }
    if let Some(value) = max {
        payload.insert("max".to_string(), json!(value));
    }
    if let Some(value) = pattern {
        payload.insert("pattern".to_string(), json!(value));
    }
    payload.insert("optimistic".to_string(), json!(false));
    Value::Object(payload)
}

fn base_discovery_payload(
    name: &str,
    unique_id: &str,
    state_topic: &str,
    state_key: &str,
    availability_topic: &str,
    device: &Value,
) -> serde_json::Map<String, Value> {
    let mut payload = serde_json::Map::new();
    payload.insert("name".to_string(), json!(name));
    payload.insert("unique_id".to_string(), json!(unique_id));
    payload.insert("state_topic".to_string(), json!(state_topic));
    payload.insert(
        "value_template".to_string(),
        json!(format!("{{{{ value_json.{state_key} }}}}")),
    );
    payload.insert("availability_topic".to_string(), json!(availability_topic));
    payload.insert("payload_available".to_string(), json!("online"));
    payload.insert("payload_not_available".to_string(), json!("offline"));
    payload.insert("device".to_string(), device.clone());
    payload
}

fn decode_command(topic_prefix: &str, publish: &Publish) -> AppResult<Option<CommandMessage>> {
    let cmd_prefix = format!("{topic_prefix}/cmd/");
    let suffix = match publish.topic.strip_prefix(&cmd_prefix) {
        Some(value) => value,
        None => return Ok(None),
    };
    let payload_text = std::str::from_utf8(&publish.payload).map_err(|error| {
        AppError::InvalidState(format!("non-utf8 payload on {}: {error}", publish.topic))
    })?;

    if suffix == SCHEDULE_ENABLED_KEY {
        let on = match payload_text {
            SCHEDULE_ENABLED_PAYLOAD_ON => true,
            SCHEDULE_ENABLED_PAYLOAD_OFF => false,
            other => {
                return Err(AppError::InvalidState(format!(
                    "schedule_enabled expected ON/OFF, got {other}"
                )));
            }
        };
        return Ok(Some(CommandMessage::ScheduleEnabled(on)));
    }
    if suffix == MIN_SOC_KEY {
        return Ok(Some(CommandMessage::MinSoc(parse_soc(payload_text)?)));
    }
    if suffix == MAX_SOC_KEY {
        return Ok(Some(CommandMessage::MaxSoc(parse_soc(payload_text)?)));
    }
    if let Some(slot) = parse_schedule_slot(suffix, "_mode") {
        let mode = ScheduleMode::from_display_name(payload_text)?;
        return Ok(Some(CommandMessage::ScheduleMode { slot, mode }));
    }
    if let Some(slot) = parse_schedule_slot(suffix, "_start") {
        return Ok(Some(CommandMessage::ScheduleStart {
            slot,
            value: payload_text.to_string(),
        }));
    }
    if let Some(slot) = parse_schedule_slot(suffix, "_end") {
        return Ok(Some(CommandMessage::ScheduleEnd {
            slot,
            value: payload_text.to_string(),
        }));
    }

    debug!("unrecognized command topic: {}", publish.topic);
    Ok(None)
}

fn parse_soc(value: &str) -> AppResult<u8> {
    let parsed = value
        .parse::<f64>()
        .map_err(|error| AppError::InvalidState(format!("soc value '{value}' invalid: {error}")))?;
    if !(0.0..=100.0).contains(&parsed) {
        return Err(AppError::InvalidState(format!(
            "soc value {parsed} out of range 0..=100"
        )));
    }
    Ok(parsed.round() as u8)
}

fn parse_schedule_slot(suffix: &str, tail: &str) -> Option<usize> {
    let stripped = suffix.strip_prefix("schedule_")?.strip_suffix(tail)?;
    let slot = stripped.parse::<usize>().ok()?;
    if (1..=SCHEDULE_COUNT).contains(&slot) {
        Some(slot)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Schedule;
    use rumqttc::Publish;

    fn make_publish(topic: &str, payload: &[u8]) -> Publish {
        Publish::new(topic, QoS::AtLeastOnce, payload.to_vec())
    }

    #[test]
    fn decode_schedule_enabled_on() {
        let publish = make_publish("qendercore/cmd/schedule_enabled", b"ON");
        let cmd = decode_command("qendercore", &publish).unwrap().unwrap();
        assert_eq!(cmd, CommandMessage::ScheduleEnabled(true));
    }

    #[test]
    fn decode_min_soc_value() {
        let publish = make_publish("qendercore/cmd/min_soc", b"15");
        let cmd = decode_command("qendercore", &publish).unwrap().unwrap();
        assert_eq!(cmd, CommandMessage::MinSoc(15));
    }

    #[test]
    fn decode_min_soc_rejects_out_of_range() {
        let publish = make_publish("qendercore/cmd/min_soc", b"150");
        let error = decode_command("qendercore", &publish).unwrap_err();
        assert!(error.to_string().contains("out of range"));
    }

    #[test]
    fn decode_schedule_mode() {
        let publish = make_publish("qendercore/cmd/schedule_2_mode", b"Forced Charge");
        let cmd = decode_command("qendercore", &publish).unwrap().unwrap();
        assert_eq!(
            cmd,
            CommandMessage::ScheduleMode {
                slot: 2,
                mode: ScheduleMode::ForcedCharge
            }
        );
    }

    #[test]
    fn decode_schedule_start() {
        let publish = make_publish("qendercore/cmd/schedule_3_start", b"01:30:00");
        let cmd = decode_command("qendercore", &publish).unwrap().unwrap();
        assert_eq!(
            cmd,
            CommandMessage::ScheduleStart {
                slot: 3,
                value: "01:30:00".to_string()
            }
        );
    }

    #[test]
    fn decode_unknown_topic_returns_none() {
        let publish = make_publish("qendercore/cmd/unknown", b"x");
        assert!(decode_command("qendercore", &publish).unwrap().is_none());
    }

    #[test]
    fn decode_other_prefix_returns_none() {
        let publish = make_publish("other/cmd/min_soc", b"5");
        assert!(decode_command("qendercore", &publish).unwrap().is_none());
    }

    #[test]
    fn schedule_state_payload_contains_all_keys() {
        let config = DailyScheduleConfig {
            state_enabled: true,
            force_charge_current: 100.0,
            force_discharge_current: 48.0,
            min_soc: 15,
            max_soc: 100,
            schedules: vec![
                Schedule {
                    mode: ScheduleMode::ForcedCharge,
                    start_time: "02:00:00".into(),
                    end_time: "05:00:00".into(),
                },
                Schedule::disabled(),
                Schedule::disabled(),
                Schedule::disabled(),
                Schedule::disabled(),
            ],
        };
        let payload = schedule_state_payload(&config);
        let object = payload.as_object().unwrap();
        assert_eq!(object["schedule_enabled"], json!("ON"));
        assert_eq!(object["min_soc"], json!(15));
        assert_eq!(object["max_soc"], json!(100));
        assert_eq!(object["force_charge_current"], json!(100.0));
        assert_eq!(object["schedule_1_mode"], json!("Forced Charge"));
        assert_eq!(object["schedule_1_start"], json!("02:00:00"));
        assert_eq!(object["schedule_5_mode"], json!("Disable"));
    }

    #[test]
    fn sensor_discovery_payload_uses_value_template() {
        let device = json!({"name": "x"});
        let payload = sensor_discovery_payload(
            "Battery SoC",
            "qc_battery_soc",
            "qendercore/state",
            "current_battery_soc",
            Some("battery"),
            Some("%"),
            Some("measurement"),
            "qendercore/status",
            &device,
        );
        assert_eq!(
            payload["value_template"],
            json!("{{ value_json.current_battery_soc }}")
        );
        assert_eq!(payload["device_class"], json!("battery"));
        assert_eq!(payload["state_class"], json!("measurement"));
    }
}
