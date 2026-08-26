use actions_scaleset::parse_message_json;

#[test]
fn unknown_job_message_type_does_not_reject_batch() {
    let body = serde_json::json!([
        {
            "messageType": "JobStarted",
            "runnerRequestId": 123,
            "runnerId": 456,
            "runnerName": "runner-1"
        },
        {
            "messageType": "SomeFutureMessageType",
            "runnerRequestId": 789
        }
    ]);

    let response = serde_json::json!({
        "messageId": 42,
        "messageType": "RunnerScaleSetJobMessages",
        "body": body.to_string()
    });

    let message = parse_message_json(response.to_string().as_bytes()).unwrap();

    assert_eq!(message.message_id, 42);
    assert_eq!(message.job_started_messages.len(), 1);
}
