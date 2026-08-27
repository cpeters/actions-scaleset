//! Rust client for the GitHub Actions **Runner Scale Set** APIs.
//!
//! Implements the core client and message-session behavior of
//! [`github.com/actions/scaleset`](https://github.com/actions/scaleset).
//! It is not a drop-in replacement for every upstream HTTP transport or
//! diagnostic option.
//!
//! One behavior intentionally differs from upstream: [`listener`] acknowledges
//! messages **after** job acquisition and scaler callbacks succeed, so a
//! processing failure leaves the message available for redelivery.
//!
pub mod auth;
pub mod client;
pub mod config;
pub mod error;
pub mod http;
pub mod listener;
pub mod session;
pub mod timeutil;
pub mod types;

pub use auth::GitHubAppAuth;
pub use client::{Client, GitHubAppClientConfig, PersonalAccessTokenConfig};
pub use config::{GitHubConfig, GitHubScope};
pub use error::{Error, Kind, Result};
pub use http::HttpOptions;
pub use listener::{Listener, ListenerConfig, MetricsRecorder, NoopMetrics, Scaler};
pub use session::{MessageSessionClient, SessionApi};
pub use types::{
    parse_message_json, JobAssigned, JobAvailable, JobCompleted, JobMessageBase, JobStarted, Label,
    MessageType, RunnerGroup, RunnerReference, RunnerScaleSet, RunnerScaleSetJitRunnerConfig,
    RunnerScaleSetJitRunnerSetting, RunnerScaleSetMessage, RunnerScaleSetSession,
    RunnerScaleSetStatistic, RunnerSetting, SystemInfo, DEFAULT_RUNNER_GROUP,
    HEADER_SCALE_SET_MAX_CAPACITY,
};
