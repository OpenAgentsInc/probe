use std::path::PathBuf;
use std::time::Duration;

use probe_core::server_control::{
    PsionicServerConfig, PsionicServerMode, ServerConfigOverrides, ServerTargetKind,
};
use probe_protocol::backend::BackendKind;
use probe_test_support::{FakeAppleFmServer, FakeHttpResponse};
use serde_json::json;

#[test]
fn server_suite_summarizes_launch_and_remote_attach_modes() {
    let loopback = PsionicServerConfig::default().operator_summary();
    assert_eq!(loopback.target_kind, ServerTargetKind::LoopbackAttach);

    let remote = PsionicServerConfig {
        host: String::from("100.88.7.9"),
        ..PsionicServerConfig::default()
    }
    .operator_summary();
    assert_eq!(remote.target_kind, ServerTargetKind::TailnetAttach);
    assert!(remote.is_remote_target());

    let launched = PsionicServerConfig {
        mode: PsionicServerMode::Launch,
        ..PsionicServerConfig::default()
    }
    .operator_summary();
    assert_eq!(launched.target_kind, ServerTargetKind::ManagedLaunch);
}

#[test]
fn server_suite_applies_launch_overrides_consistently() {
    let mut config = PsionicServerConfig::default();
    config
        .apply_overrides(&ServerConfigOverrides {
            mode: Some(PsionicServerMode::Launch),
            host: Some(String::from("0.0.0.0")),
            port: Some(9090),
            backend: Some(String::from("cuda")),
            binary_path: Some(PathBuf::from("/tmp/psionic-openai-server")),
            model_path: Some(PathBuf::from("/tmp/model.gguf")),
            model_id: Some(String::from("custom.gguf")),
            reasoning_budget: Some(2),
        })
        .expect("apply overrides should succeed");

    assert_eq!(config.mode, PsionicServerMode::Launch);
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 9090);
    assert_eq!(config.backend, "cuda");
    assert_eq!(config.resolved_model_id().as_deref(), Some("custom.gguf"));
}

#[test]
fn server_suite_waits_for_ready_apple_fm_bridge() {
    let server = FakeAppleFmServer::from_responses(vec![FakeHttpResponse::json_status(
        200,
        json!({
            "status": "ok",
            "model_available": true,
            "version": "1.0",
            "platform": "macOS"
        }),
    )]);
    let address = server
        .base_url()
        .strip_prefix("http://")
        .expect("base url should start with http://");
    let (host, port) = address.rsplit_once(':').expect("host:port pair");
    let port = port.parse::<u16>().expect("port should parse");
    let config = PsionicServerConfig {
        api_kind: BackendKind::AppleFmBridge,
        host: host.to_string(),
        port,
        model_id: Some(String::from("apple-foundation-model")),
        ..PsionicServerConfig::default()
    };

    let guard = config
        .prepare(Duration::from_secs(2))
        .expect("attach should succeed");

    assert_eq!(guard.base_url(), server.base_url());
    let requests = server.finish();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("GET /health HTTP/1.1"));
}
