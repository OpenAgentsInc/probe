use std::fmt::{Display, Formatter};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use probe_protocol::admin_chat::{
    AdminChatBridgeAcceptedResponse, AdminChatBridgeAuth, AdminChatBridgeCorrelationMetadata,
    AdminChatBridgeEvent, AdminChatBridgeRequest, AdminChatBridgeSignedRequest,
    AdminChatProviderMetadata, AdminChatRedactedDiagnostics, AdminChatUsageSnapshot,
};
use probe_protocol::backend::{BackendKind, BackendProfile};
use probe_protocol::session::{
    SessionAttachTransport, SessionBackendTarget, SessionRuntimeOwner, SessionRuntimeOwnerKind,
    TranscriptItemKind,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;

use crate::backend_profiles::{OPENAI_CODEX_SUBSCRIPTION_PROFILE, named_backend_profile};
use crate::session_store::{FilesystemSessionStore, NewItem, NewSession, SessionStoreError};

const FAKE_BACKEND_PROFILE: &str = "openagents-admin-chat-fake";
const FAKE_MODEL: &str = "probe-admin-chat-fake-v1";
const SIGNED_BRIDGE_PURPOSE: &str = "probe-admin-chat-bridge-v1";
const DEFAULT_CLOCK_SKEW_MS: u64 = 300_000;
const MIN_SHARED_SECRET_BYTES: usize = 32;
const NONCE_DIR: &str = "admin-chat-bridge";
const NONCE_FILE: &str = "nonces.json";

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug, PartialEq)]
pub struct AdminChatBridgeStream {
    pub events: Vec<AdminChatBridgeEvent>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SignedAdminChatBridgeOutcome {
    pub accepted: AdminChatBridgeAcceptedResponse,
    pub stream: AdminChatBridgeStream,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedAdminChatBridgeOptions {
    pub probe_home: PathBuf,
    pub cwd: PathBuf,
    pub shared_secret: String,
    pub now_ms: u64,
    pub max_clock_skew_ms: u64,
}

impl SignedAdminChatBridgeOptions {
    #[must_use]
    pub fn new(
        probe_home: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
        shared_secret: impl Into<String>,
    ) -> Self {
        Self {
            probe_home: probe_home.into(),
            cwd: cwd.into(),
            shared_secret: shared_secret.into(),
            now_ms: now_ms(),
            max_clock_skew_ms: DEFAULT_CLOCK_SKEW_MS,
        }
    }

    #[must_use]
    pub fn with_now_ms(mut self, now_ms: u64) -> Self {
        self.now_ms = now_ms;
        self
    }

    #[must_use]
    pub fn with_max_clock_skew_ms(mut self, max_clock_skew_ms: u64) -> Self {
        self.max_clock_skew_ms = max_clock_skew_ms;
        self
    }
}

#[derive(Debug)]
pub enum AdminChatBridgeError {
    EmptySharedSecret,
    WeakSharedSecret { min_bytes: usize },
    InvalidSignature,
    Expired { now_ms: u64, expires_at_ms: u64 },
    NotYetValid { now_ms: u64, issued_at_ms: u64 },
    InvalidAuthWindow,
    Replay { key_id: String, nonce: String },
    UnsupportedProviderMode(String),
    UnknownBackendProfile(String),
    MissingRequestField(&'static str),
    Json(serde_json::Error),
    Io(std::io::Error),
    SessionStore(SessionStoreError),
}

impl Display for AdminChatBridgeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySharedSecret => write!(f, "admin chat bridge shared secret is empty"),
            Self::WeakSharedSecret { min_bytes } => write!(
                f,
                "admin chat bridge shared secret must be at least {min_bytes} bytes"
            ),
            Self::InvalidSignature => write!(f, "admin chat bridge signature verification failed"),
            Self::Expired {
                now_ms,
                expires_at_ms,
            } => write!(
                f,
                "admin chat bridge request expired at {expires_at_ms}; now={now_ms}"
            ),
            Self::NotYetValid {
                now_ms,
                issued_at_ms,
            } => write!(
                f,
                "admin chat bridge request issued_at_ms {issued_at_ms} is too far in the future; now={now_ms}"
            ),
            Self::InvalidAuthWindow => {
                write!(
                    f,
                    "admin chat bridge auth expires_at_ms must be after issued_at_ms"
                )
            }
            Self::Replay { key_id, nonce } => {
                write!(
                    f,
                    "admin chat bridge nonce replay rejected for {key_id}:{nonce}"
                )
            }
            Self::UnsupportedProviderMode(mode) => {
                write!(f, "unsupported admin chat bridge provider mode `{mode}`")
            }
            Self::UnknownBackendProfile(profile) => {
                write!(f, "unknown admin chat bridge backend profile `{profile}`")
            }
            Self::MissingRequestField(field) => {
                write!(
                    f,
                    "admin chat bridge request missing required field `{field}`"
                )
            }
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::SessionStore(error) => write!(f, "session store error: {error}"),
        }
    }
}

