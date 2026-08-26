//! Rust client for the GitHub Actions **Runner Scale Set** APIs.
//!
//! Port of [`github.com/actions/scaleset`](https://github.com/actions/scaleset)
//! with one intentional divergence: [`listener`] acknowledges messages
//! **after** job acquisition and scaler callbacks succeed, so a crash mid-handle
//! redelivers the message instead of dropping it.
//!
//! ```ignore
//! use actions_scaleset::{
//!     Client, GitHubAppAuth, GitHubAppClientConfig, HttpOptions, Listener, ListenerConfig,
//!     RunnerScaleSet, Scaler, SystemInfo,
//! };
//!
//! let client = Client::with_github_app(
//!     GitHubAppClientConfig {
//!         github_config_url: "https://github.com/org/repo".into(),
//!         github_app_auth: GitHubAppAuth {
//!             client_id: "...".into(),
//!             installation_id: 1,
//!             private_key_pem: std::fs::read_to_string("app.pem")?,
//!         },
//!         system_info: SystemInfo {
//!             system: "vks-operator".into(),
//!             version: "0.1.0".into(),
//!             commit_sha: "dev".into(),
//!             scale_set_id: 0,
//!             subsystem: "listener".into(),
//!         },
//!     },
//!     HttpOptions::default(),
//! )?;
//!
//! let created = client
//!     .create_runner_scale_set(RunnerScaleSet { name: "vks-builders".into(), ..Default::default() })
//!     .await?;
//! let session = client.message_session_client(created.id, "vks-operator").await?;
//! let listener = Listener::new(session, ListenerConfig {
//!     scale_set_id: created.id,
//!     max_runners: 32,
//!     ack_mode: Default::default(),
//! })?;
//! listener.run(&my_scaler).await?;
//! ```

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
pub use listener::{AckMode, Listener, ListenerConfig, MetricsRecorder, NoopMetrics, Scaler};
pub use session::{MessageSessionClient, SessionApi};
pub use types::{
    parse_message_json, JobAssigned, JobAvailable, JobCompleted, JobMessageBase, JobStarted, Label,
    MessageType, RunnerGroup, RunnerReference, RunnerScaleSet, RunnerScaleSetJitRunnerConfig,
    RunnerScaleSetJitRunnerSetting, RunnerScaleSetMessage, RunnerScaleSetSession,
    RunnerScaleSetStatistic, RunnerSetting, SystemInfo, DEFAULT_RUNNER_GROUP,
    HEADER_SCALE_SET_MAX_CAPACITY,
};
