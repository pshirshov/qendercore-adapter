use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use log::{debug, warn};
use reqwest::blocking::Client;
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::QcoreConfig;
use crate::error::{AppError, AppResult};
use crate::model::{DailyScheduleConfig, QcData, SCHEDULE_COUNT, Schedule, ScheduleMode};

const TOKEN_FILE_NAME: &str = "token.json";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:124.0) Gecko/20100101 Firefox/124.0";

/// Stateful client for the Qendercore HTTP API. Owns an HTTP connection pool
/// and an in-memory copy of the cached bearer token.
pub struct QcoreClient {
    config: QcoreConfig,
    http: Client,
    token: Mutex<Option<String>>,
    device_id: Mutex<Option<String>>,
}

impl QcoreClient {
    pub fn new(config: QcoreConfig) -> AppResult<Arc<Self>> {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, header_value("https://www.qendercore.com")?);
        headers.insert(header::REFERER, header_value("https://www.qendercore.com")?);
        headers.insert(header::ACCEPT, header_value("application/json")?);
        headers.insert(
            header::ACCEPT_LANGUAGE,
            header_value("en-US,en;q=0.5")?,
        );
        headers.insert(header::CACHE_CONTROL, header_value("no-cache")?);
        headers.insert(header::PRAGMA, header_value("no-cache")?);
        headers.insert(
            HeaderName::from_static("sec-fetch-dest"),
            header_value("empty")?,
        );
        headers.insert(
            HeaderName::from_static("sec-fetch-mode"),
            header_value("cors")?,
        );
        headers.insert(
            HeaderName::from_static("sec-fetch-site"),
            header_value("same-site")?,
        );
        headers.insert(HeaderName::from_static("sec-gpc"), header_value("1")?);
        headers.insert(
            HeaderName::from_static("x-qc-client-seq"),
            header_value("W.1.1")?,
        );

        let http = Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .timeout(config.http_timeout)
            .pool_idle_timeout(Some(Duration::from_secs(90)))
            .build()?;