impl std::error::Error for AdminChatBridgeError {}

impl From<serde_json::Error> for AdminChatBridgeError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<std::io::Error> for AdminChatBridgeError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<SessionStoreError> for AdminChatBridgeError {
    fn from(value: SessionStoreError) -> Self {
        Self::SessionStore(value)
    }
}

#[must_use]
pub fn fake_admin_chat_bridge_stream(request: &AdminChatBridgeRequest) -> AdminChatBridgeStream {
    let provider = provider_metadata(request);
    let probe_session_id = format!("probe-admin-chat.{}", request.run_id);
    let diagnostics = AdminChatRedactedDiagnostics {
        probe_session_id: probe_session_id.clone(),
        probe_turn_id: None,
        transcript_ref: format!(
            "probe://admin-chat/{}/{}",
            request.workspace, request.run_id
        ),
        request_id: Some(request.request_id.clone()),
        response_id: Some(format!("fake-response-{}", request.run_id)),
    };
    let usage = AdminChatUsageSnapshot {
        input_tokens: Some(tokenish_count(request.prompt.as_str())),
        output_tokens: Some(12),
        total_tokens: Some(tokenish_count(request.prompt.as_str()) + 12),
        raw: None,
    };
    let mut events = vec![
        AdminChatBridgeEvent::RunStarted {
            request_id: request.request_id.clone(),
            run_id: request.run_id.clone(),
            probe_session_id,
            provider: provider.clone(),
            tool_policy: request.tool_policy.clone(),
        },
        AdminChatBridgeEvent::ModelStreamStarted {
            run_id: request.run_id.clone(),
            provider: provider.clone(),
        },
    ];

    for delta in fake_text(request).split_inclusive(' ') {
        events.push(AdminChatBridgeEvent::TextDelta {
            run_id: request.run_id.clone(),
            id: format!("assistant-{}", request.run_id),
            delta: delta.to_string(),
        });
    }

    events.push(AdminChatBridgeEvent::UsageLimitsSnapshot {
        run_id: request.run_id.clone(),
        provider: provider.clone(),
        usage: Some(usage.clone()),
        limits: None,
    });
    events.push(AdminChatBridgeEvent::RunCompleted {
        run_id: request.run_id.clone(),
        status: String::from("succeeded"),
        provider,
        response_id: Some(format!("fake-response-{}", request.run_id)),
        usage: Some(usage),
        diagnostics,
    });

    AdminChatBridgeStream { events }
}

pub fn sign_admin_chat_bridge_request(
    request: AdminChatBridgeRequest,
    key_id: impl Into<String>,
    shared_secret: &str,
    issued_at_ms: u64,
    expires_at_ms: u64,
    nonce: impl Into<String>,
) -> Result<AdminChatBridgeSignedRequest, AdminChatBridgeError> {
    let auth = AdminChatBridgeAuth {
        key_id: key_id.into(),
        issued_at_ms,
        expires_at_ms,
        nonce: nonce.into(),
        signature: String::new(),
    };
    validate_shared_secret(shared_secret)?;
    let signature = compute_signature(&auth, &request, shared_secret)?;

    Ok(AdminChatBridgeSignedRequest {
        auth: AdminChatBridgeAuth { signature, ..auth },
        request,
    })
}

