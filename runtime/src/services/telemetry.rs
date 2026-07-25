use crate::common::{not_implemented, ProcessMemory, Result, SystemMemory, TelemetrySnapshot};
use async_trait::async_trait;

#[async_trait]
pub trait TelemetryService: Send + Sync {
    fn record_metric(&self, name: &str, value: f64, tags: &[(&str, &str)]);
    fn record_event(&self, name: &str, payload: serde_json::Value);
    async fn snapshot(&self) -> Result<TelemetrySnapshot>;
    async fn system_memory(&self) -> Result<SystemMemory>;
    async fn top_memory_consumers(&self, limit: usize) -> Result<Vec<ProcessMemory>>;
}

pub struct StubTelemetryService;

#[async_trait]
impl TelemetryService for StubTelemetryService {
    fn record_metric(&self, _name: &str, _value: f64, _tags: &[(&str, &str)]) {}
    fn record_event(&self, _name: &str, _payload: serde_json::Value) {}
    async fn snapshot(&self) -> Result<TelemetrySnapshot> {
        not_implemented("TelemetryService::snapshot")
    }
    async fn system_memory(&self) -> Result<SystemMemory> {
        not_implemented("TelemetryService::system_memory")
    }
    async fn top_memory_consumers(&self, _limit: usize) -> Result<Vec<ProcessMemory>> {
        not_implemented("TelemetryService::top_memory_consumers")
    }
}

use crate::common::{DimiError, SqlValue};
use crate::services::storage::StorageEngine;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use sysinfo::System;

const MAX_LOGGED_EVENTS: usize = 500;

pub struct DefaultTelemetryService {
    metrics: StdMutex<HashMap<String, f64>>,
    events_log: StdMutex<Vec<(String, serde_json::Value)>>,
    system: StdMutex<System>,
    storage: Arc<dyn StorageEngine>,
}

impl DefaultTelemetryService {
    pub fn new(storage: Arc<dyn StorageEngine>) -> Self {
        Self {
            metrics: StdMutex::new(HashMap::new()),
            events_log: StdMutex::new(Vec::new()),
            system: StdMutex::new(System::new_all()),
            storage,
        }
    }
}

#[async_trait]
impl TelemetryService for DefaultTelemetryService {
    fn record_metric(&self, name: &str, value: f64, _tags: &[(&str, &str)]) {
        self.metrics
            .lock()
            .expect("telemetry metrics lock poisoned")
            .insert(name.to_string(), value);
    }

    fn record_event(&self, name: &str, payload: serde_json::Value) {
        let mut log = self
            .events_log
            .lock()
            .expect("telemetry events lock poisoned");
        log.push((name.to_string(), payload));
        if log.len() > MAX_LOGGED_EVENTS {
            let excess = log.len() - MAX_LOGGED_EVENTS;
            log.drain(0..excess);
        }
    }

    async fn snapshot(&self) -> Result<TelemetrySnapshot> {
        let (ram_bytes, cpu_percent) = {
            let mut sys = self.system.lock().expect("telemetry system lock poisoned");
            sys.refresh_all();
            let pid = sysinfo::get_current_pid()
                .map_err(|e| DimiError::Internal(format!("failed to get current pid: {e}")))?;
            let ram = sys.processes().get(&pid).map(|p| p.memory()).unwrap_or(0);
            let cpu = sys.global_cpu_usage();
            (ram, cpu)
        };

        let tokens_per_sec = self
            .metrics
            .lock()
            .expect("telemetry metrics lock poisoned")
            .get("inference.tokens_per_sec")
            .map(|v| *v as f32);

        let job_queue_depth = self
            .storage
            .query(
                "SELECT COUNT(*) AS n FROM jobs WHERE status IN ('queued', 'running')",
                &[],
            )
            .await
            .ok()
            .and_then(|rows| rows.into_iter().next())
            .and_then(|row| match row.0.get("n") {
                Some(SqlValue::Integer(n)) => Some(*n as u64),
                _ => None,
            })
            .unwrap_or(0);

        Ok(TelemetrySnapshot {
            ram_bytes,
            cpu_percent,
            cpu_temp_celsius: crate::kernel::hardware::current_cpu_temp_celsius(),
            tokens_per_sec,
            job_queue_depth,
        })
    }

    async fn system_memory(&self) -> Result<SystemMemory> {
        Ok(SystemMemory {
            total_bytes: crate::kernel::hardware::total_ram_bytes(),
            available_bytes: crate::kernel::hardware::available_ram_bytes(),
        })
    }

    async fn top_memory_consumers(&self, limit: usize) -> Result<Vec<ProcessMemory>> {
        Ok(crate::kernel::hardware::top_memory_consumers(limit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_and_reports_real_process_memory() {
        let storage: Arc<dyn StorageEngine> =
            Arc::new(crate::services::storage::SqliteStorageEngine::open_in_memory_for_test().unwrap());
        let telemetry = DefaultTelemetryService::new(storage);
        telemetry.record_metric("inference.tokens_per_sec", 12.5, &[]);
        telemetry.record_event("test.event", serde_json::json!({ "ok": true }));

        let snapshot = telemetry.snapshot().await.unwrap();
        assert!(
            snapshot.ram_bytes > 0,
            "a running process should report nonzero RSS"
        );
        assert_eq!(snapshot.tokens_per_sec, Some(12.5));
        assert_eq!(snapshot.job_queue_depth, 0);
    }

    #[tokio::test]
    async fn job_queue_depth_counts_queued_and_running_jobs_only() {
        let storage: Arc<dyn StorageEngine> =
            Arc::new(crate::services::storage::SqliteStorageEngine::open_in_memory_for_test().unwrap());
        let telemetry = DefaultTelemetryService::new(storage.clone());

        for (id, status) in [("a", "queued"), ("b", "running"), ("c", "completed")] {
            storage
                .query(
                    "INSERT INTO jobs (id, kind, status, priority, created_at) VALUES (?1, 'test', ?2, 0, 0)",
                    &[SqlValue::Text(id.into()), SqlValue::Text(status.into())],
                )
                .await
                .unwrap();
        }

        let snapshot = telemetry.snapshot().await.unwrap();
        assert_eq!(snapshot.job_queue_depth, 2, "only queued+running should count, not completed");
    }
}