        Ok(Arc::new(Self {
            config,
            http,
            token: Mutex::new(None),
            device_id: Mutex::new(None),
        }))
    }

    fn token_path(&self) -> PathBuf {
        self.config.cache_dir.join(TOKEN_FILE_NAME)
    }

    fn load_cached_token_from_disk(&self) -> Option<String> {
        let path = self.token_path();
        let raw = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => return None,
        };
        let parsed: Result<TokenFile, _> = serde_json::from_str(&raw);
        match parsed {
            Ok(file) => Some(file.token),
            Err(error) => {
                warn!(
                    "ignoring malformed token cache at {}: {}",
                    path.display(),
                    error
                );
                None
            }
        }
    }

    fn store_token_on_disk(&self, token: &str) -> AppResult<()> {
        fs::create_dir_all(&self.config.cache_dir)?;
        let path = self.token_path();
        let body = serde_json::to_string(&TokenFile {
            token: token.to_string(),
        })?;
        fs::write(&path, body)?;
        Ok(())
    }

    /// Returns a valid bearer token, fetching/refreshing as needed.
    pub fn get_token(&self) -> AppResult<String> {
        if let Some(token) = self.token.lock().unwrap().clone() {
            return Ok(token);
        }

        if let Some(token) = self.load_cached_token_from_disk() {
            if self.validate_token(&token).unwrap_or(false) {
                *self.token.lock().unwrap() = Some(token.clone());
                return Ok(token);
            }
            debug!("cached qendercore token rejected, refreshing");
        }

        let token = self.fetch_new_token()?;
        self.store_token_on_disk(&token)?;
        *self.token.lock().unwrap() = Some(token.clone());
        Ok(token)
    }

    fn invalidate_token(&self) {
        *self.token.lock().unwrap() = None;
    }

    fn fetch_new_token(&self) -> AppResult<String> {
        let url = format!("{}/auth/login", self.config.api_url);
        let resp = self
            .http
            .post(&url)
            .form(&[
                ("username", self.config.login.as_str()),
                ("password", self.config.password.as_str()),
            ])
            .send()?
            .error_for_status()?;
        let body: LoginResponse = resp.json()?;
        Ok(body.access_token)
    }

    fn validate_token(&self, token: &str) -> AppResult<bool> {
        let url = format!("{}/s/accountinfo", self.config.api_url);
        let resp = self.http.get(&url).bearer_auth(token).send()?;
        if !resp.status().is_success() {
            return Ok(false);
        }
        let body: Value = resp.json()?;
        Ok(body.get("uid").is_some())
    }

    fn authorized_get(&self, path: &str) -> AppResult<Value> {
        let url = format!("{}{path}", self.config.api_url);
        let attempt = |token: &str| -> AppResult<Value> {
            let resp = self.http.get(&url).bearer_auth(token).send()?;
            handle_response(resp)
        };

        let token = self.get_token()?;
        match attempt(&token) {
            Ok(value) => Ok(value),
            Err(AppError::QcoreApi(message)) if message.contains("401") => {
                self.invalidate_token();
                let token = self.get_token()?;
                attempt(&token)
            }
            Err(other) => Err(other),
        }
    }

    fn authorized_post_json(&self, path: &str, body: &Value) -> AppResult<Value> {
        let url = format!("{}{path}", self.config.api_url);
        let attempt = |token: &str| -> AppResult<Value> {
            let resp = self
                .http
                .post(&url)
                .bearer_auth(token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_vec(body)?)
                .send()?;
            handle_response(resp)
        };

        let token = self.get_token()?;
        match attempt(&token) {
            Ok(value) => Ok(value),
            Err(AppError::QcoreApi(message)) if message.contains("401") => {
                self.invalidate_token();
                let token = self.get_token()?;
                attempt(&token)
            }
            Err(other) => Err(other),
        }
    }

    pub fn get_device_id(&self) -> AppResult<String> {
        if let Some(id) = self.device_id.lock().unwrap().clone() {
            return Ok(id);
        }
        let dashboard = self.authorized_get("/s/dashboard")?;
        let id = extract_device_id(&dashboard)?;
        *self.device_id.lock().unwrap() = Some(id.clone());
        Ok(id)
    }

    pub fn fetch_qc_data(&self) -> AppResult<QcData> {
        let dashboard = self.authorized_get("/s/dashboard")?;

        let widgets = collect_widgets(&dashboard)?;
        let mut data = QcData::new();

        for widget in &widgets {
            let chart_request = build_chart_request(widget)?;
            let chart = self.authorized_post_json("/h/chart", &chart_request)?;
            extract_chart_into(&chart, &widget.title, &mut data);
        }

        Ok(data)
    }

    fn fetch_enchwt(&self, device_id: &str) -> AppResult<String> {
        let body = json!({
            "_ft": "hwv",
            "hwid": device_id,
            "f": ["hwid", "enchwt"],
        });
        let resp = self.authorized_post_json("/h/ds", &body)?;
        let cols = resp
            .get("cols")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError::QcoreApi("ds response missing cols".to_string()))?;
        let enchwt_index = cols
            .iter()
            .position(|col| col.get("id").and_then(Value::as_str) == Some("enchwt"))
            .ok_or_else(|| AppError::QcoreApi("ds response missing enchwt column".to_string()))?;
        let rows = resp
            .get("rows")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError::QcoreApi("ds response missing rows".to_string()))?;
        let first_row = rows.first().and_then(Value::as_array).ok_or_else(|| {
            AppError::QcoreApi("ds response returned no rows".to_string())
        })?;
        let value = first_row
            .get(enchwt_index)
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::QcoreApi("enchwt cell empty".to_string()))?;
        Ok(value.to_string())
    }

    pub fn fetch_daily_schedule(&self) -> AppResult<DailyScheduleConfig> {
        let device_id = self.get_device_id()?;
        let enchwt = self.fetch_enchwt(&device_id)?;
        let path = format!(
            "/h/devices/{device_id}/widgets/dailysched?enchwt={enchwt}",
        );
        let resp = self.authorized_get(&path)?;
        parse_daily_schedule(&resp)
    }

    pub fn set_daily_schedule(&self, config: &DailyScheduleConfig) -> AppResult<Value> {
        let device_id = self.get_device_id()?;

        let mut body = serde_json::Map::new();
        body.insert(
            "sched_state".to_string(),
            json!(if config.state_enabled { "1" } else { "0" }),
        );
        body.insert(
            "force_charge_curr".to_string(),
            json!(format_float(config.force_charge_current)),
        );
        body.insert(
            "force_discharge_curr".to_string(),
            json!(format_float(config.force_discharge_current)),
        );
        body.insert("min_soc".to_string(), json!(config.min_soc.to_string()));
        body.insert("max_soc".to_string(), json!(config.max_soc.to_string()));

        for slot in 1..=SCHEDULE_COUNT {
            let schedule = config
                .schedules
                .get(slot - 1)
                .cloned()
                .unwrap_or_else(Schedule::disabled);
            body.insert(
                format!("s{slot}_mode"),
                json!(schedule.mode.as_api_int().to_string()),
            );
            body.insert(format!("s{slot}_starttime"), json!(schedule.start_time));
            body.insert(format!("s{slot}_endtime"), json!(schedule.end_time));
        }

        let path = format!("/h/devices/{device_id}/solt/daysched");
        self.authorized_post_json(&path, &Value::Object(body))
    }
}

