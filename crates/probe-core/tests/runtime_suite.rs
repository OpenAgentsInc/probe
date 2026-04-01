use std::sync::{Arc, Mutex};

use probe_core::backend_profiles::psionic_qwen35_2b_q8_registry;
use probe_core::runtime::{
    PlainTextExecRequest, PlainTextResumeRequest, ProbeRuntime, RuntimeEvent,
};
use probe_protocol::session::TranscriptItemKind;
use probe_test_support::{FakeOpenAiServer, ProbeTestEnvironment};
use serde_json::json;

#[test]
fn runtime_suite_persists_and_resumes_plain_text_sessions() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let runtime = ProbeRuntime::new(environment.probe_home());
    let server = FakeOpenAiServer::from_json_responses(vec![
        json!({
            "id": "chatcmpl_runtime_suite_1",
            "model": "tiny-qwen35",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "first turn"},
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 4,
                "completion_tokens": 2,
                "total_tokens": 6
            }
        }),
        json!({
            "id": "chatcmpl_runtime_suite_2",
            "model": "tiny-qwen35",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "second turn"},
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 5,
                "completion_tokens": 2,
                "total_tokens": 7
            }
        }),
    ]);
    let mut profile = psionic_qwen35_2b_q8_registry();
    profile.base_url = server.base_url().to_string();
    profile.model = String::from("tiny-qwen35");

    let first = runtime
        .exec_plain_text(PlainTextExecRequest {
            profile: profile.clone(),
            prompt: String::from("Say first turn"),
            title: Some(String::from("Runtime Suite")),
            cwd: environment.workspace().to_path_buf(),
            system_prompt: None,
            harness_profile: None,
            tool_loop: None,
        })
        .expect("first runtime turn should succeed");
    let resumed = runtime
        .continue_plain_text_session(PlainTextResumeRequest {
            session_id: first.session.id.clone(),
            profile,
            prompt: String::from("Say second turn"),
            tool_loop: None,
        })
        .expect("resume turn should succeed");

    assert_eq!(first.assistant_text, "first turn");
    assert_eq!(resumed.assistant_text, "second turn");
    assert_eq!(resumed.turn.index, 1);

    let sessions = runtime
        .session_store()
        .list_sessions()
        .expect("list sessions should succeed");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "Runtime Suite");
    assert_eq!(sessions[0].next_turn_index, 2);

    let transcript = runtime
        .session_store()
        .read_transcript(&first.session.id)
        .expect("transcript should load");
    assert_eq!(transcript.len(), 2);
    assert_eq!(
        transcript[0].turn.items[0].kind,
        TranscriptItemKind::UserMessage
    );
    assert_eq!(
        transcript[1].turn.items[1].kind,
        TranscriptItemKind::AssistantMessage
    );
    assert_eq!(transcript[1].turn.items[1].text, "second turn");

    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("Say first turn"));
    assert!(requests[1].contains("Say second turn"));
}

#[test]
fn runtime_suite_emits_turn_lifecycle_events() {
    let environment = ProbeTestEnvironment::new();
    let runtime = ProbeRuntime::new(environment.probe_home());
    let server = FakeOpenAiServer::from_json_responses(vec![json!({
        "id": "chatcmpl_runtime_events",
        "model": "tiny-qwen35",
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": "eventful"},
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 3,
            "completion_tokens": 1,
            "total_tokens": 4
        }
    })]);
    let mut profile = psionic_qwen35_2b_q8_registry();
    profile.base_url = server.base_url().to_string();
    profile.model = String::from("tiny-qwen35");
    let events = Arc::new(Mutex::new(Vec::<RuntimeEvent>::new()));
    let sink_events = Arc::clone(&events);

    runtime
        .exec_plain_text_with_events(
            PlainTextExecRequest {
                profile,
                prompt: String::from("Emit events"),
                title: None,
                cwd: environment.workspace().to_path_buf(),
                system_prompt: None,
                harness_profile: None,
                tool_loop: None,
            },
            Arc::new(move |event| {
                sink_events
                    .lock()
                    .expect("event collection lock")
                    .push(event);
            }),
        )
        .expect("eventful runtime turn should succeed");

    let captured = events.lock().expect("event collection lock");
    assert!(
        captured
            .iter()
            .any(|event| matches!(event, RuntimeEvent::TurnStarted { .. }))
    );
    assert!(
        captured
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
    );
    assert!(captured.iter().any(|event| matches!(
        event,
        RuntimeEvent::AssistantTurnCommitted { assistant_text, .. } if assistant_text == "eventful"
    )));
}
