use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

pub const SCHEDULE_COUNT: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
pub enum ScheduleMode {
    Disable,
    ForcedCharge,
    ForcedDischarge,
}

impl ScheduleMode {
    pub const ALL: [ScheduleMode; 3] = [
        ScheduleMode::Disable,
        ScheduleMode::ForcedCharge,
        ScheduleMode::ForcedDischarge,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            ScheduleMode::Disable => "Disable",
            ScheduleMode::ForcedCharge => "Forced Charge",
            ScheduleMode::ForcedDischarge => "Forced Discharge",
        }
    }

    pub fn from_display_name(name: &str) -> AppResult<Self> {
        match name {
            "Disable" => Ok(ScheduleMode::Disable),
            "Forced Charge" => Ok(ScheduleMode::ForcedCharge),
            "Forced Discharge" => Ok(ScheduleMode::ForcedDischarge),
            other => Err(AppError::InvalidState(format!(
                "unknown schedule mode display name: {other}"
            ))),
        }
    }

    pub fn as_api_int(self) -> u8 {
        u8::from(self)
    }
}

impl From<ScheduleMode> for u8 {
    fn from(mode: ScheduleMode) -> Self {
        match mode {
            ScheduleMode::Disable => 0,
            ScheduleMode::ForcedCharge => 1,
            ScheduleMode::ForcedDischarge => 2,
        }
    }
}

impl TryFrom<u8> for ScheduleMode {
    type Error = AppError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(ScheduleMode::Disable),
            1 => Ok(ScheduleMode::ForcedCharge),
            2 => Ok(ScheduleMode::ForcedDischarge),
            other => Err(AppError::InvalidState(format!(
                "unknown schedule mode value: {other}"
            ))),
        }
    }
}

impl fmt::Display for ScheduleMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub mode: ScheduleMode,
    pub start_time: String,
    pub end_time: String,
}

impl Schedule {
    pub fn disabled() -> Self {
        Self {
            mode: ScheduleMode::Disable,
            start_time: "00:00:00".to_string(),
            end_time: "00:00:00".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DailyScheduleConfig {
    pub state_enabled: bool,
    pub force_charge_current: f64,
    pub force_discharge_current: f64,
    pub min_soc: u8,
    pub max_soc: u8,
    pub schedules: Vec<Schedule>,
}

impl DailyScheduleConfig {
    pub fn ensure_slot(&mut self, slot: usize) -> AppResult<()> {
        if slot == 0 || slot > SCHEDULE_COUNT {
            return Err(AppError::InvalidState(format!(
                "schedule slot {slot} out of range 1..={SCHEDULE_COUNT}"
            )));
        }
        while self.schedules.len() < slot {
            self.schedules.push(Schedule::disabled());
        }
        Ok(())
    }
}

/// Per-update view of dashboard data extracted from the Qendercore API.
#[derive(Debug, Clone, Default)]
pub struct QcData {
    pub status: BTreeMap<String, f64>,
    pub energy: BTreeMap<String, f64>,
}

impl QcData {
    pub fn new() -> Self {
        Self::default()
    }
}
