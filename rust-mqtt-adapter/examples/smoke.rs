//! End-to-end smoke test that mirrors `python run.py status`.
//!
//! Reads `../auth.json` (relative to the workspace root) for credentials and
//! exercises the live Qendercore API. Useful both as a quick liveness check
//! after API changes and as a minimal usage example for the qcore client.
//!
//! Run with:
//!     cargo run --example smoke

use std::path::PathBuf;
use std::time::Duration;

use qendercore_mqtt_adapter::config::QcoreConfig;
use qendercore_mqtt_adapter::qcore::QcoreClient;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Auth {
    login: String,
    password: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let auth_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "../auth.json".to_string());
    let raw = std::fs::read_to_string(&auth_path)?;
    let auth: Auth = serde_json::from_str(&raw)?;

    let config = QcoreConfig {
        api_url: "https://auth.qendercore.com:8000/v1".to_string(),
        login: auth.login,
        password: auth.password,
        cache_dir: PathBuf::from("../.cache"),
        http_timeout: Duration::from_secs(15),
    };

    let client = QcoreClient::new(config)?;

    println!("=== Current Status ===\n");
    let data = client.fetch_qc_data()?;
    for (k, v) in &data.status {
        println!("{k}: {v}");
    }

    println!("\n=== Energy ===\n");
    for (k, v) in &data.energy {
        println!("{k}: {v}");
    }

    println!("\n=== Daily Schedule ===\n");
    let schedule = client.fetch_daily_schedule()?;
    println!(
        "state_enabled = {}\nforce_charge_current = {} A\nforce_discharge_current = {} A\nmin_soc = {}%\nmax_soc = {}%",
        schedule.state_enabled,
        schedule.force_charge_current,
        schedule.force_discharge_current,
        schedule.min_soc,
        schedule.max_soc,
    );
    for (idx, slot) in schedule.schedules.iter().enumerate() {
        println!(
            "  slot {}: mode={} {} - {}",
            idx + 1,
            slot.mode,
            slot.start_time,
            slot.end_time
        );
    }

    Ok(())
}
