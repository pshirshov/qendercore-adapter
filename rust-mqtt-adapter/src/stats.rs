use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct RuntimeStats {
    started_at: Instant,
    successful_polls: AtomicU64,
    failed_polls: AtomicU64,
    schedule_fetches: AtomicU64,
    schedule_writes: AtomicU64,
    mqtt_messages_sent: AtomicU64,
    commands_received: AtomicU64,
    recoveries: AtomicU64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsSnapshot {
    pub uptime: Duration,
    pub successful_polls: u64,
    pub failed_polls: u64,
    pub schedule_fetches: u64,
    pub schedule_writes: u64,
    pub mqtt_messages_sent: u64,
    pub commands_received: u64,
    pub recoveries: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsDelta {
    pub successful_polls: u64,
    pub failed_polls: u64,
    pub schedule_fetches: u64,
    pub schedule_writes: u64,
    pub mqtt_messages_sent: u64,
    pub commands_received: u64,
    pub recoveries: u64,
}

impl RuntimeStats {
    pub fn new_shared() -> Arc<Self> {
        Arc::new(Self {
            started_at: Instant::now(),
            successful_polls: AtomicU64::new(0),
            failed_polls: AtomicU64::new(0),
            schedule_fetches: AtomicU64::new(0),
            schedule_writes: AtomicU64::new(0),
            mqtt_messages_sent: AtomicU64::new(0),
            commands_received: AtomicU64::new(0),
            recoveries: AtomicU64::new(0),
        })
    }

    pub fn spawn_reporter(self: &Arc<Self>, interval: Duration) {
        let stats = Arc::clone(self);
        thread::spawn(move || {
            let mut previous = stats.snapshot();
            loop {
                thread::sleep(interval);
                let current = stats.snapshot();
                let delta = current.delta_from(&previous);
                eprintln!("{}", format_summary(&current, &delta));
                previous = current;
            }
        });
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            uptime: self.started_at.elapsed(),
            successful_polls: self.successful_polls.load(Ordering::Relaxed),
            failed_polls: self.failed_polls.load(Ordering::Relaxed),
            schedule_fetches: self.schedule_fetches.load(Ordering::Relaxed),
            schedule_writes: self.schedule_writes.load(Ordering::Relaxed),
            mqtt_messages_sent: self.mqtt_messages_sent.load(Ordering::Relaxed),
            commands_received: self.commands_received.load(Ordering::Relaxed),
            recoveries: self.recoveries.load(Ordering::Relaxed),
        }
    }

    pub fn record_successful_poll(&self) {
        self.successful_polls.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_failed_poll(&self) {
        self.failed_polls.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_schedule_fetch(&self) {
        self.schedule_fetches.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_schedule_write(&self) {
        self.schedule_writes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_mqtt_message_sent(&self) {
        self.mqtt_messages_sent.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_command_received(&self) {
        self.commands_received.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_recovery(&self) {
        self.recoveries.fetch_add(1, Ordering::Relaxed);
    }
}

impl StatsSnapshot {
    pub fn delta_from(&self, previous: &Self) -> StatsDelta {
        StatsDelta {
            successful_polls: self.successful_polls - previous.successful_polls,
            failed_polls: self.failed_polls - previous.failed_polls,
            schedule_fetches: self.schedule_fetches - previous.schedule_fetches,
            schedule_writes: self.schedule_writes - previous.schedule_writes,
            mqtt_messages_sent: self.mqtt_messages_sent - previous.mqtt_messages_sent,
            commands_received: self.commands_received - previous.commands_received,
            recoveries: self.recoveries - previous.recoveries,
        }
    }
}

pub fn format_summary(snapshot: &StatsSnapshot, delta: &StatsDelta) -> String {
    let mut summary = String::new();
    let _ = write!(
        summary,
        "alive: uptime={} polls_ok={} (+{}) polls_fail={} (+{}) sched_fetch={} (+{}) sched_write={} (+{}) mqtt_messages={} (+{}) commands={} (+{}) recoveries={} (+{})",
        format_duration(snapshot.uptime),
        snapshot.successful_polls,
        delta.successful_polls,
        snapshot.failed_polls,
        delta.failed_polls,
        snapshot.schedule_fetches,
        delta.schedule_fetches,
        snapshot.schedule_writes,
        delta.schedule_writes,
        snapshot.mqtt_messages_sent,
        delta.mqtt_messages_sent,
        snapshot.commands_received,
        delta.commands_received,
        snapshot.recoveries,
        delta.recoveries,
    );
    summary
}

fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{StatsDelta, StatsSnapshot, format_summary};

    #[test]
    fn format_summary_includes_totals_and_deltas() {
        let snapshot = StatsSnapshot {
            uptime: Duration::from_secs(3661),
            successful_polls: 100,
            failed_polls: 2,
            schedule_fetches: 100,
            schedule_writes: 5,
            mqtt_messages_sent: 800,
            commands_received: 5,
            recoveries: 1,
        };
        let delta = StatsDelta {
            successful_polls: 5,
            failed_polls: 0,
            schedule_fetches: 5,
            schedule_writes: 1,
            mqtt_messages_sent: 40,
            commands_received: 1,
            recoveries: 0,
        };

        let summary = format_summary(&snapshot, &delta);
        assert!(summary.contains("uptime=01:01:01"));
        assert!(summary.contains("polls_ok=100 (+5)"));
        assert!(summary.contains("commands=5 (+1)"));
    }
}
