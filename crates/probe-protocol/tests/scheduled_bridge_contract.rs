use probe_protocol::scheduled_bridge::{
    PROBE_SCHEDULED_AGENT_BRIDGE_SCHEMA_VERSION, ScheduledAgentBridgeApprovalStatus,
    ScheduledAgentBridgeContractFixture, ScheduledAgentBridgeRunStatus,
};
use probe_protocol::website_events::{PROBE_WEBSITE_EVENT_SCHEMA_VERSION, ProbeWebsiteEventType};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

const FIXTURES: &[&str] = &[
    "success.json",
    "approval_pause.json",
    "child_session.json",
    "cancellation.json",
    "runtime_failure.json",
    "auth_failure.json",
];

#[test]
fn scheduled_bridge_fixtures_cover_required_scenarios() {
    let fixtures = load_fixtures();
    let scenarios = fixtures
        .iter()
        .map(|fixture| fixture.scenario.as_str())
        .collect::<HashSet<_>>();

    for required in [
        "success",
        "approval_pause",
        "child_session",
        "cancellation",
        "runtime_failure",
        "auth_failure",
    ] {
        assert!(
            scenarios.contains(required),
            "missing fixture scenario {required}"
        );
    }
}

#[test]
fn scheduled_bridge_fixtures_deserialize_and_keep_stable_versions() {
    for fixture in load_fixtures() {
        if let Some(signed_request) = fixture.signed_request.as_ref() {
            assert_eq!(
                signed_request.request.schema_version,
                PROBE_SCHEDULED_AGENT_BRIDGE_SCHEMA_VERSION
            );
            assert!(!signed_request.request.idempotency_key.is_empty());
            assert!(!signed_request.auth.nonce.is_empty());
        }

        if let Some(response) = fixture.accepted_response.as_ref() {
            assert_eq!(
                response.schema_version,
                PROBE_SCHEDULED_AGENT_BRIDGE_SCHEMA_VERSION
            );
            assert_eq!(response.request_id, fixture.request_id());
            assert!(!response.probe_session_id.is_empty());
            assert!(!response.transcript_ref.is_empty());
        }

        if let Some(error_response) = fixture.error_response.as_ref() {
            assert_eq!(
                error_response.schema_version,
                PROBE_SCHEDULED_AGENT_BRIDGE_SCHEMA_VERSION
            );
            assert!(!error_response.error.code.is_empty());
            assert!(!error_response.error.message.is_empty());
        }

        if let Some(event_batch) = fixture.event_batch.as_ref() {
            assert_eq!(
                event_batch.schema_version,
                PROBE_WEBSITE_EVENT_SCHEMA_VERSION
            );
            assert_sequences_increase(&fixture.name, event_batch);
            assert_events_match_run(&fixture, event_batch);
        }
    }
}

#[test]
fn scheduled_bridge_fixtures_encode_the_important_runtime_states() {
    let fixtures = load_fixtures();

    let success = fixture(&fixtures, "success");
    assert_eq!(
        success.accepted_response.as_ref().expect("response").status,
        ScheduledAgentBridgeRunStatus::Completed
    );
    assert!(success.has_event_type(ProbeWebsiteEventType::RunCompleted));
    assert!(success.has_event_type(ProbeWebsiteEventType::ArtifactRef));

    let approval_pause = fixture(&fixtures, "approval_pause");
    assert_eq!(
        approval_pause
            .accepted_response
            .as_ref()
            .expect("response")
            .status,
        ScheduledAgentBridgeRunStatus::ApprovalRequired
    );
    assert_eq!(
        approval_pause.approval.as_ref().expect("approval").status,
        ScheduledAgentBridgeApprovalStatus::Pending
    );
    assert!(approval_pause.has_event_type(ProbeWebsiteEventType::ApprovalRequested));

    let child_session = fixture(&fixtures, "child_session");
    assert!(child_session.has_event_type(ProbeWebsiteEventType::ChildSessionStarted));
    assert!(child_session.has_event_type(ProbeWebsiteEventType::ChildSessionUpdated));

    let cancellation = fixture(&fixtures, "cancellation");
    assert_eq!(
        cancellation
            .accepted_response
            .as_ref()
            .expect("response")
            .status,
        ScheduledAgentBridgeRunStatus::Cancelled
    );
    assert!(cancellation.has_event_type(ProbeWebsiteEventType::RunCancelled));

    let runtime_failure = fixture(&fixtures, "runtime_failure");
    assert_eq!(
        runtime_failure
            .accepted_response
            .as_ref()
            .expect("response")
            .status,
        ScheduledAgentBridgeRunStatus::Failed
    );
    assert_eq!(
        runtime_failure
            .error_response
            .as_ref()
            .expect("error")
            .error
            .code,
        "runtime.backend_failed"
    );
    assert!(runtime_failure.has_event_type(ProbeWebsiteEventType::RunFailed));

    let auth_failure = fixture(&fixtures, "auth_failure");
    assert!(auth_failure.accepted_response.is_none());
    assert!(auth_failure.event_batch.is_none());
    assert_eq!(
        auth_failure
            .error_response
            .as_ref()
            .expect("error")
            .error
            .code,
        "auth.invalid_signature"
    );
}