pub fn run_signed_admin_chat_bridge(
    signed: &AdminChatBridgeSignedRequest,
    options: &SignedAdminChatBridgeOptions,
) -> Result<SignedAdminChatBridgeOutcome, AdminChatBridgeError> {
    validate_signed_request(signed, options)?;
    claim_nonce(options.probe_home.as_path(), &signed.auth, options.now_ms)?;

    validate_required_request_fields(&signed.request)?;
    let profile = resolve_signed_backend_profile(&signed.request)?;
    let provider = signed_provider_metadata(&signed.request, &profile);
    let session_store = FilesystemSessionStore::new(options.probe_home.clone());
    let title = format!(
        "OpenAgents admin bridge {}",
        bounded_label(signed.request.run_id.as_str(), 48)
    );
    let session = session_store.create_session_with(
        NewSession::new(title, options.cwd.clone())
            .with_backend(SessionBackendTarget::from_profile(&profile))
            .with_runtime_owner(Some(SessionRuntimeOwner {
                kind: SessionRuntimeOwnerKind::LocalDaemon,
                owner_id: String::from("openagents-admin-chat-bridge"),
                attach_transport: SessionAttachTransport::StdioJsonl,
                display_name: Some(String::from("OpenAgents admin chat bridge")),
                attach_target: None,
            })),
    )?;
    let turn = session_store.append_turn(
        &session.id,
        &[
            NewItem::new(TranscriptItemKind::UserMessage, signed.request.prompt.clone()),
            NewItem::new(
                TranscriptItemKind::Note,
                format!(
                    "Signed OpenAgents admin chat bridge request accepted for workspace {} conversation {} run {}.",
                    signed.request.workspace,
                    signed.request.conversation_id,
                    signed.request.run_id
                ),
            ),
        ],
    )?;
    let probe_session_id = session.id.as_str().to_string();
    let probe_turn_id = format!("turn-{}", turn.index);
    let transcript_ref = format!(
        "probe://sessions/{}/transcript#{}",
        probe_session_id, probe_turn_id
    );
    let diagnostics = AdminChatRedactedDiagnostics {
        probe_session_id: probe_session_id.clone(),
        probe_turn_id: Some(probe_turn_id.clone()),
        transcript_ref: transcript_ref.clone(),
        request_id: Some(signed.request.request_id.clone()),
        response_id: None,
    };
    let accepted = AdminChatBridgeAcceptedResponse {
        request_id: signed.request.request_id.clone(),
        run_id: signed.request.run_id.clone(),
        probe_session_id: probe_session_id.clone(),
        probe_turn_id,
        provider: provider.clone(),
        transcript_ref,
        correlation: correlation_metadata(&signed.request),
    };
    let stream = AdminChatBridgeStream {
        events: vec![
            AdminChatBridgeEvent::RunStarted {
                request_id: signed.request.request_id.clone(),
                run_id: signed.request.run_id.clone(),
                probe_session_id,
                provider: provider.clone(),
                tool_policy: signed.request.tool_policy.clone(),
            },
            AdminChatBridgeEvent::RunCompleted {
                run_id: signed.request.run_id.clone(),
                status: String::from("accepted"),
                provider,
                response_id: None,
                usage: None,
                diagnostics,
            },
        ],
    };

    Ok(SignedAdminChatBridgeOutcome { accepted, stream })
}

pub fn render_admin_chat_sse(events: &[AdminChatBridgeEvent]) -> Result<String, serde_json::Error> {
    let mut output = String::new();

    for event in events {
        output.push_str("data: ");
        output.push_str(serde_json::to_string(event)?.as_str());
        output.push_str("\n\n");
    }

    output.push_str("data: [DONE]\n\n");

    Ok(output)
}

