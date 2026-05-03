# Managed Environment Contract

Probe exposes `probe.managed_environment.v1` as the provider-neutral execution
environment contract for managed agents.

Canonical Rust types:

- `probe_protocol::managed_environment`
- `probe_core::managed_environment`
- `RuntimeCapabilities.managed_environment`
- `ManagedRuntimeHeartbeatRequest.environment`

## Ownership

Laravel owns durable environment records:

- environment ids, names, admin visibility, and API authorization
- which providers and host classes are allowed
- minimum CPU, memory, disk, and GPU requirements
- required language/tool/backend-profile capabilities
- workspace, package-cache, persistence, and checkpoint policy
- redaction and any provider-specific secret binding

Probe owns runtime advertisements:

- worker id
- provider kind and host class
- public environment class
- image or snapshot refs safe to show to the control plane
- public resource limits and tool/language/backend-profile capabilities
- public network/workspace/cache/persistence/checkpoint policies
- incompatibility reasons when a worker cannot satisfy constraints

Provider credentials, bearer tokens, API keys, secret-manager resource payloads,
refresh tokens, and local secret material must not be placed in environment
advertisements. The protocol metadata wrapper drops secret-like keys during
serialization/deserialization, but callers should still treat advertisements as
public control-plane facts.

## Providers

The same contract covers:

- `pylon` with `pylon_device`
- `google_cloud` with `cloud_run_job`, `cloud_run_worker_pool`, or `gce_vm`
- `private_gce` with `gce_vm`
- `daytona` with `daytona_workspace`
- `local` with `local_dev`

Google Cloud should be the first hosted path while existing GCP services and
credits are available. Daytona should be added as a supplemental provider after
the environment contract and GCP/Pylon paths are stable. A separate Rust Forge
service is not required for this contract; Laravel can own product-level Forge
records and translate them into Probe constraints.

## Laravel Mapping

Laravel environment records should map to `ManagedEnvironmentConstraints`:

- `environment_id` from the Laravel environment primary key or UUID
- `environment_class` for named classes such as `gcp-coding-standard`
- `allowed_providers` for provider allow lists
- `allowed_host_classes` for host class allow lists
- `allowed_network_egress` for explicit network policy
- `min_resources` for CPU, memory, disk, and GPU minimums
- `required_languages` for runtime language/version needs
- `required_tools` for tool availability and maximum risk class needs
- `required_backend_profiles` for Probe backend/profile availability
- `working_directory`, `package_cache`, `persistence`, and `checkpoint` for
  workspace lifecycle requirements
- `required_labels` for coarse routing labels

Laravel can reject a managed session before dispatch by evaluating current
worker advertisements against those constraints. Probe's Rust matcher returns a
`ManagedEnvironmentCompatibilityReport` with explicit reason codes such as
`provider_not_allowed`, `host_class_not_allowed`, `missing_language`,
`missing_tool`, `missing_backend_profile`, and `insufficient_memory_mib`.

## Worker Advertisement

Probe workers advertise `ManagedEnvironmentWorkerAdvertisement` through runtime
initialization and managed-runtime heartbeats:

```json
{
  "schemaVersion": "probe.managed_environment.v1",
  "advertisedAtMs": 1777777777000,
  "workerId": "worker-gcp-1",
  "capabilities": {
    "schemaVersion": "probe.managed_environment.v1",
    "provider": "google_cloud",
    "hostClass": "cloud_run_worker_pool",
    "environmentClass": "gcp-coding-standard",
    "resourceLimits": {
      "cpuMillicores": 4000,
      "memoryMib": 8192,
      "diskMib": 51200
    },
    "languages": [{"language": "rust", "versions": ["1.86"]}],
    "tools": [{"name": "git", "kind": "vcs"}],
    "backendProfiles": ["openai-codex-subscription"],
    "networkEgress": "restricted",
    "workingDirectory": "persistent_workspace",
    "packageCache": "persistent_per_worker",
    "persistence": "checkpointed",
    "checkpoint": "periodic"
  }
}
```

`RuntimeCapabilities.supports_managed_environment_contract=true` tells Laravel
that the server can speak this contract. `managed_environment` on the same
capabilities object is the local worker advertisement for the current transport.

## Verification

Focused coverage:

```bash
cargo test -p probe-protocol managed_environment -- --nocapture
cargo test -p probe-core managed_environment -- --nocapture
cargo test -p probe-server stdio_protocol_can_initialize_start_resume_and_run_a_turn -- --nocapture
```
