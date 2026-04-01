use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
use probe_protocol::runtime::{
    ClientMessage, RequestEnvelope, ResponseBody, RuntimeProgressEvent, RuntimeRequest,
    RuntimeResponse, ServerEvent, ServerMessage, SessionLookupRequest, StartSessionRequest,
    TransportKind, TurnRequest,
};
use probe_protocol::session::SessionId;
use probe_test_support::{FakeHttpResponse, FakeOpenAiServer, ProbeTestEnvironment};

const TEST_MODEL: &str = "tiny-qwen35";

#[test]
fn stdio_protocol_can_initialize_start_resume_and_run_a_turn() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let fake_backend = FakeOpenAiServer::from_responses(vec![FakeHttpResponse::text_event_stream(
        200,
        concat!(
            "data: {\"id\":\"chatcmpl_server_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hello\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_server_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" from probe-server\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_server_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":3,\"total_tokens\":6}}\n\n"
        ),
    )]);
    let profile = test_profile(fake_backend.base_url());
    let mut harness = ProbeServerHarness::spawn(environment.probe_home());

    let initialize = harness.request(
        "req-init",
        RuntimeRequest::Initialize(probe_protocol::runtime::InitializeRequest {
            client_name: String::from("probe-server-test"),
            client_version: Some(String::from("0.1.0")),
            protocol_version: 1,
        }),
    );
    let RuntimeResponse::Initialize(response) = expect_ok_response(initialize) else {
        panic!("expected initialize response");
    };
    assert_eq!(response.protocol_version, 1);
    assert_eq!(response.capabilities.transport, TransportKind::StdioJsonl);
    assert!(!response.capabilities.supports_queued_turns);

    let start_session = harness.request(
        "req-start-session",
        RuntimeRequest::StartSession(StartSessionRequest {
            title: Some(String::from("server e2e")),
            cwd: environment.workspace().to_path_buf(),
            profile: profile.clone(),
            system_prompt: Some(String::from("You are concise.")),
            harness_profile: None,
        }),
    );
    let RuntimeResponse::StartSession(snapshot) = expect_ok_response(start_session) else {
        panic!("expected start session response");
    };
    let session_id = snapshot.session.id.clone();
    assert_eq!(snapshot.session.title, "server e2e");
    assert!(snapshot.transcript.is_empty());

    let list_sessions = harness.request("req-list", RuntimeRequest::ListSessions);
    let RuntimeResponse::ListSessions(list) = expect_ok_response(list_sessions) else {
        panic!("expected list sessions response");
    };
    assert_eq!(list.sessions.len(), 1);
    assert_eq!(list.sessions[0].id, session_id);

    let resume = harness.request(
        "req-resume",
        RuntimeRequest::ResumeSession(SessionLookupRequest {
            session_id: session_id.clone(),
        }),
    );
    let RuntimeResponse::ResumeSession(resume_snapshot) = expect_ok_response(resume) else {
        panic!("expected resume session response");
    };
    assert!(resume_snapshot.transcript.is_empty());

    harness.send_request(
        "req-turn",
        RuntimeRequest::ContinueTurn(TurnRequest {
            session_id: session_id.clone(),
            profile,
            prompt: String::from("hello"),
            tool_loop: None,
        }),
    );
    let (events, turn_response) = harness.read_until_response("req-turn");
    let RuntimeResponse::ContinueTurn(turn) = expect_ok_response(turn_response) else {
        panic!("expected continue turn response");
    };
    let probe_protocol::runtime::TurnResponse::Completed(completed) = turn else {
        panic!("expected completed turn response");
    };
    assert_eq!(completed.assistant_text, "hello from probe-server");
    assert_eq!(completed.turn.index, 0);
    assert_eq!(completed.response_model, TEST_MODEL);
    assert!(events.iter().any(|event| matches!(
        event,
        ServerEvent::RuntimeProgress {
            event: RuntimeProgressEvent::TurnStarted { .. },
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        ServerEvent::RuntimeProgress {
            event: RuntimeProgressEvent::AssistantDelta { delta, .. },
            ..
        } if delta.contains("hello")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        ServerEvent::RuntimeProgress {
            event: RuntimeProgressEvent::AssistantTurnCommitted { assistant_text, .. },
            ..
        } if assistant_text == "hello from probe-server"
    )));

    let inspect = harness.request(
        "req-inspect",
        RuntimeRequest::InspectSession(SessionLookupRequest {
            session_id: session_id.clone(),
        }),
    );
    let RuntimeResponse::InspectSession(inspect_snapshot) = expect_ok_response(inspect) else {
        panic!("expected inspect session response");
    };
    assert_eq!(inspect_snapshot.transcript.len(), 1);
    assert_eq!(
        inspect_snapshot.transcript[0].turn.items[1].text,
        "hello from probe-server"
    );

    harness.shutdown();
    let requests = fake_backend.finish();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("hello"));
}

