use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::session::TimestampMs;

pub const PROBE_CODEX_WORKROOM_SCHEMA_VERSION: &str = "probe.codex_workroom.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexWorkroomMode {
    CodexExec,
    CodexMcpServer,
    CodexSdkThread,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexWorkroomSandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexWorkroomApprovalPolicy {
    Never,
    OnRequest,
    OnFailure,
    UnlessTrusted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexWorkroomArtifactVisibility {
    Private,
    Workroom,
    PublicProjection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexWorkroomArtifactRetention {
    Ephemeral,
    Retained,
    SnapshotOnFinish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexWorkroomEventKind {
    Queued,
    Started,
    Log,
    Redacted,
    Artifact,
    Receipt,
    Completed,
    Failed,
    Timeout,
    Cancelled,
}

impl CodexWorkroomEventKind {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Timeout | Self::Cancelled
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexWorkroomFailureKind {
    NonzeroExit,
    Timeout,
    Cancelled,
    AuthFailure,
    SetupFailure,
    ArtifactCaptureFailure,
    StreamDisconnect,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexWorkroomSessionSpec {
    pub schema_version: String,
    pub workroom_id: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_ref: Option<String>,
    pub cwd: PathBuf,
    pub sandbox_mode: CodexWorkroomSandboxMode,
    pub approval_policy: CodexWorkroomApprovalPolicy,
    pub timeout_ms: u64,
    pub auth_profile_ref: String,
    pub artifact_policy: String,
    pub callback_target: String,
    pub mode: CodexWorkroomMode,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

impl CodexWorkroomSessionSpec {
    #[must_use]
    pub fn new(
        workroom_id: impl Into<String>,
        session_id: impl Into<String>,
        cwd: impl Into<PathBuf>,
        auth_profile_ref: impl Into<String>,
        callback_target: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: String::from(PROBE_CODEX_WORKROOM_SCHEMA_VERSION),
            workroom_id: workroom_id.into(),
            session_id: session_id.into(),
            thread_id: None,
            repo_ref: None,
            cwd: cwd.into(),
            sandbox_mode: CodexWorkroomSandboxMode::WorkspaceWrite,
            approval_policy: CodexWorkroomApprovalPolicy::Never,
            timeout_ms: 900_000,
            auth_profile_ref: auth_profile_ref.into(),
            artifact_policy: String::from("redacted_logs"),
            callback_target: callback_target.into(),
            mode: CodexWorkroomMode::CodexExec,
            metadata: Map::new(),
        }
    }

    #[must_use]
    pub fn with_thread_id(mut self, thread_id: impl Into<String>) -> Self {
        self.thread_id = Some(thread_id.into());
        self
    }

    #[must_use]
    pub fn with_repo_ref(mut self, repo_ref: impl Into<String>) -> Self {
        self.repo_ref = Some(repo_ref.into());
        self
    }

    #[must_use]
    pub fn with_metadata(mut self, metadata: Map<String, Value>) -> Self {
        let Value::Object(redacted) = redact_codex_workroom_value(Value::Object(metadata)) else {
            return self;
        };
        self.metadata = redacted;
        self
    }

    #[must_use]
    pub fn session_ref(&self) -> CodexWorkroomSessionRef {
        CodexWorkroomSessionRef {
            workroom_id: self.workroom_id.clone(),
            session_id: self.session_id.clone(),
            thread_id: self.thread_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexWorkroomSessionRef {
    pub workroom_id: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexWorkroomArtifactRef {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub visibility: CodexWorkroomArtifactVisibility,
    pub retention: CodexWorkroomArtifactRetention,
    pub producer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closeout_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexWorkroomReceiptRef {
    pub receipt_type: String,
    pub resource_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexWorkroomFailure {
    pub kind: CodexWorkroomFailureKind,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub retryable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexWorkroomEvent {
    pub schema_version: String,
    pub sequence: u64,
    pub occurred_at_ms: TimestampMs,
    pub kind: CodexWorkroomEventKind,
    pub session: CodexWorkroomSessionRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub details: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<CodexWorkroomArtifactRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receipt_refs: Vec<CodexWorkroomReceiptRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<CodexWorkroomFailure>,
    #[serde(default)]
    pub redacted: bool,
}

impl CodexWorkroomEvent {
    #[must_use]
    pub fn new(
        sequence: u64,
        occurred_at_ms: TimestampMs,
        kind: CodexWorkroomEventKind,
        session: CodexWorkroomSessionRef,
    ) -> Self {
        Self {
            schema_version: String::from(PROBE_CODEX_WORKROOM_SCHEMA_VERSION),
            sequence,
            occurred_at_ms,
            kind,
            session,
            message: None,
            details: Map::new(),
            artifact_refs: Vec::new(),
            receipt_refs: Vec::new(),
            failure: None,
            redacted: false,
        }
    }

    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        let raw = message.into();
        let redacted = redact_codex_workroom_text(&raw);
        self.redacted |= raw != redacted;
        self.message = Some(redacted);
        self
    }

    #[must_use]
    pub fn with_details(mut self, details: Map<String, Value>) -> Self {
        let raw = Value::Object(details);
        let redacted = redact_codex_workroom_value(raw.clone());
        self.redacted |= raw != redacted;
        if let Value::Object(redacted_details) = redacted {
            self.details = redacted_details;
        }
        self
    }

    #[must_use]
    pub fn with_artifact_refs(mut self, artifact_refs: Vec<CodexWorkroomArtifactRef>) -> Self {
        self.artifact_refs = artifact_refs;
        self
    }

    #[must_use]
    pub fn with_receipt_refs(mut self, receipt_refs: Vec<CodexWorkroomReceiptRef>) -> Self {
        self.receipt_refs = receipt_refs;
        self
    }

    #[must_use]
    pub fn with_failure(mut self, failure: CodexWorkroomFailure) -> Self {
        self.failure = Some(failure);
        self
    }
}

#[must_use]
pub fn fake_codex_workroom_success_lifecycle(
    spec: &CodexWorkroomSessionSpec,
    started_at_ms: TimestampMs,
) -> Vec<CodexWorkroomEvent> {
    let session = spec.session_ref();
    let artifact = CodexWorkroomArtifactRef {
        path: String::from("artifacts/codex-summary.md"),
        digest: Some(String::from("sha256:fake-codex-summary")),
        mime_type: Some(String::from("text/markdown")),
        visibility: CodexWorkroomArtifactVisibility::Workroom,
        retention: CodexWorkroomArtifactRetention::Retained,
        producer: String::from("codex_exec"),
        closeout_ref: Some(String::from("probe://receipts/fake-codex-closeout")),
    };
    let receipt = CodexWorkroomReceiptRef {
        receipt_type: String::from("workroom.closeout"),
        resource_ref: String::from("probe://receipts/fake-codex-closeout"),
        digest: Some(String::from("sha256:fake-codex-closeout")),
    };

    vec![
        CodexWorkroomEvent::new(
            1,
            started_at_ms,
            CodexWorkroomEventKind::Queued,
            session.clone(),
        )
        .with_message("Codex VM workroom queued."),
        CodexWorkroomEvent::new(
            2,
            started_at_ms + 1,
            CodexWorkroomEventKind::Started,
            session.clone(),
        )
        .with_message("Codex exec started."),
        CodexWorkroomEvent::new(
            3,
            started_at_ms + 2,
            CodexWorkroomEventKind::Log,
            session.clone(),
        )
        .with_message("Codex produced a redacted progress update."),
        CodexWorkroomEvent::new(
            4,
            started_at_ms + 3,
            CodexWorkroomEventKind::Artifact,
            session.clone(),
        )
        .with_artifact_refs(vec![artifact.clone()])
        .with_message("Codex artifact captured."),
        CodexWorkroomEvent::new(
            5,
            started_at_ms + 4,
            CodexWorkroomEventKind::Receipt,
            session.clone(),
        )
        .with_receipt_refs(vec![receipt.clone()])
        .with_message("Codex closeout receipt captured."),
        CodexWorkroomEvent::new(
            6,
            started_at_ms + 5,
            CodexWorkroomEventKind::Completed,
            session,
        )
        .with_artifact_refs(vec![artifact])
        .with_receipt_refs(vec![receipt])
        .with_message("Codex VM workroom completed."),
    ]
}

#[must_use]
pub fn fake_codex_workroom_failure_lifecycle(
    spec: &CodexWorkroomSessionSpec,
    started_at_ms: TimestampMs,
    failure: CodexWorkroomFailure,
) -> Vec<CodexWorkroomEvent> {
    let session = spec.session_ref();
    vec![
        CodexWorkroomEvent::new(
            1,
            started_at_ms,
            CodexWorkroomEventKind::Queued,
            session.clone(),
        )
        .with_message("Codex VM workroom queued."),
        CodexWorkroomEvent::new(
            2,
            started_at_ms + 1,
            CodexWorkroomEventKind::Started,
            session.clone(),
        )
        .with_message("Codex exec started."),
        CodexWorkroomEvent::new(3, started_at_ms + 2, failure_event_kind(&failure), session)
            .with_failure(failure)
            .with_message("Codex VM workroom failed."),
    ]
}

#[must_use]
pub const fn failure_event_kind(failure: &CodexWorkroomFailure) -> CodexWorkroomEventKind {
    match failure.kind {
        CodexWorkroomFailureKind::Timeout => CodexWorkroomEventKind::Timeout,
        CodexWorkroomFailureKind::Cancelled => CodexWorkroomEventKind::Cancelled,
        _ => CodexWorkroomEventKind::Failed,
    }
}

#[must_use]
pub fn normalize_codex_exec_jsonl_line(
    line: &str,
    spec: &CodexWorkroomSessionSpec,
    sequence: u64,
    occurred_at_ms: TimestampMs,
) -> Option<CodexWorkroomEvent> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Some(
            CodexWorkroomEvent::new(
                sequence,
                occurred_at_ms,
                CodexWorkroomEventKind::Log,
                spec.session_ref(),
            )
            .with_message(line),
        );
    };

    Some(normalize_codex_exec_value(
        &value,
        spec,
        sequence,
        occurred_at_ms,
    ))
}

#[must_use]
pub fn normalize_codex_exec_value(
    value: &Value,
    spec: &CodexWorkroomSessionSpec,
    sequence: u64,
    occurred_at_ms: TimestampMs,
) -> CodexWorkroomEvent {
    let kind = string_field(value, &["type", "kind", "event"])
        .as_deref()
        .and_then(codex_workroom_kind_from_str)
        .unwrap_or(CodexWorkroomEventKind::Log);
    let mut session = spec.session_ref();
    if let Some(thread_id) = string_field(value, &["thread_id", "threadId"]) {
        session.thread_id = Some(thread_id);
    }

    let mut event = CodexWorkroomEvent::new(sequence, occurred_at_ms, kind, session);
    if let Some(message) = string_field(
        value,
        &["message", "summary", "delta", "output", "stderr", "stdout"],
    ) {
        event = event.with_message(message);
    }
    if let Some(details) = object_details(value) {
        event = event.with_details(details);
    }
    if matches!(kind, CodexWorkroomEventKind::Artifact) {
        if let Some(artifact) = artifact_ref_from_value(value, "codex_exec") {
            event = event.with_artifact_refs(vec![artifact]);
        }
    }
    if matches!(
        kind,
        CodexWorkroomEventKind::Receipt | CodexWorkroomEventKind::Completed
    ) {
        let receipts = receipt_refs_from_value(value);
        if !receipts.is_empty() {
            event = event.with_receipt_refs(receipts);
        }
    }
    if matches!(
        kind,
        CodexWorkroomEventKind::Failed
            | CodexWorkroomEventKind::Timeout
            | CodexWorkroomEventKind::Cancelled
    ) {
        let message = event
            .message
            .clone()
            .or_else(|| string_field(value, &["error", "reason"]))
            .unwrap_or_else(|| String::from("Codex workroom failed."));
        event = event.with_failure(normalize_codex_failure(
            string_field(value, &["failure_kind", "failureKind", "code"])
                .as_deref()
                .and_then(failure_kind_from_str)
                .unwrap_or_else(|| failure_kind_for_event(kind)),
            string_field(value, &["code"]).unwrap_or_else(|| String::from("codex_failure")),
            message,
            i64_field(value, &["exit_code", "exitCode"]).and_then(|code| i32::try_from(code).ok()),
            !matches!(
                kind,
                CodexWorkroomEventKind::Cancelled | CodexWorkroomEventKind::Timeout
            ),
        ));
    }

    event
}

#[must_use]
pub fn normalize_cloud_runner_event(
    value: &Value,
    spec: &CodexWorkroomSessionSpec,
    sequence: u64,
    occurred_at_ms: TimestampMs,
) -> Option<CodexWorkroomEvent> {
    let kind = string_field(value, &["kind", "type", "event"])
        .as_deref()
        .and_then(codex_workroom_kind_from_str)
        .unwrap_or(CodexWorkroomEventKind::Log);
    let mut event = CodexWorkroomEvent::new(sequence, occurred_at_ms, kind, spec.session_ref());

    if let Some(message) = string_field(value, &["message", "summary", "detail"]) {
        event = event.with_message(message);
    }
    if let Some(details) = object_details(value) {
        event = event.with_details(details);
    }
    let artifacts = artifact_refs_from_cloud_value(value);
    if !artifacts.is_empty() {
        event = event.with_artifact_refs(artifacts);
    }
    let receipts = receipt_refs_from_value(value);
    if !receipts.is_empty() {
        event = event.with_receipt_refs(receipts);
    }
    if matches!(
        kind,
        CodexWorkroomEventKind::Failed
            | CodexWorkroomEventKind::Timeout
            | CodexWorkroomEventKind::Cancelled
    ) {
        let message = event
            .message
            .clone()
            .unwrap_or_else(|| String::from("Cloud Codex runner failed."));
        event = event.with_failure(normalize_codex_failure(
            failure_kind_for_event(kind),
            string_field(value, &["code"]).unwrap_or_else(|| String::from("cloud_runner_failure")),
            message,
            i64_field(value, &["exit_code", "exitCode"]).and_then(|code| i32::try_from(code).ok()),
            !matches!(
                kind,
                CodexWorkroomEventKind::Cancelled | CodexWorkroomEventKind::Timeout
            ),
        ));
    }

    Some(event)
}

#[must_use]
pub fn codex_workroom_kind_from_str(raw: &str) -> Option<CodexWorkroomEventKind> {
    let normalized = raw.trim().to_ascii_lowercase().replace(['-', '.'], "_");
    match normalized.as_str() {
        "queued" | "run_queued" => Some(CodexWorkroomEventKind::Queued),
        "started" | "session_configured" | "turn_started" | "run_started" => {
            Some(CodexWorkroomEventKind::Started)
        }
        "log" | "runner_log" | "agent_message" | "assistant_message" | "exec_command"
        | "command_output" | "mcp_response" | "reasoning" | "text_delta" => {
            Some(CodexWorkroomEventKind::Log)
        }
        "redacted" | "redacted_output" => Some(CodexWorkroomEventKind::Redacted),
        "artifact" | "artifact_ref" | "file_change" | "patch_apply" => {
            Some(CodexWorkroomEventKind::Artifact)
        }
        "receipt" | "receipt_ref" | "closeout" => Some(CodexWorkroomEventKind::Receipt),
        "completed" | "turn_completed" | "run_completed" => Some(CodexWorkroomEventKind::Completed),
        "failed" | "error" | "run_failed" => Some(CodexWorkroomEventKind::Failed),
        "timeout" | "timed_out" => Some(CodexWorkroomEventKind::Timeout),
        "cancelled" | "canceled" | "run_cancelled" | "run_canceled" => {
            Some(CodexWorkroomEventKind::Cancelled)
        }
        _ => None,
    }
}

#[must_use]
pub fn normalize_codex_failure(
    kind: CodexWorkroomFailureKind,
    code: impl Into<String>,
    message: impl Into<String>,
    exit_code: Option<i32>,
    retryable: bool,
) -> CodexWorkroomFailure {
    let raw_message = message.into();
    CodexWorkroomFailure {
        kind,
        code: code.into(),
        message: redact_codex_workroom_text(&raw_message),
        exit_code,
        retryable,
    }
}

#[must_use]
pub fn normalize_codex_exit_failure(
    exit_code: Option<i32>,
    timed_out: bool,
    cancelled: bool,
    stderr: &str,
) -> Option<CodexWorkroomFailure> {
    if cancelled {
        return Some(normalize_codex_failure(
            CodexWorkroomFailureKind::Cancelled,
            "codex_cancelled",
            "Codex workroom was cancelled.",
            exit_code,
            false,
        ));
    }
    if timed_out {
        return Some(normalize_codex_failure(
            CodexWorkroomFailureKind::Timeout,
            "codex_timeout",
            "Codex workroom timed out.",
            exit_code,
            true,
        ));
    }
    if let Some(code) = exit_code {
        if code == 0 {
            return None;
        }
        let lower = stderr.to_ascii_lowercase();
        let kind = if lower.contains("auth")
            || lower.contains("login")
            || lower.contains("unauthorized")
            || lower.contains("forbidden")
        {
            CodexWorkroomFailureKind::AuthFailure
        } else {
            CodexWorkroomFailureKind::NonzeroExit
        };
        return Some(normalize_codex_failure(
            kind,
            "codex_nonzero_exit",
            if stderr.trim().is_empty() {
                format!("Codex exited with code {code}.")
            } else {
                stderr.to_string()
            },
            Some(code),
            !matches!(kind, CodexWorkroomFailureKind::AuthFailure),
        ));
    }
    None
}

#[must_use]
pub fn redact_codex_workroom_value(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact_codex_workroom_text(&text)),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(redact_codex_workroom_value).collect())
        }
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    if is_secret_like_key(&key) {
                        (key, Value::String(String::from("[redacted]")))
                    } else {
                        (key, redact_codex_workroom_value(value))
                    }
                })
                .collect(),
        ),
        other => other,
    }
}

#[must_use]
pub fn redact_codex_workroom_text(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    let lower = input.to_ascii_lowercase();
    for forbidden in [
        "authorization:",
        "bearer ",
        "sk-",
        "ghp_",
        "refresh_token",
        "access_token",
        ".codex/auth.json",
        "/auth.json",
        "google_application_credentials",
        "private_key",
        "client_secret",
        ".secrets",
        ".env",
        "/users/",
    ] {
        if lower.contains(forbidden) {
            return String::from("[redacted]");
        }
    }
    input.to_string()
}

fn is_secret_like_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "authorization",
        "auth",
        "authjson",
        "auth_json",
        "token",
        "access_token",
        "refresh_token",
        "secret",
        "password",
        "api_key",
        "apikey",
        "credential",
        "credentials",
        "private_key",
        "client_secret",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    keys.iter()
        .find_map(|key| object.get(*key)?.as_str().map(ToOwned::to_owned))
}

fn i64_field(value: &Value, keys: &[&str]) -> Option<i64> {
    let object = value.as_object()?;
    keys.iter().find_map(|key| object.get(*key)?.as_i64())
}

fn object_details(value: &Value) -> Option<Map<String, Value>> {
    value.as_object().cloned()
}

fn artifact_ref_from_value(value: &Value, producer: &str) -> Option<CodexWorkroomArtifactRef> {
    let path = string_field(
        value,
        &["path", "resource_ref", "resourceRef", "artifactRef"],
    )?;
    Some(CodexWorkroomArtifactRef {
        path: redact_codex_workroom_text(&path),
        digest: string_field(value, &["digest", "stable_digest", "stableDigest"]),
        mime_type: string_field(value, &["mime_type", "mimeType", "type"]),
        visibility: CodexWorkroomArtifactVisibility::Workroom,
        retention: CodexWorkroomArtifactRetention::Retained,
        producer: String::from(producer),
        closeout_ref: string_field(value, &["closeout_ref", "closeoutRef"]),
    })
}

fn artifact_refs_from_cloud_value(value: &Value) -> Vec<CodexWorkroomArtifactRef> {
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    let Some(raw_refs) = object
        .get("artifactRefs")
        .or_else(|| object.get("artifact_refs"))
        .and_then(Value::as_array)
    else {
        return artifact_ref_from_value(value, "cloud_runner")
            .into_iter()
            .collect();
    };

    raw_refs
        .iter()
        .filter_map(|item| {
            if let Some(resource_ref) = item.as_str() {
                return Some(CodexWorkroomArtifactRef {
                    path: redact_codex_workroom_text(resource_ref),
                    digest: None,
                    mime_type: None,
                    visibility: CodexWorkroomArtifactVisibility::Workroom,
                    retention: CodexWorkroomArtifactRetention::Retained,
                    producer: String::from("cloud_runner"),
                    closeout_ref: None,
                });
            }
            artifact_ref_from_value(item, "cloud_runner")
        })
        .collect()
}

fn receipt_refs_from_value(value: &Value) -> Vec<CodexWorkroomReceiptRef> {
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    if let Some(raw_refs) = object
        .get("receiptRefs")
        .or_else(|| object.get("receipt_refs"))
        .and_then(Value::as_array)
    {
        return raw_refs
            .iter()
            .filter_map(|item| {
                if let Some(resource_ref) = item.as_str() {
                    return Some(CodexWorkroomReceiptRef {
                        receipt_type: String::from("workroom.receipt"),
                        resource_ref: redact_codex_workroom_text(resource_ref),
                        digest: None,
                    });
                }
                let object = item.as_object()?;
                Some(CodexWorkroomReceiptRef {
                    receipt_type: object
                        .get("receiptType")
                        .or_else(|| object.get("receipt_type"))
                        .and_then(Value::as_str)
                        .unwrap_or("workroom.receipt")
                        .to_string(),
                    resource_ref: object
                        .get("resourceRef")
                        .or_else(|| object.get("resource_ref"))
                        .and_then(Value::as_str)
                        .map(redact_codex_workroom_text)?,
                    digest: object
                        .get("digest")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                })
            })
            .collect();
    }
    string_field(
        value,
        &["receipt_ref", "receiptRef", "closeout_ref", "closeoutRef"],
    )
    .map(|resource_ref| {
        vec![CodexWorkroomReceiptRef {
            receipt_type: String::from("workroom.closeout"),
            resource_ref: redact_codex_workroom_text(&resource_ref),
            digest: string_field(value, &["digest", "stable_digest", "stableDigest"]),
        }]
    })
    .unwrap_or_default()
}

fn failure_kind_from_str(raw: &str) -> Option<CodexWorkroomFailureKind> {
    match raw
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '.'], "_")
        .as_str()
    {
        "nonzero_exit" | "exit_code" | "codex_nonzero_exit" => {
            Some(CodexWorkroomFailureKind::NonzeroExit)
        }
        "timeout" | "timed_out" | "codex_timeout" => Some(CodexWorkroomFailureKind::Timeout),
        "cancelled" | "canceled" | "codex_cancelled" => Some(CodexWorkroomFailureKind::Cancelled),
        "auth" | "auth_failure" | "unauthorized" | "forbidden" => {
            Some(CodexWorkroomFailureKind::AuthFailure)
        }
        "setup" | "setup_failure" => Some(CodexWorkroomFailureKind::SetupFailure),
        "artifact" | "artifact_capture_failure" => {
            Some(CodexWorkroomFailureKind::ArtifactCaptureFailure)
        }
        "stream" | "stream_disconnect" => Some(CodexWorkroomFailureKind::StreamDisconnect),
        "unknown" => Some(CodexWorkroomFailureKind::Unknown),
        _ => None,
    }
}

fn failure_kind_for_event(kind: CodexWorkroomEventKind) -> CodexWorkroomFailureKind {
    match kind {
        CodexWorkroomEventKind::Timeout => CodexWorkroomFailureKind::Timeout,
        CodexWorkroomEventKind::Cancelled => CodexWorkroomFailureKind::Cancelled,
        _ => CodexWorkroomFailureKind::Unknown,
    }
}
