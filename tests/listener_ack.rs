use std::sync::{Arc, Mutex};

use actions_scaleset::{
    Error, JobAvailable, JobCompleted, JobMessageBase, JobStarted, Listener, ListenerConfig,
    Result, RunnerScaleSetMessage, RunnerScaleSetSession, RunnerScaleSetStatistic, Scaler,
    SessionApi,
};
use async_trait::async_trait;
use uuid::Uuid;

#[derive(Default)]
struct Trace {
    calls: Mutex<Vec<String>>,
    fail_acquire: bool,
    fail_desired: bool,
}

struct MockSession {
    trace: Arc<Trace>,
    session: RunnerScaleSetSession,
}

#[async_trait]
impl SessionApi for MockSession {
    async fn get_message(
        &self,
        _last_message_id: i32,
        _max_capacity: i32,
    ) -> Result<Option<RunnerScaleSetMessage>> {
        Ok(None)
    }

    async fn delete_message(&self, message_id: i32) -> Result<()> {
        self.trace
            .calls
            .lock()
            .unwrap()
            .push(format!("delete:{message_id}"));
        Ok(())
    }

    async fn acquire_jobs(&self, request_ids: &[i64]) -> Result<Vec<i64>> {
        self.trace
            .calls
            .lock()
            .unwrap()
            .push(format!("acquire:{request_ids:?}"));
        if self.trace.fail_acquire {
            return Err(Error::message("acquire failed"));
        }
        Ok(request_ids.to_vec())
    }

    async fn session(&self) -> RunnerScaleSetSession {
        self.session.clone()
    }
}

struct RecordingScaler {
    trace: Arc<Trace>,
}

#[async_trait]
impl Scaler for RecordingScaler {
    async fn handle_job_started(&self, job: &JobStarted) -> Result<()> {
        self.trace
            .calls
            .lock()
            .unwrap()
            .push(format!("started:{}", job.base.job_id));
        Ok(())
    }

    async fn handle_job_completed(&self, job: &JobCompleted) -> Result<()> {
        self.trace
            .calls
            .lock()
            .unwrap()
            .push(format!("completed:{}", job.base.job_id));
        Ok(())
    }

    async fn handle_desired_runner_count(&self, count: i32) -> Result<i32> {
        self.trace
            .calls
            .lock()
            .unwrap()
            .push(format!("desired:{count}"));
        if self.trace.fail_desired {
            return Err(Error::message("scale failed"));
        }
        Ok(count)
    }
}

fn sample_message() -> RunnerScaleSetMessage {
    RunnerScaleSetMessage {
        message_id: 99,
        statistics: Some(RunnerScaleSetStatistic {
            total_assigned_jobs: 4,
            ..Default::default()
        }),
        job_available_messages: vec![JobAvailable {
            acquire_job_url: "https://example/acq".into(),
            base: JobMessageBase {
                runner_request_id: 501,
                job_id: "job-1".into(),
                ..Default::default()
            },
        }],
        job_started_messages: vec![JobStarted {
            runner_id: 1,
            runner_name: "vpod-1".into(),
            base: JobMessageBase {
                job_id: "job-1".into(),
                ..Default::default()
            },
        }],
        job_completed_messages: vec![],
        job_assigned_messages: vec![],
    }
}

fn listener(trace: Arc<Trace>) -> Listener<MockSession> {
    let mut session = RunnerScaleSetSession {
        session_id: Uuid::new_v4(),
        owner_name: "test".into(),
        ..Default::default()
    };

    session.statistics = Some(RunnerScaleSetStatistic::default());

    Listener::new(
        MockSession { trace, session },
        ListenerConfig {
            scale_set_id: 1,
            max_runners: 8,
        },
    )
    .unwrap()
}

#[test]
fn set_max_runners_rejects_negative_values() {
    let trace = Arc::new(Trace::default());
    let listener = listener(trace);

    assert!(listener.set_max_runners(-1).is_err());
}