#[test]
fn scheduled_bridge_codex_is_a_probe_backend_not_the_scheduler_boundary() {
    let fixtures = load_fixtures();
    let codex_fixture_count = fixtures
        .iter()
        .filter(|fixture| {
            fixture
                .signed_request
                .as_ref()
                .is_some_and(|request| request.request.backend.family == "codex")
        })
        .count();

    assert!(codex_fixture_count >= 2);

    for fixture in fixtures {
        if let Some(signed_request) = fixture.signed_request.as_ref() {
            assert_eq!(signed_request.request.backend.mode, "probe_backend");
        }
    }
}

#[test]
fn scheduled_bridge_fixtures_do_not_include_secret_material() {
    for name in FIXTURES {
        let content = fs::read_to_string(fixture_path(name)).expect("read fixture");
        let lower = content.to_ascii_lowercase();

        for forbidden in [
            "bearer ",
            "refresh_token",
            "access_token",
            "api_key",
            "sk-",
            "raw_secret",
        ] {
            assert!(
                !lower.contains(forbidden),
                "{name} contains forbidden secret-shaped text {forbidden}"
            );
        }
    }
}

fn load_fixtures() -> Vec<ScheduledAgentBridgeContractFixture> {
    FIXTURES
        .iter()
        .map(|name| {
            let content = fs::read_to_string(fixture_path(name)).expect("read fixture");
            serde_json::from_str::<ScheduledAgentBridgeContractFixture>(&content)
                .unwrap_or_else(|error| panic!("deserialize {name}: {error}"))
        })
        .collect()
}

fn fixture<'a>(
    fixtures: &'a [ScheduledAgentBridgeContractFixture],
    scenario: &str,
) -> &'a ScheduledAgentBridgeContractFixture {
    fixtures
        .iter()
        .find(|fixture| fixture.scenario == scenario)
        .unwrap_or_else(|| panic!("fixture scenario {scenario}"))
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("scheduled_agent_bridge")
        .join(name)
}

fn assert_sequences_increase(
    fixture_name: &str,
    event_batch: &probe_protocol::website_events::ProbeWebsiteEventBatch,
) {
    let mut previous = 0;

    for event in &event_batch.events {
        assert!(
            event.sequence > previous,
            "{fixture_name} has non-increasing sequence {} after {previous}",
            event.sequence
        );
        previous = event.sequence;
    }
}

fn assert_events_match_run(
    fixture: &ScheduledAgentBridgeContractFixture,
    event_batch: &probe_protocol::website_events::ProbeWebsiteEventBatch,
) {
    let Some(response) = fixture.accepted_response.as_ref() else {
        return;
    };

    for event in &event_batch.events {
        assert_eq!(
            event.correlation.run_id.as_deref(),
            Some(response.run_id.as_str())
        );
        assert_eq!(
            event.correlation.scheduled_run_id.as_deref(),
            Some(response.scheduled_run_id.as_str())
        );
        assert_eq!(
            event.correlation.probe_session_id.as_deref(),
            Some(response.probe_session_id.as_str())
        );
    }
}

trait ScheduledAgentBridgeFixtureExt {
    fn request_id(&self) -> String;
    fn has_event_type(&self, event_type: ProbeWebsiteEventType) -> bool;
}

impl ScheduledAgentBridgeFixtureExt for ScheduledAgentBridgeContractFixture {
    fn request_id(&self) -> String {
        self.signed_request
            .as_ref()
            .map(|request| request.request.request_id.clone())
            .or_else(|| {
                self.error_response
                    .as_ref()
                    .and_then(|response| response.request_id.clone())
            })
            .unwrap_or_default()
    }

    fn has_event_type(&self, event_type: ProbeWebsiteEventType) -> bool {
        self.event_batch.as_ref().is_some_and(|batch| {
            batch
                .events
                .iter()
                .any(|event| event.event_type == event_type)
        })
    }
}