fn validate_signed_request(
    signed: &AdminChatBridgeSignedRequest,
    options: &SignedAdminChatBridgeOptions,
) -> Result<(), AdminChatBridgeError> {
    validate_shared_secret(options.shared_secret.as_str())?;
    if signed.auth.expires_at_ms <= signed.auth.issued_at_ms {
        return Err(AdminChatBridgeError::InvalidAuthWindow);
    }
    if options.now_ms > signed.auth.expires_at_ms {
        return Err(AdminChatBridgeError::Expired {
            now_ms: options.now_ms,
            expires_at_ms: signed.auth.expires_at_ms,
        });
    }
    if signed.auth.issued_at_ms > options.now_ms.saturating_add(options.max_clock_skew_ms) {
        return Err(AdminChatBridgeError::NotYetValid {
            now_ms: options.now_ms,
            issued_at_ms: signed.auth.issued_at_ms,
        });
    }

    let provided = normalize_signature(signed.auth.signature.as_str())?;
    verify_signature(
        &signed.auth,
        &signed.request,
        options.shared_secret.as_str(),
        provided.as_slice(),
    )?;

    Ok(())
}

fn validate_required_request_fields(
    request: &AdminChatBridgeRequest,
) -> Result<(), AdminChatBridgeError> {
    for (field, value) in [
        ("requestId", request.request_id.as_str()),
        ("workspace", request.workspace.as_str()),
        ("conversationId", request.conversation_id.as_str()),
        ("runId", request.run_id.as_str()),
        ("prompt", request.prompt.as_str()),
        ("provider.key", request.provider.key.as_str()),
        ("provider.mode", request.provider.mode.as_str()),
        ("toolPolicy.mode", request.tool_policy.mode.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(AdminChatBridgeError::MissingRequestField(field));
        }
    }

    Ok(())
}

fn validate_shared_secret(shared_secret: &str) -> Result<(), AdminChatBridgeError> {
    if shared_secret.is_empty() {
        return Err(AdminChatBridgeError::EmptySharedSecret);
    }
    if shared_secret.as_bytes().len() < MIN_SHARED_SECRET_BYTES {
        return Err(AdminChatBridgeError::WeakSharedSecret {
            min_bytes: MIN_SHARED_SECRET_BYTES,
        });
    }

    Ok(())
}

fn compute_signature(
    auth: &AdminChatBridgeAuth,
    request: &AdminChatBridgeRequest,
    shared_secret: &str,
) -> Result<String, AdminChatBridgeError> {
    let mut mac = HmacSha256::new_from_slice(shared_secret.as_bytes())
        .map_err(|_| AdminChatBridgeError::InvalidSignature)?;
    mac.update(canonical_signed_payload(auth, request)?.as_bytes());
    let digest = mac.finalize().into_bytes();
    Ok(format!("sha256={}", hex::encode(digest)))
}

fn verify_signature(
    auth: &AdminChatBridgeAuth,
    request: &AdminChatBridgeRequest,
    shared_secret: &str,
    provided: &[u8],
) -> Result<(), AdminChatBridgeError> {
    let mut mac = HmacSha256::new_from_slice(shared_secret.as_bytes())
        .map_err(|_| AdminChatBridgeError::InvalidSignature)?;
    mac.update(canonical_signed_payload(auth, request)?.as_bytes());
    mac.verify_slice(provided)
        .map_err(|_| AdminChatBridgeError::InvalidSignature)
}

fn canonical_signed_payload(
    auth: &AdminChatBridgeAuth,
    request: &AdminChatBridgeRequest,
) -> Result<String, AdminChatBridgeError> {
    let request_json = serde_json::to_string(request)?;
    Ok(format!(
        "{SIGNED_BRIDGE_PURPOSE}\n{}\n{}\n{}\n{}\n{}",
        auth.key_id, auth.issued_at_ms, auth.expires_at_ms, auth.nonce, request_json
    ))
}

fn normalize_signature(signature: &str) -> Result<Vec<u8>, AdminChatBridgeError> {
    let value = signature
        .trim()
        .strip_prefix("sha256=")
        .unwrap_or_else(|| signature.trim());
    hex::decode(value).map_err(|_| AdminChatBridgeError::InvalidSignature)
}

