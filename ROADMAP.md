# actions-scaleset Roadmap

This roadmap tracks work that is intentionally deferred while the crate is used to build the kube-rs controller. The baseline reviewed for this document is commit `b8d32d8`.

The Runner Scale Set API is still in public preview, so compatibility with `github.com/actions/scaleset` should be re-audited periodically.

## Controller-driven follow-up

Prefer to discover these requirements by using the crate in the kube-rs controller rather than expanding the library speculatively.

- [ ] Validate the public API ergonomics from real controller code.
- [ ] Add end-to-end tests for the complete demand → runner → job → cleanup lifecycle.
- [ ] Test controller restart and message redelivery behavior with durable Kubernetes state.
- [ ] Test duplicate job/lifecycle messages and prove scaler operations are idempotent.
- [ ] Verify graceful shutdown closes the message session.
- [ ] Verify crash/restart behavior when a message session is abandoned.
- [ ] Validate behavior under concurrent reconciliation and session-token refresh.
- [ ] Add tests for multiple runner scale sets using independent sessions.
- [ ] Revisit whether `Listener` belongs in this crate or whether the controller should own more of the reconciliation loop.

## Protocol and robustness

- [ ] Add fixture-based compatibility tests using representative upstream Go API payloads.
- [ ] Add explicit `Retry-After` tests for both integer-second and HTTP-date formats.
- [ ] Review low-level network retry classification.
  - The Rust transport currently retries `reqwest::Error` broadly.
  - The upstream retry client excludes some permanent TLS, URL, and redirect failures.
- [ ] Make retry response draining strictly honor the configured byte limit.
- [ ] Handle a UTF-8 BOM in JSON response bodies if GitHub/upstream compatibility requires it.
- [ ] Replace hand-built query-string assembly with `url::Url` query APIs.
- [ ] Review hosted-development URL handling (`github.localhost`, `*.ghe.com`) for custom schemes and ports.
- [ ] Replace the lightweight JWT expiration decoder with a more robust parser if token formats evolve.
- [ ] Re-audit unknown message handling as GitHub adds message types.

## Transport parity with the Go client

These are intentionally deferred unless a deployment requires them.

- [ ] Custom root certificate authorities.
- [ ] mTLS client certificates.
- [ ] Custom proxy function/configuration.
- [ ] Custom retry HTTP client.
- [ ] Custom logger integration.
- [ ] `DebugInfo`-equivalent diagnostics.
- [ ] Additional TLS controls beyond `danger_accept_invalid_certs`.

## API and package quality

Before publishing a stable crate or committing to a 1.0 API:

- [ ] Review which modules should remain public versus re-exported through the crate root.
- [ ] Audit all public types for upstream wire-format compatibility.
- [ ] Review `SystemInfo` serialization names against upstream JSON fields.
- [ ] Add API-level documentation for authentication, session lifetime, retries, and acknowledgement semantics.
- [ ] Add a changelog and release/versioning policy.
- [ ] Decide whether to publish to crates.io.
- [ ] Add docs.rs/crates.io metadata and badges if published.
- [ ] Add MSRV CI coverage for `rust-version`.
- [ ] Consider `cargo-deny` and/or `cargo audit` in CI.
- [ ] Review dependency features and binary-size impact.

## Upstream tracking

Because the GitHub Runner Scale Set API is in public preview:

- [ ] Periodically inventory public methods/types in `github.com/actions/scaleset`.
- [ ] Track changes to request/response payloads and API version constants.
- [ ] Track changes to message acknowledgement and redelivery semantics.
- [ ] Track GitHub runner issues involving premature or inconsistent `JobCompleted` events.
- [ ] Re-run parity tests whenever the upstream Go library changes materially.

## Intentional design decisions

These are not bugs to “fix” during future parity work unless there is a deliberate design change.

- **Acknowledge after processing.** Messages are deleted only after processing succeeds.
- **Assume at-least-once delivery.** Scaler/controller operations must be idempotent.
- **Scale from `statistics.total_assigned_jobs`.** Individual job-message counts are not authoritative scaling state.
- **Fail closed when statistics are unavailable.** Missing statistics must not be interpreted as zero demand.
- **Treat `JobCompleted` as a signal, not proof that a runner is safe to destroy.**
- **Keep GitHub API transport concerns separate from Kubernetes reconciliation state.**

## Non-goal for the current phase

Do not delay the kube-rs controller to achieve perfect feature parity with the upstream Go transport. Add deferred features when the controller or a real deployment demonstrates a requirement for them.