fn header_value(value: &str) -> AppResult<HeaderValue> {
    HeaderValue::from_str(value).map_err(|error| {
        AppError::InvalidConfig(format!("invalid header value '{value}': {error}"))
    })
}

fn handle_response(resp: reqwest::blocking::Response) -> AppResult<Value> {
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(AppError::QcoreApi(format!(
            "{} {}: {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or(""),
            body
        )));
    }
    let value: Value = resp.json()?;
    Ok(value)
}

fn extract_device_id(dashboard: &Value) -> AppResult<String> {
    let widget = dashboard
        .get("rows")
        .and_then(Value::as_array)
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("cells"))
        .and_then(Value::as_array)
        .and_then(|cells| cells.first())
        .and_then(|cell| cell.get("widget"))
        .ok_or_else(|| AppError::QcoreApi("dashboard missing widgets".to_string()))?;

    let device_id = widget
        .get("datafetch")
        .and_then(|df| df.get("parameters"))
        .and_then(|p| p.get("deviceId"))
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::QcoreApi("widget missing deviceId".to_string()))?;
    Ok(device_id.to_string())
}

#[derive(Debug)]
struct DashboardWidget {
    datafetch: Value,
    echart_opts: Value,
    title: String,
}

fn collect_widgets(dashboard: &Value) -> AppResult<Vec<DashboardWidget>> {
    let rows = dashboard
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::QcoreApi("dashboard missing rows".to_string()))?;

    let mut widgets = Vec::new();
    for row in rows {
        let cells = row
            .get("cells")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError::QcoreApi("dashboard row missing cells".to_string()))?;
        for cell in cells {
            let widget = cell
                .get("widget")
                .ok_or_else(|| AppError::QcoreApi("dashboard cell missing widget".to_string()))?;

            let datafetch = widget
                .get("datafetch")
                .ok_or_else(|| AppError::QcoreApi("widget missing datafetch".to_string()))?;
            let echart_opts = widget.get("echartOpts").cloned().unwrap_or(Value::Null);
            let title = widget
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            widgets.push(DashboardWidget {
                datafetch: datafetch.clone(),
                echart_opts,
                title,
            });
        }
    }

    Ok(widgets)
}

/// Builds the body for `/h/chart`.
fn build_chart_request(widget: &DashboardWidget) -> AppResult<Value> {
    let parameters = widget
        .datafetch
        .get("parameters")
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::QcoreApi("widget datafetch missing parameters".to_string()))?;

    let mut datafetch = serde_json::Map::new();
    if let Some(fetch_type) = widget.datafetch.get("fetchType") {
        datafetch.insert("fetchType".to_string(), fetch_type.clone());
    }
    if let Some(device_id) = parameters.get("deviceId") {
        datafetch.insert("deviceId".to_string(), device_id.clone());
    }
    for (key, value) in parameters {
        datafetch
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }

    Ok(json!({
        "datafetch": Value::Object(datafetch),
        "echartOpts": widget.echart_opts,
    }))
}

fn extract_chart_into(chart: &Value, widget_title: &str, data: &mut QcData) {
    let series = match chart.get("series") {
        Some(value) => value,
        None => return,
    };

    if let Some(links) = series.get("links").and_then(Value::as_array) {
        for link in links {
            let id = match link.get("id").and_then(Value::as_str) {
                Some(value) => value,
                None => continue,
            };
            let value = match link.get("value").and_then(Value::as_f64) {
                Some(value) => value,
                None => continue,
            };
            let key = format!("{}_wh", normalize_key(id));
            data.status.insert(key, value);
        }
        return;
    }

    if chart.get("dataset").is_some() {
        // Time series — currently unused by the MQTT adapter, intentionally skipped.
        return;
    }

    if let Some(items) = series.as_array() {
        for element in items {
            let entries = match element.get("data").and_then(Value::as_array) {
                Some(items) => items,
                None => continue,
            };
            for d in entries {
                let key = match d.get("name").and_then(Value::as_str) {
                    Some(name) => normalize_key(name),
                    None => normalize_key(widget_title),
                };
                if let Some(value) = d.get("value").and_then(Value::as_f64) {
                    data.energy.insert(key, value);
                }
            }
        }
    }
}