fn resolve_signed_backend_profile(
    request: &AdminChatBridgeRequest,
) -> Result<BackendProfile, AdminChatBridgeError> {
    if request.provider.mode == "fake" {
        return Err(AdminChatBridgeError::UnsupportedProviderMode(
            request.provider.mode.clone(),
        ));
    }

    let profile_name = metadata_string(
        request,
        &["backendProfile", "backend_profile", "probeBackendProfile"],
    )
    .unwrap_or_else(|| String::from(OPENAI_CODEX_SUBSCRIPTION_PROFILE));

    named_backend_profile(profile_name.as_str())
        .ok_or(AdminChatBridgeError::UnknownBackendProfile(profile_name))
}

fn signed_provider_metadata(
    request: &AdminChatBridgeRequest,
    profile: &BackendProfile,
) -> AdminChatProviderMetadata {
    AdminChatProviderMetadata {
        key: request.provider.key.clone(),
        mode: request.provider.mode.clone(),
        account_ref: request.provider.account_ref.clone(),
        label: request.provider.label.clone(),
        backend_family: backend_family_name(profile.kind).to_string(),
        backend_profile: profile.name.clone(),
        model: profile.model.clone(),
    }
}

fn correlation_metadata(request: &AdminChatBridgeRequest) -> AdminChatBridgeCorrelationMetadata {
    AdminChatBridgeCorrelationMetadata {
        request_id: request.request_id.clone(),
        workspace: request.workspace.clone(),
        web_user_id: request.web_user_id,
        conversation_id: request.conversation_id.clone(),
        run_id: request.run_id.clone(),
        schedule_id: metadata_string(request, &["scheduleId", "schedule_id"]),
        wake_id: metadata_string(request, &["wakeId", "wake_id"]),
        scheduled_run_id: metadata_string(request, &["scheduledRunId", "scheduled_run_id"]),
    }
}

fn metadata_string(request: &AdminChatBridgeRequest, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| match request.metadata.get(*key) {
            Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
            _ => None,
        })
}

fn backend_family_name(kind: BackendKind) -> &'static str {
    match kind {
        BackendKind::OpenAiChatCompletions => "openai_chat_completions",
        BackendKind::OpenAiCodexSubscription => "openai_codex_subscription",
        BackendKind::AppleFmBridge => "apple_fm_bridge",
    }
}

