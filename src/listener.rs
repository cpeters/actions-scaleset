//! Message listener with correct acknowledgment ordering.
//!
//! The Go `listener` package deletes a message *before* `AcquireJobs` and
//! scaler callbacks. If those later steps fail, the message is gone and will
//! never be redelivered.
//!
//! This listener acknowledges **after** acquisition and handlers succeed
//! (`AckMode::AfterProcess`, the default). `AckMode::GoCompat` is available
//! only for reproducing the upstream bug.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::session::SessionApi;
use crate::types::{
    JobCompleted, JobStarted, RunnerScaleSetMessage, RunnerScaleSetSession, RunnerScaleSetStatistic,
};

/// When the queue item is deleted (acked).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AckMode {
    /// Delete only after acquire + scaler handlers succeed.
    #[default]
    AfterProcess,
    /// Upstream Go listener order: delete immediately, then process.
    /// Unsafe — included so callers can A/B the bug.
    GoCompat,
}

#[derive(Debug, Clone)]
pub struct ListenerConfig {
    pub scale_set_id: i32,
    pub max_runners: i32,
    pub ack_mode: AckMode,
}

impl Default for ListenerConfig {
    fn default() -> Self {
        Self {
            scale_set_id: 0,
            max_runners: 0,
            ack_mode: AckMode::AfterProcess,
        }
    }
}

impl ListenerConfig {
    pub fn validate(&self) -> Result<()> {
        if self.scale_set_id == 0 {
            return Err(Error::message("scaleSetID is required"));
        }
        if self.max_runners < 0 {
            return Err(Error::message("maxRunners must be >= 0"));
        }
        Ok(())
    }
}

#[async_trait]
pub trait Scaler: Send + Sync {
    async fn handle_job_started(&self, job: &JobStarted) -> Result<()>;
    async fn handle_job_completed(&self, job: &JobCompleted) -> Result<()>;
    /// Return the runner count actually applied.
    async fn handle_desired_runner_count(&self, count: i32) -> Result<i32>;
}

#[async_trait]
pub trait MetricsRecorder: Send + Sync {
    fn record_statistics(&self, statistics: &RunnerScaleSetStatistic);
    fn record_job_started(&self, job: &JobStarted);
    fn record_job_completed(&self, job: &JobCompleted);
    fn record_desired_runners(&self, count: i32);
}

pub struct NoopMetrics;

impl MetricsRecorder for NoopMetrics {
    fn record_statistics(&self, _: &RunnerScaleSetStatistic) {}
    fn record_job_started(&self, _: &JobStarted) {}
    fn record_job_completed(&self, _: &JobCompleted) {}
    fn record_desired_runners(&self, _: i32) {}
}

pub struct Listener<S> {
    session: S,
    max_runners: AtomicI32,
    ack_mode: AckMode,
    metrics: Arc<dyn MetricsRecorder>,
    latest_statistics: std::sync::Mutex<Option<RunnerScaleSetStatistic>>,
}