pub fn normalize_key(name: &str) -> String {
    let lowered = name.to_lowercase();
    let mut output = String::with_capacity(lowered.len());
    let mut prev_was_underscore = false;
    for ch in lowered.chars() {
        let mapped = match ch {
            ' ' | '(' | ')' | '.' | '-' => '_',
            other => other,
        };
        if mapped == '_' {
            if !prev_was_underscore {
                output.push('_');
            }
            prev_was_underscore = true;
        } else {
            output.push(mapped);
            prev_was_underscore = false;
        }
    }
    output.trim_matches('_').to_string()
}

fn parse_daily_schedule(resp: &Value) -> AppResult<DailyScheduleConfig> {
    let filters = resp
        .get("filters")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::QcoreApi("schedule response missing filters".to_string()))?;

    let mut values: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    for filter in filters {
        let output = match filter.get("output").and_then(Value::as_str) {
            Some(value) => value,
            None => continue,
        };
        let init = match filter.get("init") {
            Some(value) => value,
            None => continue,
        };
        values.insert(output.to_string(), init.clone());
    }

    let mut schedules = Vec::with_capacity(SCHEDULE_COUNT);
    for slot in 1..=SCHEDULE_COUNT {
        let mode_key = format!("s{slot}_mode");
        let start_key = format!("s{slot}_starttime");
        let end_key = format!("s{slot}_endtime");

        let mode_value = require_value(&values, &mode_key)?;
        let start_value = require_value(&values, &start_key)?;
        let end_value = require_value(&values, &end_key)?;

        let mode_int = read_u8(mode_value, &mode_key)?;
        let mode = ScheduleMode::try_from(mode_int)?;
        schedules.push(Schedule {
            mode,
            start_time: read_string(start_value, &start_key)?,
            end_time: read_string(end_value, &end_key)?,
        });
    }

    Ok(DailyScheduleConfig {
        state_enabled: read_string(require_value(&values, "sched_state")?, "sched_state")? == "1",
        force_charge_current: read_f64(
            require_value(&values, "force_charge_curr")?,
            "force_charge_curr",
        )?,
        force_discharge_current: read_f64(
            require_value(&values, "force_discharge_curr")?,
            "force_discharge_curr",
        )?,
        min_soc: read_u8(require_value(&values, "min_soc")?, "min_soc")?,
        max_soc: read_u8(require_value(&values, "max_soc")?, "max_soc")?,
        schedules,
    })
}

fn require_value<'a>(
    values: &'a std::collections::BTreeMap<String, Value>,
    key: &str,
) -> AppResult<&'a Value> {
    values
        .get(key)
        .ok_or_else(|| AppError::QcoreApi(format!("schedule response missing {key}")))
}

fn read_string(value: &Value, key: &str) -> AppResult<String> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_string());
    }
    if let Some(number) = value.as_f64() {
        return Ok(format_float(number));
    }
    Err(AppError::QcoreApi(format!(
        "schedule field {key} expected string, got {value}"
    )))
}

fn read_f64(value: &Value, key: &str) -> AppResult<f64> {
    if let Some(number) = value.as_f64() {
        return Ok(number);
    }
    if let Some(text) = value.as_str() {
        return text.parse::<f64>().map_err(|error| {
            AppError::QcoreApi(format!("schedule field {key} not a float: {error}"))
        });
    }
    Err(AppError::QcoreApi(format!(
        "schedule field {key} expected number, got {value}"
    )))
}

fn read_u8(value: &Value, key: &str) -> AppResult<u8> {
    let number = read_f64(value, key)?;
    if !(0.0..=255.0).contains(&number) {
        return Err(AppError::QcoreApi(format!(
            "schedule field {key} out of u8 range: {number}"
        )));
    }
    Ok(number as u8)
}

