use crate::common::{not_implemented, Job, JobId, JobStatus, Result};
use async_trait::async_trait;

#[async_trait]
pub trait SchedulerService: Send + Sync {
    async fn enqueue(&self, job: Job) -> Result<JobId>;
    async fn cancel(&self, id: JobId) -> Result<()>;
    async fn status(&self, id: JobId) -> Result<JobStatus>;
}

pub struct StubSchedulerService;

#[async_trait]
impl SchedulerService for StubSchedulerService {
    async fn enqueue(&self, _job: Job) -> Result<JobId> {
        not_implemented("SchedulerService::enqueue")
    }
    async fn cancel(&self, _id: JobId) -> Result<()> {
        not_implemented("SchedulerService::cancel")
    }
    async fn status(&self, _id: JobId) -> Result<JobStatus> {
        not_implemented("SchedulerService::status")
    }
}

use crate::common::{DimiError, ResourceClass, SqlValue};
use crate::kernel::events::{topics, EventBus};
use crate::services::storage::StorageEngine;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Semaphore;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

type StatusMap = StdMutex<HashMap<String, JobStatus>>;

async fn insert_queued_row(
    storage: &Arc<dyn StorageEngine>,
    id: &str,
    kind: &str,
    priority: i32,
) -> Result<()> {
    storage
        .query(
            "INSERT INTO jobs (id, kind, status, priority, created_at) VALUES (?1, ?2, 'queued', ?3, ?4)",
            &[
                SqlValue::Text(id.to_string()),
                SqlValue::Text(kind.to_string()),
                SqlValue::Integer(priority as i64),
                SqlValue::Integer(now_unix()),
            ],
        )
        .await?;
    Ok(())
}

fn mark_queued(statuses: &StatusMap, events: &EventBus, id: &str, kind: &str) {
    statuses
        .lock()
        .expect("scheduler status map lock poisoned")
        .insert(id.to_string(), JobStatus::Queued);
    events.publish(topics::JOB_QUEUED, serde_json::json!({ "job_id": id, "kind": kind }));
}

async fn mark_running(
    storage: &Arc<dyn StorageEngine>,
    events: &EventBus,
    statuses: &StatusMap,
    id: &str,
    kind: &str,
) {
    statuses
        .lock()
        .expect("scheduler status map lock poisoned")
        .insert(id.to_string(), JobStatus::Running);
    events.publish(topics::JOB_STARTED, serde_json::json!({ "job_id": id, "kind": kind }));
    let _ = storage
        .query(
            "UPDATE jobs SET status = 'running' WHERE id = ?1",
            &[SqlValue::Text(id.to_string())],
        )
        .await;
}

async fn mark_completed(
    storage: &Arc<dyn StorageEngine>,
    events: &EventBus,
    statuses: &StatusMap,
    id: &str,
    kind: &str,
) {
    statuses
        .lock()
        .expect("scheduler status map lock poisoned")
        .insert(id.to_string(), JobStatus::Completed);
    let _ = storage
        .query(
            "UPDATE jobs SET status = 'completed', completed_at = ?2 WHERE id = ?1",
            &[SqlValue::Text(id.to_string()), SqlValue::Integer(now_unix())],
        )
        .await;
    events.publish(topics::JOB_COMPLETED, serde_json::json!({ "job_id": id, "kind": kind }));
}

async fn mark_failed(
    storage: &Arc<dyn StorageEngine>,
    events: &EventBus,
    statuses: &StatusMap,
    id: &str,
    kind: &str,
    error: &str,
) {
    statuses
        .lock()
        .expect("scheduler status map lock poisoned")
        .insert(id.to_string(), JobStatus::Failed);
    let _ = storage
        .query(
            "UPDATE jobs SET status = 'failed', completed_at = ?2, error_message = ?3 WHERE id = ?1",
            &[
                SqlValue::Text(id.to_string()),
                SqlValue::Integer(now_unix()),
                SqlValue::Text(error.to_string()),
            ],
        )
        .await;
    events.publish(
        topics::JOB_FAILED,
        serde_json::json!({ "job_id": id, "kind": kind, "error": error }),
    );
}