#[test]
fn interrupt_turn_is_explicit_when_session_is_idle() {
    let environment = ProbeTestEnvironment::new();
    let mut harness = ProbeServerHarness::spawn(environment.probe_home());
    let response = harness.request(
        "req-interrupt",
        RuntimeRequest::InterruptTurn(probe_protocol::runtime::InterruptTurnRequest {
            session_id: SessionId::new("sess-idle"),
        }),
    );
    let RuntimeResponse::InterruptTurn(interrupt) = expect_ok_response(response) else {
        panic!("expected interrupt response");
    };
    assert!(!interrupt.interrupted);
    assert_eq!(interrupt.reason_code.as_deref(), Some("not_running"));
    harness.shutdown();
}

struct ProbeServerHarness {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ProbeServerHarness {
    fn spawn(probe_home: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_probe-server"))
            .arg("--probe-home")
            .arg(probe_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn probe-server");
        let stdin = child.stdin.take().expect("probe-server stdin");
        let stdout = BufReader::new(child.stdout.take().expect("probe-server stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn request(&mut self, request_id: &str, request: RuntimeRequest) -> ResponseEnvelopeOwned {
        self.send_request(request_id, request);
        let (events, response) = self.read_until_response(request_id);
        assert!(events.is_empty(), "expected no streamed events for request");
        response
    }

    fn send_request(&mut self, request_id: &str, request: RuntimeRequest) {
        let message = ClientMessage::Request(RequestEnvelope {
            request_id: String::from(request_id),
            request,
        });
        let encoded = serde_json::to_string(&message).expect("request should encode");
        writeln!(self.stdin, "{encoded}").expect("write probe-server request");
        self.stdin.flush().expect("flush probe-server request");
    }

    fn read_until_response(
        &mut self,
        request_id: &str,
    ) -> (Vec<ServerEvent>, ResponseEnvelopeOwned) {
        let mut events = Vec::new();
        loop {
            let message = self.read_message();
            match message {
                ServerMessage::Event(event) => {
                    assert_eq!(event.request_id, request_id);
                    events.push(event.event);
                }
                ServerMessage::Response(response) => {
                    assert_eq!(response.request_id, request_id);
                    return (
                        events,
                        ResponseEnvelopeOwned {
                            body: response.body,
                        },
                    );
                }
            }
        }
    }

    fn read_message(&mut self) -> ServerMessage {
        let mut line = String::new();
        let bytes = self
            .stdout
            .read_line(&mut line)
            .expect("read probe-server line");
        assert!(bytes > 0, "probe-server exited before sending a response");
        serde_json::from_str(line.trim_end()).expect("decode probe-server message")
    }

    fn shutdown(&mut self) {
        let response = self.request("req-shutdown", RuntimeRequest::Shutdown);
        let RuntimeResponse::Shutdown(shutdown) = expect_ok_response(response) else {
            panic!("expected shutdown response");
        };
        assert!(shutdown.accepted);
        let status = self.child.wait().expect("wait for probe-server exit");
        assert!(status.success());
    }
}

struct ResponseEnvelopeOwned {
    body: ResponseBody,
}

fn expect_ok_response(response: ResponseEnvelopeOwned) -> RuntimeResponse {
    match response.body {
        ResponseBody::Ok { response } => response,
        ResponseBody::Error { error } => panic!("unexpected protocol error: {error:?}"),
    }
}

fn test_profile(base_url: &str) -> BackendProfile {
    BackendProfile {
        name: String::from("server-test"),
        kind: BackendKind::OpenAiChatCompletions,
        base_url: String::from(base_url),
        model: String::from(TEST_MODEL),
        api_key_env: String::from("PROBE_OPENAI_API_KEY"),
        timeout_secs: 30,
        attach_mode: ServerAttachMode::AttachToExisting,
        prefix_cache_mode: PrefixCacheMode::BackendDefault,
    }
}
