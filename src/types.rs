use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::timeutil;

pub const DEFAULT_RUNNER_GROUP: &str = "default";
pub const HEADER_SCALE_SET_MAX_CAPACITY: &str = "X-ScaleSetMaxCapacity";
pub const API_VERSION: &str = "6.0-preview";
pub(crate) const RUNNER_ENDPOINT: &str = "_apis/distributedtask/pools/0/agents";
pub(crate) const SCALE_SET_ENDPOINT: &str = "_apis/runtime/runnerscalesets";

/// Information about the system that uses this client (surfaced in User-Agent).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub system: String,
    pub version: String,
    pub commit_sha: String,
    pub scale_set_id: i32,
    pub subsystem: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageType {
    JobAvailable,
    JobAssigned,
    JobStarted,
    JobCompleted,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct JobMessageBase {
    #[serde(default)]
    pub message_type: Option<MessageType>,
    #[serde(default)]
    pub runner_request_id: i64,
    #[serde(default)]
    pub repository_name: String,
    #[serde(default)]
    pub owner_name: String,
    #[serde(default)]
    pub job_id: String,
    #[serde(default)]
    pub job_workflow_ref: String,
    #[serde(default)]
    pub job_display_name: String,
    #[serde(default)]
    pub workflow_run_id: i64,
    #[serde(default)]
    pub event_name: String,
    #[serde(default)]
    pub request_labels: Vec<String>,
    #[serde(default, with = "timeutil::opt")]
    pub queue_time: Option<OffsetDateTime>,
    #[serde(default, with = "timeutil::opt")]
    pub scale_set_assign_time: Option<OffsetDateTime>,
    #[serde(default, with = "timeutil::opt")]
    pub runner_assign_time: Option<OffsetDateTime>,
    #[serde(default, with = "timeutil::opt")]
    pub finish_time: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct JobAvailable {
    #[serde(default)]
    pub acquire_job_url: String,
    #[serde(flatten)]
    pub base: JobMessageBase,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct JobAssigned {
    #[serde(flatten)]
    pub base: JobMessageBase,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct JobStarted {
    #[serde(default)]
    pub runner_id: i32,
    #[serde(default)]
    pub runner_name: String,
    #[serde(flatten)]
    pub base: JobMessageBase,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct JobCompleted {
    #[serde(default)]
    pub result: String,
    #[serde(default)]
    pub runner_id: i32,
    #[serde(default)]
    pub runner_name: String,
    #[serde(flatten)]
    pub base: JobMessageBase,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Label {
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunnerGroup {
    pub id: i32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: i32,
    #[serde(rename = "isDefaultGroup", default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunnerGroupList {
    #[serde(default)]
    pub count: i32,
    #[serde(rename = "value", default)]
    pub runner_groups: Vec<RunnerGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunnerSetting {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disable_update: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunnerScaleSetStatistic {
    #[serde(default)]
    pub total_available_jobs: i32,
    #[serde(default)]
    pub total_acquired_jobs: i32,
    #[serde(default)]
    pub total_assigned_jobs: i32,
    #[serde(default)]
    pub total_running_jobs: i32,
    #[serde(default)]
    pub total_registered_runners: i32,
    #[serde(default)]
    pub total_busy_runners: i32,
    #[serde(default)]
    pub total_idle_runners: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunnerScaleSet {
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub id: i32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(rename = "runnerGroupId", default, skip_serializing_if = "is_zero_i32")]
    pub runner_group_id: i32,
    #[serde(
        rename = "runnerGroupName",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub runner_group_name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<Label>,
    #[serde(rename = "RunnerSetting", default)]
    pub runner_setting: RunnerSetting,
    #[serde(rename = "createdOn", default, with = "timeutil::opt")]
    pub created_on: Option<OffsetDateTime>,
    #[serde(
        rename = "runnerJitConfigUrl",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub runner_jit_config_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statistics: Option<RunnerScaleSetStatistic>,
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunnerScaleSetJitRunnerSetting {
    pub name: String,
    #[serde(default)]
    pub work_folder: String,
}

#[derive(Debug, Clone, Default)]
pub struct RunnerScaleSetMessage {
    pub message_id: i32,
    pub statistics: Option<RunnerScaleSetStatistic>,
    pub job_available_messages: Vec<JobAvailable>,
    pub job_assigned_messages: Vec<JobAssigned>,
    pub job_started_messages: Vec<JobStarted>,
    pub job_completed_messages: Vec<JobCompleted>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunnerScaleSetSession {
    #[serde(default)]
    pub session_id: Uuid,
    #[serde(default)]
    pub owner_name: String,
    #[serde(default)]
    pub runner_scale_set: Option<RunnerScaleSet>,
    #[serde(default)]
    pub message_queue_url: String,
    #[serde(default)]
    pub message_queue_access_token: String,
    #[serde(default)]
    pub statistics: Option<RunnerScaleSetStatistic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunnerReference {
    pub id: i32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub runner_scale_set_id: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunnerReferenceList {
    #[serde(default)]
    pub count: i32,
    #[serde(rename = "value", default)]
    pub runner_references: Vec<RunnerReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunnerScaleSetJitRunnerConfig {
    #[serde(default)]
    pub runner: Option<RunnerReference>,
    #[serde(rename = "encodedJITConfig", default)]
    pub encoded_jit_config: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RunnerScaleSetMessageResponse {
    #[serde(rename = "messageId")]
    pub message_id: i32,
    #[serde(rename = "messageType", default)]
    pub message_type: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub statistics: Option<RunnerScaleSetStatistic>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RunnerScaleSetsResponse {
    #[serde(default)]
    pub count: i32,
    #[serde(rename = "value", default)]
    pub runner_scale_sets: Vec<RunnerScaleSet>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AcquireJobsResponse {
    #[serde(default)]
    pub count: i32,
    #[serde(default)]
    pub value: Vec<i64>,
}

#[derive(Debug, Deserialize)]
struct EnvelopeType {
    #[serde(rename = "messageType")]
    message_type: Option<MessageType>,
}

pub(crate) fn parse_runner_scale_set_message(
    response: RunnerScaleSetMessageResponse,
) -> Result<RunnerScaleSetMessage> {
    if response.message_type != "RunnerScaleSetJobMessages" {
        return Err(Error::message(format!(
            "unsupported message type: {}",
            response.message_type
        )));
    }

    let mut message = RunnerScaleSetMessage {
        message_id: response.message_id,
        statistics: response.statistics,
        ..Default::default()
    };

    if response.body.is_empty() {
        return Ok(message);
    }

    let batched: Vec<serde_json::Value> = serde_json::from_str(&response.body)?;
    for msg in batched {
        let envelope: EnvelopeType = serde_json::from_value(msg.clone())?;
        match envelope.message_type {
            Some(MessageType::JobAvailable) => {
                message
                    .job_available_messages
                    .push(serde_json::from_value(msg)?);
            }
            Some(MessageType::JobAssigned) => {
                message
                    .job_assigned_messages
                    .push(serde_json::from_value(msg)?);
            }
            Some(MessageType::JobStarted) => {
                message
                    .job_started_messages
                    .push(serde_json::from_value(msg)?);
            }
            Some(MessageType::JobCompleted) => {
                message
                    .job_completed_messages
                    .push(serde_json::from_value(msg)?);
            }
            Some(MessageType::Unknown) | None => {}
        }
    }

    Ok(message)
}

pub(crate) fn apply_default_label_types(scale_set: &mut RunnerScaleSet) {
    for label in &mut scale_set.labels {
        if label.r#type.is_empty() {
            label.r#type = "System".into();
        }
    }
}

pub(crate) fn ensure_labels(scale_set: &mut RunnerScaleSet) -> Result<()> {
    if !scale_set.labels.is_empty() {
        return Ok(());
    }
    if scale_set.name.is_empty() {
        return Err(Error::message(
            "runner scale set must have a name or at least one label",
        ));
    }
    scale_set.labels = vec![Label {
        name: scale_set.name.clone(),
        r#type: "System".into(),
    }];
    Ok(())
}

pub fn parse_message_json(bytes: &[u8]) -> Result<RunnerScaleSetMessage> {
    let response: RunnerScaleSetMessageResponse = serde_json::from_slice(bytes)?;
    parse_runner_scale_set_message(response)
}