/// Caps how many finished (`completed`/`failed`) rows the `jobs` table keeps.
/// `run()` inserts one row per generate/embed/OCR/parse call — high-frequency
/// enough (a large import alone can be thousands) that without this the table
/// grows unbounded over a long session. Queued/running rows are never touched.
const MAX_RETAINED_TERMINAL_JOBS: i64 = 2000;

/// Pruning on every single terminal transition would add a DELETE to a hot
/// path that already fires per embed/OCR/parse call; this throttles it to
/// roughly one prune per batch of completions instead.
const PRUNE_EVERY_N_TERMINATIONS: u64 = 50;

async fn maybe_prune_old_jobs(storage: &Arc<dyn StorageEngine>, terminations: &std::sync::atomic::AtomicU64) {
    let count = terminations.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if !count.is_multiple_of(PRUNE_EVERY_N_TERMINATIONS) {
        return;
    }
    prune_terminal_jobs(storage, MAX_RETAINED_TERMINAL_JOBS).await;
}

async fn prune_terminal_jobs(storage: &Arc<dyn StorageEngine>, retain: i64) {
    let _ = storage
        .query(
            "DELETE FROM jobs WHERE status IN ('completed', 'failed') AND id NOT IN ( \
                 SELECT id FROM jobs WHERE status IN ('completed', 'failed') \
                 ORDER BY completed_at DESC LIMIT ?1 \
             )",
            &[SqlValue::Integer(retain)],
        )
        .await;
}

struct Lanes {
    cpu: Arc<Semaphore>,
    gpu: Arc<Semaphore>,
    io: Arc<Semaphore>,
}

pub struct TokioSchedulerService {
    storage: Arc<dyn StorageEngine>,
    events: EventBus,
    statuses: Arc<StatusMap>,
    lanes: Lanes,
    terminations: Arc<std::sync::atomic::AtomicU64>,
}

