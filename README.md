# actions-scaleset

Rust client for the GitHub Actions **Runner Scale Set** APIs. Port of
[`github.com/actions/scaleset`](https://github.com/actions/scaleset) for custom
autoscalers (VMs, containers, vSphere vPods, bare metal).

The API is in public preview. This crate mirrors the Go surface:

- Create / update / delete runner scale sets
- Generate just-in-time runner configs
- Message sessions (`GetMessage`, `DeleteMessage`, `AcquireJobs`)
- GitHub App (JWT → installation token) or PAT authentication
- Automatic Actions-service admin token refresh
- Session token refresh on `MessageQueueTokenExpired`

## The listener fix

The Go `listener` package **deletes (acks) a message before** `AcquireJobs` and
scaler callbacks. If those later steps fail, GitHub will not redeliver the
message.

This crate’s `Listener` acks **after** processing (`AckMode::AfterProcess`).
`AckMode::GoCompat` exists only to reproduce the upstream order.

```text
Go (unsafe)                         Rust (default)
────────────                        ──────────────
GetMessage                          GetMessage
handle statistics                   handle statistics
DeleteMessage   ← ack too early     AcquireJobs
AcquireJobs                         HandleJobStarted / Completed
HandleJob*                          HandleDesiredRunnerCount
HandleDesiredRunnerCount            DeleteMessage   ← ack after success
```

## Usage

```rust
use actions_scaleset::{
    Client, GitHubAppAuth, GitHubAppClientConfig, HttpOptions, Listener, ListenerConfig,
    RunnerScaleSet, SystemInfo,
};

# async fn demo() -> actions_scaleset::Result<()> {
let client = Client::with_github_app(
    GitHubAppClientConfig {
        github_config_url: "https://github.com/org/repo".into(),
        github_app_auth: GitHubAppAuth {
            client_id: std::env::var("GH_APP_CLIENT_ID").unwrap(),
            installation_id: 1,
            private_key_pem: std::fs::read_to_string("app.pem")?,
        },
        system_info: SystemInfo {
            system: "vks-operator".into(),
            version: "0.1.0".into(),
            ..Default::default()
        },
    },
    HttpOptions::default(),
)?;

let set = client
    .create_runner_scale_set(RunnerScaleSet {
        name: "vks-builders".into(),
        ..Default::default()
    })
    .await?;

let session = client
    .message_session_client(set.id, "vks-operator")
    .await?;

let listener = Listener::new(
    session,
    ListenerConfig {
        scale_set_id: set.id,
        max_runners: 32,
        ack_mode: Default::default(),
    },
)?;

// listener.run(&my_scaler).await?;
# Ok(())
# }
```

Scale on `statistics.total_assigned_jobs`, not on individual job-message
counts. Responses are capped (~50 messages) and jobs may be reassigned.

JIT configs are secrets. Prefer a GitHub App over a PAT.

## License

MIT, same as the upstream Go client.