fn bounded_label(value: &str, max_chars: usize) -> String {
    let mut label = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        label.push_str("...");
    }
    label
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NonceLedger {
    #[serde(default)]
    records: Vec<NonceRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NonceRecord {
    key_id: String,
    nonce: String,
    expires_at_ms: u64,
}

fn claim_nonce(
    probe_home: &Path,
    auth: &AdminChatBridgeAuth,
    now_ms: u64,
) -> Result<(), AdminChatBridgeError> {
    if auth.key_id.trim().is_empty() {
        return Err(AdminChatBridgeError::MissingRequestField("auth.keyId"));
    }
    if auth.nonce.trim().is_empty() {
        return Err(AdminChatBridgeError::MissingRequestField("auth.nonce"));
    }

    let dir = probe_home.join(NONCE_DIR);
    fs::create_dir_all(dir.as_path())?;
    let path = dir.join(NONCE_FILE);
    let mut ledger = read_nonce_ledger(path.as_path())?;
    ledger
        .records
        .retain(|record| record.expires_at_ms >= now_ms);

    if ledger
        .records
        .iter()
        .any(|record| record.key_id == auth.key_id && record.nonce == auth.nonce)
    {
        return Err(AdminChatBridgeError::Replay {
            key_id: auth.key_id.clone(),
            nonce: auth.nonce.clone(),
        });
    }

    ledger.records.push(NonceRecord {
        key_id: auth.key_id.clone(),
        nonce: auth.nonce.clone(),
        expires_at_ms: auth.expires_at_ms,
    });
    write_nonce_ledger(path.as_path(), &ledger)?;

    Ok(())
}

fn read_nonce_ledger(path: &Path) -> Result<NonceLedger, AdminChatBridgeError> {
    if !path.exists() {
        return Ok(NonceLedger::default());
    }
    let file = File::open(path)?;
    Ok(serde_json::from_reader(file)?)
}

fn write_nonce_ledger(path: &Path, ledger: &NonceLedger) -> Result<(), AdminChatBridgeError> {
    let tmp_path = path.with_extension(format!("json.tmp.{}", std::process::id()));
    {
        let mut file = File::create(tmp_path.as_path())?;
        serde_json::to_writer_pretty(&mut file, ledger)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    fs::rename(tmp_path, path)?;
    Ok(())
}

fn provider_metadata(request: &AdminChatBridgeRequest) -> AdminChatProviderMetadata {
    AdminChatProviderMetadata {
        key: request.provider.key.clone(),
        mode: request.provider.mode.clone(),
        account_ref: request.provider.account_ref.clone(),
        label: request.provider.label.clone(),
        backend_family: String::from("fake"),
        backend_profile: String::from(FAKE_BACKEND_PROFILE),
        model: String::from(FAKE_MODEL),
    }
}

fn fake_text(request: &AdminChatBridgeRequest) -> String {
    let prompt = request
        .prompt
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let summary = prompt.chars().take(96).collect::<String>();

    format!(
        "Probe admin chat bridge fake response for {} run {}: {}",
        request.workspace, request.run_id, summary
    )
}

fn tokenish_count(text: &str) -> u64 {
    text.split_whitespace().count().max(1) as u64
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        AdminChatBridgeError, SignedAdminChatBridgeOptions, fake_admin_chat_bridge_stream,
        render_admin_chat_sse, run_signed_admin_chat_bridge, sign_admin_chat_bridge_request,
    };
    use probe_protocol::admin_chat::{AdminChatBridgeEvent, AdminChatBridgeRequest};
    use serde_json::json;
    use tempfile::tempdir;

    const TEST_SECRET: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn fake_bridge_stream_maps_to_laravel_persistence_events() {
        let mut request = AdminChatBridgeRequest::fake(
            "request-1",
            123,
            "admin@example.com",
            "Summarize provider state.",
        );
        request.run_id = String::from("run-123");
        request.provider.account_ref = Some(String::from("provider-account-opaque-1"));

        let stream = fake_admin_chat_bridge_stream(&request);

        assert!(matches!(
            stream.events.first(),
            Some(AdminChatBridgeEvent::RunStarted { .. })
        ));
        assert!(
            stream
                .events
                .iter()
                .any(|event| matches!(event, AdminChatBridgeEvent::ModelStreamStarted { .. }))
        );
        assert!(
            stream
                .events
                .iter()
                .any(|event| matches!(event, AdminChatBridgeEvent::TextDelta { .. }))
        );
        assert!(matches!(
            stream.events.last(),
            Some(AdminChatBridgeEvent::RunCompleted { .. })
        ));
    }

    #[test]
    fn fake_bridge_sse_does_not_echo_secret_shaped_metadata() {
        let mut request = AdminChatBridgeRequest::fake(
            "request-2",
            123,
            "admin@example.com",
            "Do not leak secrets.",
        );
        request
            .metadata
            .insert(String::from("api_key"), json!("sk-should-not-appear"));
        request
            .metadata
            .insert(String::from("refresh_token"), json!("refresh-secret"));

        let stream = fake_admin_chat_bridge_stream(&request);
        let rendered = render_admin_chat_sse(&stream.events).expect("render sse");

        assert!(rendered.contains("data: {\"type\":\"run_started\""));
        assert!(rendered.contains("data: [DONE]"));
        assert!(!rendered.contains("sk-should-not-appear"));
        assert!(!rendered.contains("refresh-secret"));
    }

    #[test]
    fn signed_bridge_accepts_valid_request_and_creates_probe_session() {
        let temp = tempdir().expect("tempdir");
        let mut request = AdminChatBridgeRequest::fake(
            "request-signed-1",
            123,
            "admin@example.com",
            "Get acquainted with the current schedule state.",
        );
        request.conversation_id = String::from("conversation-admin-1");
        request.run_id = String::from("run-admin-1");
        request.provider.mode = String::from("service_api_key");
        request.metadata.insert(
            String::from("backendProfile"),
            json!("openai-codex-subscription"),
        );
        request
            .metadata
            .insert(String::from("api_key"), json!("sk-should-not-leak"));

        let signed = sign_admin_chat_bridge_request(
            request,
            "openagents.com",
            TEST_SECRET,
            1_000,
            61_000,
            "nonce-1",
        )
        .expect("sign request");
        let options =
            SignedAdminChatBridgeOptions::new(temp.path().join(".probe"), temp.path(), TEST_SECRET)
                .with_now_ms(2_000);

        let outcome = run_signed_admin_chat_bridge(&signed, &options).expect("run bridge");

        assert_eq!(outcome.accepted.request_id, "request-signed-1");
        assert_eq!(outcome.accepted.run_id, "run-admin-1");
        assert_eq!(outcome.accepted.probe_turn_id, "turn-0");
        assert_eq!(
            outcome.accepted.provider.backend_profile,
            "openai-codex-subscription"
        );
        assert!(
            outcome
                .accepted
                .transcript_ref
                .contains(outcome.accepted.probe_session_id.as_str())
        );
        assert!(matches!(
            outcome.stream.events.first(),
            Some(AdminChatBridgeEvent::RunStarted { .. })
        ));
        assert!(matches!(
            outcome.stream.events.last(),
            Some(AdminChatBridgeEvent::RunCompleted { .. })
        ));
        let rendered = render_admin_chat_sse(&outcome.stream.events).expect("render sse");
        assert!(!rendered.contains("sk-should-not-leak"));
    }

    #[test]
    fn signed_bridge_rejects_tampered_signature() {
        let temp = tempdir().expect("tempdir");
        let request = AdminChatBridgeRequest::fake(
            "request-signed-2",
            123,
            "admin@example.com",
            "Summarize the bridge state.",
        );
        let mut signed = sign_admin_chat_bridge_request(
            request,
            "openagents.com",
            TEST_SECRET,
            1_000,
            61_000,
            "nonce-2",
        )
        .expect("sign request");
        signed.request.prompt = String::from("tampered prompt");
        let options =
            SignedAdminChatBridgeOptions::new(temp.path().join(".probe"), temp.path(), TEST_SECRET)
                .with_now_ms(2_000);

        let error = run_signed_admin_chat_bridge(&signed, &options).expect_err("reject signature");

        assert!(matches!(error, AdminChatBridgeError::InvalidSignature));
    }

    #[test]
    fn signed_bridge_rejects_replayed_nonce() {
        let temp = tempdir().expect("tempdir");
        let request = AdminChatBridgeRequest::fake(
            "request-signed-3",
            123,
            "admin@example.com",
            "Summarize the bridge state.",
        );
        let signed = sign_admin_chat_bridge_request(
            request,
            "openagents.com",
            TEST_SECRET,
            1_000,
            61_000,
            "nonce-3",
        )
        .expect("sign request");
        let options =
            SignedAdminChatBridgeOptions::new(temp.path().join(".probe"), temp.path(), TEST_SECRET)
                .with_now_ms(2_000);

        run_signed_admin_chat_bridge(&signed, &options).expect("first request succeeds");
        let error = run_signed_admin_chat_bridge(&signed, &options).expect_err("reject replay");

        assert!(matches!(error, AdminChatBridgeError::Replay { .. }));
    }

    #[test]
    fn signed_bridge_rejects_fake_provider_mode() {
        let temp = tempdir().expect("tempdir");
        let mut request = AdminChatBridgeRequest::fake(
            "request-signed-4",
            123,
            "admin@example.com",
            "Summarize the bridge state.",
        );
        request.provider.mode = String::from("fake");
        let signed = sign_admin_chat_bridge_request(
            request,
            "openagents.com",
            TEST_SECRET,
            1_000,
            61_000,
            "nonce-4",
        )
        .expect("sign request");
        let options =
            SignedAdminChatBridgeOptions::new(temp.path().join(".probe"), temp.path(), TEST_SECRET)
                .with_now_ms(2_000);

        let error = run_signed_admin_chat_bridge(&signed, &options).expect_err("reject fake mode");

        assert!(matches!(
            error,
            AdminChatBridgeError::UnsupportedProviderMode(_)
        ));
    }
}