fn format_float(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct TokenFile {
    token: String,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    access_token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_key_handles_typical_inputs() {
        assert_eq!(normalize_key("Battery SoC"), "battery_soc");
        assert_eq!(
            normalize_key("Self Consumption (kWh)"),
            "self_consumption_kwh"
        );
        assert_eq!(normalize_key("Grid-Export"), "grid_export");
        assert_eq!(normalize_key("__test__"), "test");
    }

    #[test]
    fn extract_chart_handles_links_payload() {
        let chart = json!({
            "series": {
                "links": [
                    {"id": "Grid Export", "value": 3779.0},
                    {"id": "Battery Discharge", "value": 2417.0},
                ]
            }
        });
        let mut data = QcData::new();
        extract_chart_into(&chart, "Status", &mut data);
        assert_eq!(data.status.get("grid_export_wh"), Some(&3779.0));
        assert_eq!(data.status.get("battery_discharge_wh"), Some(&2417.0));
    }

    #[test]
    fn extract_chart_handles_energy_series_list() {
        let chart = json!({
            "series": [
                {
                    "data": [
                        {"name": "Import Energy (kWh)", "value": 0.0},
                        {"name": "Export Energy (kWh)", "value": 4.6}
                    ]
                }
            ]
        });
        let mut data = QcData::new();
        extract_chart_into(&chart, "Energy", &mut data);
        assert_eq!(data.energy.get("import_energy_kwh"), Some(&0.0));
        assert_eq!(data.energy.get("export_energy_kwh"), Some(&4.6));
    }

    #[test]
    fn extract_chart_uses_widget_title_when_data_has_no_name() {
        let chart = json!({
            "series": [
                {"data": [{"value": 31.0}]}
            ]
        });
        let mut data = QcData::new();
        extract_chart_into(&chart, "Current Battery SoC", &mut data);
        assert_eq!(data.energy.get("current_battery_soc"), Some(&31.0));
    }

    #[test]
    fn build_chart_request_merges_parameters_into_datafetch() {
        let widget = DashboardWidget {
            datafetch: json!({
                "fetchType": "chart",
                "parameters": {
                    "deviceId": "abc123",
                    "lookback": "1d",
                    "metric": "power"
                }
            }),
            echart_opts: json!({"title": {"text": "Power"}}),
            title: "Power".to_string(),
        };
        let body = build_chart_request(&widget).unwrap();
        let datafetch = body.get("datafetch").unwrap();
        assert_eq!(datafetch.get("fetchType").unwrap(), "chart");
        assert_eq!(datafetch.get("deviceId").unwrap(), "abc123");
        assert_eq!(datafetch.get("lookback").unwrap(), "1d");
        assert_eq!(datafetch.get("metric").unwrap(), "power");
        assert_eq!(body.get("echartOpts").unwrap(), &json!({"title": {"text": "Power"}}));
    }

    #[test]
    fn parse_daily_schedule_extracts_all_fields() {
        let body = json!({
            "filters": [
                {"output": "sched_state", "init": "1"},
                {"output": "force_charge_curr", "init": "100.0"},
                {"output": "force_discharge_curr", "init": "48.0"},
                {"output": "min_soc", "init": "15"},
                {"output": "max_soc", "init": "100"},
                {"output": "s1_mode", "init": "1"},
                {"output": "s1_starttime", "init": "02:00:00"},
                {"output": "s1_endtime", "init": "05:00:00"},
                {"output": "s2_mode", "init": "1"},
                {"output": "s2_starttime", "init": "05:00:00"},
                {"output": "s2_endtime", "init": "20:00:00"},
                {"output": "s3_mode", "init": "0"},
                {"output": "s3_starttime", "init": "00:00:00"},
                {"output": "s3_endtime", "init": "00:00:00"},
                {"output": "s4_mode", "init": "2"},
                {"output": "s4_starttime", "init": "00:00:00"},
                {"output": "s4_endtime", "init": "02:00:00"},
                {"output": "s5_mode", "init": "0"},
                {"output": "s5_starttime", "init": "00:00:00"},
                {"output": "s5_endtime", "init": "00:00:00"},
            ]
        });
        let config = parse_daily_schedule(&body).unwrap();
        assert!(config.state_enabled);
        assert_eq!(config.force_charge_current, 100.0);
        assert_eq!(config.force_discharge_current, 48.0);
        assert_eq!(config.min_soc, 15);
        assert_eq!(config.max_soc, 100);
        assert_eq!(config.schedules.len(), 5);
        assert_eq!(config.schedules[0].mode, ScheduleMode::ForcedCharge);
        assert_eq!(config.schedules[0].start_time, "02:00:00");
        assert_eq!(config.schedules[3].mode, ScheduleMode::ForcedDischarge);
        assert_eq!(config.schedules[2].mode, ScheduleMode::Disable);
    }
}