impl<S: SessionApi> Listener<S> {
    pub fn new(session: S, config: ListenerConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            session,
            max_runners: AtomicI32::new(config.max_runners),
            ack_mode: config.ack_mode,
            metrics: Arc::new(NoopMetrics),
            latest_statistics: std::sync::Mutex::new(None),
        })
    }

    pub fn with_metrics(mut self, metrics: Arc<dyn MetricsRecorder>) -> Self {
        self.metrics = metrics;
        self
    }

    pub fn set_max_runners(&self, count: i32) -> Result<()> {
        if count < 0 {
            return Err(Error::message("maxRunners must be >= 0"));
        }
        self.max_runners.store(count, Ordering::SeqCst);

        Ok(())
    }

    /// Process a single already-fetched message (used by tests and custom loops).
    pub async fn handle_one(&self, scaler: &dyn Scaler, msg: RunnerScaleSetMessage) -> Result<()> {
        self.handle_message(scaler, msg).await
    }

    pub async fn run(&self, scaler: &dyn Scaler) -> Result<()> {
        let initial = self.session.session().await;
        self.run_with_stop(scaler, initial, None).await
    }

    /// Same as [`run`] but exits when `stop` is set (or the sender is dropped).
    pub async fn run_until(
        &self,
        scaler: &dyn Scaler,
        mut stop: watch::Receiver<bool>,
    ) -> Result<()> {
        let initial = self.session.session().await;
        self.run_with_stop(scaler, initial, Some(&mut stop)).await
    }

    async fn run_with_stop(
        &self,
        scaler: &dyn Scaler,
        initial: RunnerScaleSetSession,
        mut stop: Option<&mut watch::Receiver<bool>>,
    ) -> Result<()> {
        if initial.session_id == Uuid::nil() {
            return Err(Error::message("initial session is nil"));
        }

        let stats = initial
            .statistics
            .clone()
            .ok_or_else(|| Error::message("session statistics is nil"))?;

        self.store_statistics(&stats)?;

        tracing::info!(
            total_assigned_jobs = stats.total_assigned_jobs,
            "handling initial session statistics"
        );

        let desired = scaler
            .handle_desired_runner_count(stats.total_assigned_jobs)
            .await?;

        self.metrics.record_desired_runners(desired);

        let mut last_message_id = 0i32;

        loop {
            if let Some(rx) = stop.as_mut() {
                if *rx.borrow() {
                    return Ok(());
                }
            }

            tracing::info!(last_message_id, "getting next message");

            let get_message = self
                .session
                .get_message(last_message_id, self.max_runners.load(Ordering::SeqCst));

            let msg = match stop.as_mut() {
                Some(rx) => {
                    tokio::select! {
                        changed = rx.changed() => {
                            match changed {
                                Ok(()) if *rx.borrow() => return Ok(()),
                                Ok(()) => continue,
                                Err(_) => return Ok(()),
                            }
                        }
                        result = get_message => result,
                    }
                }
                None => get_message.await,
            }
            .map_err(|e| Error::message(format!("failed to get message: {e}")))?;

            match msg {
                None => {
                    let assigned = {
                        let stats = self
                            .latest_statistics
                            .lock()
                            .map_err(|_| Error::message("latest statistics lock poisoned"))?;

                        stats
                            .as_ref()
                            .ok_or_else(|| Error::message("latest statistics is nil"))?
                            .total_assigned_jobs
                    };
                    scaler.handle_desired_runner_count(assigned).await?;
                }
                Some(msg) => {
                    let message_id = msg.message_id;
                    self.handle_message(scaler, msg).await?;
                    last_message_id = message_id;
                }
            }
        }
    }

    async fn handle_message(&self, scaler: &dyn Scaler, msg: RunnerScaleSetMessage) -> Result<()> {
        if let Some(stats) = &msg.statistics {
            self.store_statistics(stats)?;
        }

        match self.ack_mode {
            AckMode::GoCompat => {
                self.session
                    .delete_message(msg.message_id)
                    .await
                    .map_err(|e| Error::message(format!("failed to delete message: {e}")))?;
                self.process(scaler, &msg).await?;
            }
            AckMode::AfterProcess => {
                self.process(scaler, &msg).await?;
                self.session
                    .delete_message(msg.message_id)
                    .await
                    .map_err(|e| Error::message(format!("failed to delete message: {e}")))?;
            }
        }
        Ok(())
    }

    async fn process(&self, scaler: &dyn Scaler, msg: &RunnerScaleSetMessage) -> Result<()> {
        let stats = msg
            .statistics
            .as_ref()
            .ok_or_else(|| Error::message("message statistics is nil"))?;

        let assigned = stats.total_assigned_jobs;

        if !msg.job_available_messages.is_empty() {
            let ids: Vec<i64> = msg
                .job_available_messages
                .iter()
                .map(|j| j.base.runner_request_id)
                .collect();
            tracing::info!(count = ids.len(), "acquiring jobs");
            let acquired =
                self.session.acquire_jobs(&ids).await.map_err(|e| {
                    Error::message(format!("failed to acquire available jobs: {e}"))
                })?;
            tracing::info!(count = acquired.len(), "jobs acquired");
        }

        for job in &msg.job_started_messages {
            self.metrics.record_job_started(job);
            scaler
                .handle_job_started(job)
                .await
                .map_err(|e| Error::message(format!("failed to handle job started: {e}")))?;
        }
        for job in &msg.job_completed_messages {
            self.metrics.record_job_completed(job);
            scaler
                .handle_job_completed(job)
                .await
                .map_err(|e| Error::message(format!("failed to handle job completed: {e}")))?;
        }

        let desired = scaler
            .handle_desired_runner_count(assigned)
            .await
            .map_err(|e| Error::message(format!("failed to handle desired runner count: {e}")))?;

        self.metrics.record_desired_runners(desired);

        Ok(())
    }

    fn store_statistics(&self, stats: &RunnerScaleSetStatistic) -> Result<()> {
        self.metrics.record_statistics(stats);

        let mut stored = self
            .latest_statistics
            .lock()
            .map_err(|_| Error::message("latest statistics lock poisoned"))?;

        *stored = Some(stats.clone());

        Ok(())
    }
}

/// Spawn `run` on the current runtime. The caller owns cancellation via `stop`.
pub fn spawn_listener<S>(
    listener: Arc<Listener<S>>,
    scaler: Arc<dyn Scaler>,
    stop: watch::Receiver<bool>,
) -> JoinHandle<Result<()>>
where
    S: SessionApi + 'static,
{
    tokio::spawn(async move { listener.run_until(scaler.as_ref(), stop).await })
}