impl TokioSchedulerService {
    pub fn new(storage: Arc<dyn StorageEngine>, events: EventBus) -> Self {
        let cpu_lanes = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self {
            storage,
            events,
            statuses: Arc::new(StdMutex::new(HashMap::new())),
            lanes: Lanes {
                cpu: Arc::new(Semaphore::new(cpu_lanes)),
                gpu: Arc::new(Semaphore::new(1)),
                io: Arc::new(Semaphore::new(8)),
            },
            terminations: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    fn lane(&self, resource_class: ResourceClass) -> Arc<Semaphore> {
        match resource_class {
            ResourceClass::Cpu => self.lanes.cpu.clone(),
            ResourceClass::Gpu => self.lanes.gpu.clone(),
            ResourceClass::Io => self.lanes.io.clone(),
        }
    }

    /// Runs `work` under this scheduler's concurrency lanes: persists a queued row and
    /// publishes `job:queued`, acquires the `resource_class` permit — the actual throttle,
    /// this is where concurrent callers block until their turn — marks `Running`, runs
    /// `work` inline, then marks `Completed`/`Failed` and returns `work`'s result
    /// unchanged. No `tokio::spawn` involved, so `work` doesn't need `'static`: this call
    /// itself *is* the job, and the caller's own `.await` keeps everything alive.
    pub async fn run<T, F>(
        &self,
        kind: impl Into<String>,
        resource_class: ResourceClass,
        priority: i32,
        work: F,
    ) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>> + Send,
        T: Send,
    {
        let kind = kind.into();
        let id = JobId::new();
        let id_str = id.to_string();

        insert_queued_row(&self.storage, &id_str, &kind, priority).await?;
        mark_queued(&self.statuses, &self.events, &id_str, &kind);

        let semaphore = self.lane(resource_class);
        let _permit = semaphore
            .acquire()
            .await
            .map_err(|_| DimiError::Internal("scheduler semaphore closed".into()))?;

        mark_running(&self.storage, &self.events, &self.statuses, &id_str, &kind).await;
        let result = work.await;
        match &result {
            Ok(_) => mark_completed(&self.storage, &self.events, &self.statuses, &id_str, &kind).await,
            Err(e) => {
                mark_failed(&self.storage, &self.events, &self.statuses, &id_str, &kind, &e.to_string()).await
            }
        }
        maybe_prune_old_jobs(&self.storage, &self.terminations).await;
        result
    }
}

#[async_trait]
impl SchedulerService for TokioSchedulerService {
    async fn enqueue(&self, job: Job) -> Result<JobId> {
        let id = JobId::new();
        let id_str = id.to_string();

        insert_queued_row(&self.storage, &id_str, &job.kind, job.priority).await?;
        mark_queued(&self.statuses, &self.events, &id_str, &job.kind);

        let semaphore = self.lane(job.resource_class);
        let storage = self.storage.clone();
        let events = self.events.clone();
        let statuses = self.statuses.clone();
        let terminations = self.terminations.clone();
        let kind = job.kind.clone();
        let job_id_for_task = id_str.clone();

        tokio::spawn(async move {
            let Ok(_permit) = semaphore.acquire().await else {
                return;
            };

            let was_cancelled = matches!(
                statuses
                    .lock()
                    .expect("scheduler status map lock poisoned")
                    .get(&job_id_for_task),
                Some(JobStatus::Failed)
            );
            if was_cancelled {
                return;
            }

            mark_running(&storage, &events, &statuses, &job_id_for_task, &kind).await;
            mark_completed(&storage, &events, &statuses, &job_id_for_task, &kind).await;
            maybe_prune_old_jobs(&storage, &terminations).await;
        });

        Ok(id)
    }

    async fn cancel(&self, id: JobId) -> Result<()> {
        let id_str = id.to_string();
        let was_queued = {
            let mut guard = self
                .statuses
                .lock()
                .expect("scheduler status map lock poisoned");
            let queued = matches!(guard.get(&id_str), Some(JobStatus::Queued));
            if queued {
                guard.insert(id_str.clone(), JobStatus::Failed);
            }
            queued
        };
        if was_queued {
            self.storage
                .query(
                    "UPDATE jobs SET status = 'failed' WHERE id = ?1",
                    &[SqlValue::Text(id_str)],
                )
                .await?;
        }
        Ok(())
    }

    async fn status(&self, id: JobId) -> Result<JobStatus> {
        let id_str = id.to_string();
        if let Some(status) = self
            .statuses
            .lock()
            .expect("scheduler status map lock poisoned")
            .get(&id_str)
            .copied()
        {
            return Ok(status);
        }
        let rows = self
            .storage
            .query(
                "SELECT status FROM jobs WHERE id = ?1",
                &[SqlValue::Text(id_str.clone())],
            )
            .await?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| crate::common::DimiError::NotFound(format!("job {id}")))?;
        Ok(match row.0.get("status") {
            Some(SqlValue::Text(s)) => match s.as_str() {
                "queued" => JobStatus::Queued,
                "running" => JobStatus::Running,
                "completed" => JobStatus::Completed,
                _ => JobStatus::Failed,
            },
            _ => JobStatus::Failed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::storage::SqliteStorageEngine;
    use std::sync::atomic::{AtomicI64, Ordering};

    fn test_scheduler() -> TokioSchedulerService {
        let storage: Arc<dyn StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        TokioSchedulerService::new(storage, EventBus::new())
    }

    #[tokio::test]
    async fn enqueue_runs_to_completion() {
        let storage: Arc<dyn StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        let scheduler = TokioSchedulerService::new(storage, EventBus::new());

        let id = scheduler
            .enqueue(Job {
                kind: "test-job".to_string(),
                priority: 0,
                resource_class: ResourceClass::Cpu,
                payload: serde_json::json!({}),
            })
            .await
            .unwrap();

        for _ in 0..50 {
            if matches!(scheduler.status(id).await.unwrap(), JobStatus::Completed) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("job never reached Completed status");
    }

    #[tokio::test]
    async fn cancel_before_start_marks_failed() {
        let storage: Arc<dyn StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        let scheduler = TokioSchedulerService::new(storage, EventBus::new());

        let cpu_lanes = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let mut permits = Vec::new();
        for _ in 0..cpu_lanes {
            permits.push(scheduler.lanes.cpu.clone().acquire_owned().await.unwrap());
        }

        let id = scheduler
            .enqueue(Job {
                kind: "test-job".to_string(),
                priority: 0,
                resource_class: ResourceClass::Cpu,
                payload: serde_json::json!({}),
            })
            .await
            .unwrap();

        scheduler.cancel(id).await.unwrap();
        assert_eq!(scheduler.status(id).await.unwrap(), JobStatus::Failed);

        drop(permits);
    }

    #[tokio::test]
    async fn run_executes_work_and_returns_its_result() {
        let scheduler = test_scheduler();

        let result = scheduler
            .run("test.compute", ResourceClass::Cpu, 0, async { Ok(21 * 2) })
            .await
            .unwrap();

        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn run_propagates_errors_from_work_unchanged() {
        let scheduler = test_scheduler();

        let result: Result<()> = scheduler
            .run("test.fail", ResourceClass::Cpu, 0, async {
                Err(DimiError::NotFound("widget".into()))
            })
            .await;

        assert!(matches!(result, Err(DimiError::NotFound(msg)) if msg == "widget"));
    }

    #[tokio::test]
    async fn run_serializes_within_a_lane() {
        let scheduler = Arc::new(test_scheduler());
        let active = Arc::new(AtomicI64::new(0));
        let max_observed = Arc::new(AtomicI64::new(0));

        let mut handles = Vec::new();
        for _ in 0..5 {
            let scheduler = scheduler.clone();
            let active = active.clone();
            let max_observed = max_observed.clone();
            handles.push(tokio::spawn(async move {
                scheduler
                    .run("test.gpu_work", ResourceClass::Gpu, 0, async {
                        let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                        max_observed.fetch_max(now, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        active.fetch_sub(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .await
            }));
        }
        for h in handles {
            h.await.unwrap().unwrap();
        }

        assert_eq!(
            max_observed.load(Ordering::SeqCst),
            1,
            "GPU lane has capacity 1, so no two run() calls should ever overlap"
        );
    }

    #[tokio::test]
    async fn mark_failed_persists_the_error_message() {
        let scheduler = test_scheduler();

        let result: Result<()> = scheduler
            .run("test.fail", ResourceClass::Cpu, 0, async {
                Err(DimiError::Internal("boom".into()))
            })
            .await;
        assert!(result.is_err());

        let rows = scheduler
            .storage
            .query("SELECT error_message FROM jobs WHERE kind = 'test.fail'", &[])
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        let Some(SqlValue::Text(msg)) = rows[0].0.get("error_message") else {
            panic!("expected a persisted error_message");
        };
        assert!(msg.contains("boom"));
    }

    #[tokio::test]
    async fn prune_terminal_jobs_keeps_only_the_most_recent_and_spares_active_ones() {
        let scheduler = test_scheduler();

        for i in 0..10 {
            scheduler
                .storage
                .query(
                    "INSERT INTO jobs (id, kind, status, priority, created_at, completed_at) \
                     VALUES (?1, 'test', 'completed', 0, ?2, ?2)",
                    &[
                        SqlValue::Text(format!("done-{i}")),
                        SqlValue::Integer(i),
                    ],
                )
                .await
                .unwrap();
        }
        scheduler
            .storage
            .query(
                "INSERT INTO jobs (id, kind, status, priority, created_at) VALUES ('still-queued', 'test', 'queued', 0, 0)",
                &[],
            )
            .await
            .unwrap();

        prune_terminal_jobs(&scheduler.storage, 3).await;

        let remaining = scheduler
            .storage
            .query("SELECT id, completed_at FROM jobs WHERE status = 'completed'", &[])
            .await
            .unwrap();
        assert_eq!(remaining.len(), 3, "should keep exactly `retain` completed rows");
        for row in &remaining {
            let Some(SqlValue::Integer(completed_at)) = row.0.get("completed_at") else {
                panic!("missing completed_at");
            };
            assert!(*completed_at >= 7, "should keep the most recent rows, not the oldest");
        }

        let still_queued = scheduler
            .storage
            .query("SELECT id FROM jobs WHERE id = 'still-queued'", &[])
            .await
            .unwrap();
        assert_eq!(still_queued.len(), 1, "queued jobs must never be pruned");
    }
}