#[test]
fn set_max_runners_accepts_non_negative_values() {
    let trace = Arc::new(Trace::default());
    let listener = listener(trace);

    assert!(listener.set_max_runners(0).is_ok());
    assert!(listener.set_max_runners(10).is_ok());
}

#[tokio::test]
async fn after_process_deletes_last() {
    let trace = Arc::new(Trace::default());
    let listener = listener(trace.clone());
    let scaler = RecordingScaler {
        trace: trace.clone(),
    };
    listener
        .handle_one(&scaler, sample_message())
        .await
        .unwrap();
    let calls = trace.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        vec![
            "acquire:[501]".to_string(),
            "started:job-1".to_string(),
            "desired:4".to_string(),
            "delete:99".to_string(),
        ]
    );
}

#[tokio::test]
async fn after_process_skips_delete_when_acquire_fails() {
    let trace = Arc::new(Trace {
        fail_acquire: true,
        ..Default::default()
    });

    let listener = listener(trace.clone());

    let scaler = RecordingScaler {
        trace: trace.clone(),
    };

    let err = listener
        .handle_one(&scaler, sample_message())
        .await
        .unwrap_err();

    assert!(err.to_string().contains("acquire"));

    let calls = trace.calls.lock().unwrap().clone();

    assert_eq!(calls, vec!["acquire:[501]".to_string()]);
    assert!(!calls.iter().any(|c| c.starts_with("delete:")));
}

#[tokio::test]
async fn after_process_skips_delete_when_desired_count_fails() {
    let trace = Arc::new(Trace {
        fail_desired: true,
        ..Default::default()
    });
    let listener = listener(trace.clone());
    let scaler = RecordingScaler {
        trace: trace.clone(),
    };
    let err = listener
        .handle_one(&scaler, sample_message())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("desired runner count"));

    let calls = trace.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        vec![
            "acquire:[501]".to_string(),
            "started:job-1".to_string(),
            "desired:4".to_string(),
        ]
    );
    assert!(!calls.iter().any(|c| c.starts_with("delete:")));
}

#[tokio::test]
async fn missing_statistics_is_not_processed_or_acked() {
    let trace = Arc::new(Trace::default());
    let listener = listener(trace.clone());
    let scaler = RecordingScaler {
        trace: trace.clone(),
    };

    let mut msg = sample_message();
    msg.statistics = None;

    let err = listener.handle_one(&scaler, msg).await.unwrap_err();

    assert!(err.to_string().contains("message statistics is nil"));
    assert!(trace.calls.lock().unwrap().is_empty());
}

struct BlockingSession {
    session: RunnerScaleSetSession,
}

#[async_trait]
impl SessionApi for BlockingSession {
    async fn get_message(
        &self,
        _last_message_id: i32,
        _max_capacity: i32,
    ) -> Result<Option<RunnerScaleSetMessage>> {
        std::future::pending().await
    }

    async fn delete_message(&self, _message_id: i32) -> Result<()> {
        Ok(())
    }

    async fn acquire_jobs(&self, _request_ids: &[i64]) -> Result<Vec<i64>> {
        Ok(vec![])
    }

    async fn session(&self) -> RunnerScaleSetSession {
        self.session.clone()
    }
}

#[tokio::test]
async fn run_until_interrupts_pending_get_message() {
    let trace = Arc::new(Trace::default());

    let session = RunnerScaleSetSession {
        session_id: Uuid::new_v4(),
        statistics: Some(RunnerScaleSetStatistic::default()),
        ..Default::default()
    };

    let listener = Listener::new(
        BlockingSession { session },
        ListenerConfig {
            scale_set_id: 1,
            max_runners: 8,
        },
    )
    .unwrap();

    let scaler = RecordingScaler { trace };
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);

    let run = listener.run_until(&scaler, stop_rx);

    tokio::pin!(run);

    tokio::select! {
        result = &mut run => panic!("listener exited unexpectedly: {result:?}"),
        _ = tokio::task::yield_now() => {}
    }

    stop_tx.send(true).unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(1), &mut run)
        .await
        .expect("listener did not stop while get_message was pending")
        .unwrap();
}
