use actions_scaleset::parse_message_json;

#[test]
fn parses_batched_job_messages() {
    let body = serde_json::json!([
        {
            "messageType": "JobAvailable",
            "runnerRequestId": 11,
            "jobId": "job-a",
            "acquireJobUrl": "https://example/acquire"
        },
        {
            "messageType": "JobStarted",
            "runnerRequestId": 11,
            "runnerId": 7,
            "runnerName": "vpod-7",
            "jobId": "job-a"
        },
        {
            "messageType": "JobCompleted",
            "runnerRequestId": 11,
            "result": "succeeded",
            "runnerId": 7,
            "jobId": "job-a"
        }
    ]);
    let envelope = serde_json::json!({
        "messageId": 42,
        "messageType": "RunnerScaleSetJobMessages",
        "body": body.to_string(),
        "statistics": { "totalAssignedJobs": 3, "totalAvailableJobs": 1 }
    });
    let msg = parse_message_json(envelope.to_string().as_bytes()).expect("parse");
    assert_eq!(msg.message_id, 42);
    assert_eq!(msg.job_available_messages.len(), 1);
    assert_eq!(msg.job_started_messages.len(), 1);
    assert_eq!(msg.job_completed_messages.len(), 1);
    assert_eq!(
        msg.statistics.as_ref().unwrap().total_assigned_jobs,
        3
    );
    assert_eq!(
        msg.job_available_messages[0].base.runner_request_id,
        11
    );
}

#[test]
fn rejects_unknown_envelope_type() {
    let envelope = serde_json::json!({
        "messageId": 1,
        "messageType": "Nope",
        "body": "[]"
    });
    assert!(parse_message_json(envelope.to_string().as_bytes()).is_err());
}
