use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::codex_workroom::{redact_codex_workroom_text, redact_codex_workroom_value};
use crate::session::TimestampMs;
use crate::signature_context::SessionSignatureContext;

pub const PROBE_CODEX_MANAGED_EVENT_SCHEMA_VERSION: &str = "probe.codex_managed_event.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexManagedEventType {
    RunQueued,
    RunStarted,
    RunHeartbeat,
    TurnStarted,
    MessageDelta,
    MessageCompleted,
    ToolCallStarted,
    ToolCallDelta,
    ToolCallCompleted,
    ShellCommandStarted,
    ShellOutputDelta,
    ShellCommandCompleted,
    FileEdit,
    ArtifactCreated,
    ReceiptCreated,
    UsageUnavailable,
    RunWaitingForInput,
    FailureClassified,
    ContinuationCheckpoint,
    SignaturePackSelected,
    CodexPackageRendered,
    CodexPackageValidated,
    CodexPackageLoaded,
    Redacted,
    RunFailed,
    RunTimedOut,
    RunCancelled,
    RunCompleted,
}

impl CodexManagedEventType {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::RunFailed | Self::RunTimedOut | Self::RunCancelled | Self::RunCompleted
        )
    }

    #[must_use]
    pub const fn is_content_bearing(self) -> bool {
        matches!(
            self,
            Self::MessageDelta
                | Self::MessageCompleted
                | Self::ToolCallStarted
                | Self::ToolCallDelta
                | Self::ToolCallCompleted
                | Self::ShellCommandStarted
                | Self::ShellOutputDelta
                | Self::ShellCommandCompleted
                | Self::FileEdit
                | Self::ArtifactCreated
                | Self::ReceiptCreated
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexManagedRetentionMode {
    Retained,
    MetadataOnly,
    LocalOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexManagedTrainingUse {
    Allowed,
    Denied,
    NeedsReview,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexManagedRunRef {
    pub workroom_id: String,
    pub run_id: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexManagedArtifactRef {
    pub resource_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    pub retention_mode: CodexManagedRetentionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redaction_state: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexManagedReceiptRef {
    pub receipt_type: String,
    pub resource_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexPackageEvidence {
    pub package_id: String,
    pub adapter_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_at_ms: Option<TimestampMs>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_signature_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<CodexManagedArtifactRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CodexManagedEventPayload {
    RunLifecycle {
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        details: Map<String, Value>,
    },
    TurnLifecycle {
        turn_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_sha256: Option<String>,
    },
    AssistantMessage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        role: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delta: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_sha256: Option<String>,
    },
    ToolCall {
        call_id: String,
        tool_name: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arguments: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    ShellCommand {
        command_id: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_sha256: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        argv_summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    ShellOutput {
        command_id: String,
        stream: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_sha256: Option<String>,
        #[serde(default)]
        truncated: bool,
    },
    FileEdit {
        path: String,
        operation: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_digest: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_digest: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff_sha256: Option<String>,
    },
    Artifact {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact: Option<CodexManagedArtifactRef>,
    },
    Receipt {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        receipt: Option<CodexManagedReceiptRef>,
    },
    UsageUnavailable {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    FailureClassification {
        failure_type: String,
        fingerprint: String,
        #[serde(default)]
        retryable: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        evidence_refs: Vec<String>,
    },
    SignatureContext {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature_context: Option<SessionSignatureContext>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        selected_signature_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        package_evidence: Option<CodexPackageEvidence>,
    },
    ContinuationCheckpoint {
        checkpoint_ref: String,
        after_sequence: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume_hint: Option<String>,
    },
    LocalOnlyRef {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        local_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_summary: Option<String>,
    },
    Redacted {
        reason: String,
    },
    Generic {
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        details: Map<String, Value>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexManagedEvent {
    pub schema_version: String,
    pub sequence: u64,
    pub occurred_at_ms: TimestampMs,
    pub event_type: CodexManagedEventType,
    pub run_ref: CodexManagedRunRef,
    pub retention_mode: CodexManagedRetentionMode,
    pub training_use: CodexManagedTrainingUse,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_rights_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<CodexManagedArtifactRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receipt_refs: Vec<CodexManagedReceiptRef>,
    pub payload: CodexManagedEventPayload,
    #[serde(default)]
    pub redacted: bool,
}

impl CodexManagedEvent {
    #[must_use]
    pub fn new(
        sequence: u64,
        occurred_at_ms: TimestampMs,
        event_type: CodexManagedEventType,
        run_ref: CodexManagedRunRef,
        payload: CodexManagedEventPayload,
    ) -> Self {
        let (payload, redacted) = redact_payload(payload);
        Self {
            schema_version: String::from(PROBE_CODEX_MANAGED_EVENT_SCHEMA_VERSION),
            sequence,
            occurred_at_ms,
            event_type,
            run_ref,
            retention_mode: CodexManagedRetentionMode::Retained,
            training_use: CodexManagedTrainingUse::Allowed,
            data_rights_ref: None,
            artifact_refs: Vec::new(),
            receipt_refs: Vec::new(),
            payload,
            redacted,
        }
    }

    #[must_use]
    pub fn with_policy(
        mut self,
        retention_mode: CodexManagedRetentionMode,
        training_use: CodexManagedTrainingUse,
        data_rights_ref: Option<String>,
    ) -> Self {
        self.retention_mode = retention_mode;
        self.training_use = training_use;
        self.data_rights_ref = data_rights_ref.map(|value| redact_codex_workroom_text(&value));
        if retention_mode == CodexManagedRetentionMode::LocalOnly
            && self.event_type.is_content_bearing()
        {
            self.payload = CodexManagedEventPayload::LocalOnlyRef {
                local_ref: None,
                event_summary: Some(format!("{:?}", self.event_type)),
            };
            self.artifact_refs.clear();
            self.receipt_refs.clear();
            self.redacted = true;
        }
        self
    }

    #[must_use]
    pub fn with_artifact_refs(mut self, artifact_refs: Vec<CodexManagedArtifactRef>) -> Self {
        self.artifact_refs = artifact_refs;
        self
    }

    #[must_use]
    pub fn with_receipt_refs(mut self, receipt_refs: Vec<CodexManagedReceiptRef>) -> Self {
        self.receipt_refs = receipt_refs;
        self
    }
}

#[must_use]
pub fn normalize_cloud_codex_runner_event(
    value: &Value,
    base_run_ref: &CodexManagedRunRef,
    sequence: u64,
    occurred_at_ms: TimestampMs,
) -> Option<CodexManagedEvent> {
    let input_was_redacted = redact_codex_workroom_value(value.clone()) != *value;
    let event_type = string_field(value, &["kind", "type", "event"])
        .as_deref()
        .and_then(codex_managed_event_type_from_str)?;
    let mut run_ref = base_run_ref.clone();
    if let Some(thread_id) = string_field(value, &["threadId", "thread_id"]) {
        run_ref.thread_id = Some(redact_codex_workroom_text(&thread_id));
    }
    if let Some(turn_id) = string_field(value, &["turnId", "turn_id"]) {
        run_ref.turn_id = Some(redact_codex_workroom_text(&turn_id));
    }

    let artifact_refs = artifact_refs_from_value(value);
    let receipt_refs = receipt_refs_from_value(value);
    let retention_mode =
        retention_mode_from_value(value).unwrap_or(CodexManagedRetentionMode::Retained);
    let training_use = training_use_from_value(value).unwrap_or(CodexManagedTrainingUse::Allowed);
    let data_rights_ref = string_field(value, &["dataRightsRef", "data_rights_ref"]);
    let payload = payload_from_value(event_type, value, &artifact_refs, &receipt_refs);

    let mut event = CodexManagedEvent::new(sequence, occurred_at_ms, event_type, run_ref, payload)
        .with_artifact_refs(artifact_refs)
        .with_receipt_refs(receipt_refs)
        .with_policy(retention_mode, training_use, data_rights_ref);
    event.redacted |= input_was_redacted;
    Some(event)
}

#[must_use]
pub fn codex_managed_event_type_from_str(raw: &str) -> Option<CodexManagedEventType> {
    let normalized = raw.trim().to_ascii_lowercase().replace(['-', '.'], "_");
    match normalized.as_str() {
        "run_queued" => Some(CodexManagedEventType::RunQueued),
        "run_started" => Some(CodexManagedEventType::RunStarted),
        "run_heartbeat" => Some(CodexManagedEventType::RunHeartbeat),
        "turn_started" => Some(CodexManagedEventType::TurnStarted),
        "message_delta" | "assistant_delta" | "text_delta" => {
            Some(CodexManagedEventType::MessageDelta)
        }
        "message_completed" | "assistant_message" | "assistant_completed" => {
            Some(CodexManagedEventType::MessageCompleted)
        }
        "tool_call_started" => Some(CodexManagedEventType::ToolCallStarted),
        "tool_call_delta" => Some(CodexManagedEventType::ToolCallDelta),
        "tool_call_completed" => Some(CodexManagedEventType::ToolCallCompleted),
        "shell_command_started" => Some(CodexManagedEventType::ShellCommandStarted),
        "shell_output_delta" => Some(CodexManagedEventType::ShellOutputDelta),
        "shell_command_completed" => Some(CodexManagedEventType::ShellCommandCompleted),
        "file_edit" => Some(CodexManagedEventType::FileEdit),
        "artifact_created" | "artifact_ref" => Some(CodexManagedEventType::ArtifactCreated),
        "receipt_created" | "receipt_ref" => Some(CodexManagedEventType::ReceiptCreated),
        "usage_unavailable" => Some(CodexManagedEventType::UsageUnavailable),
        "run_waiting_for_input" => Some(CodexManagedEventType::RunWaitingForInput),
        "failure_classified" => Some(CodexManagedEventType::FailureClassified),
        "continuation_checkpoint" => Some(CodexManagedEventType::ContinuationCheckpoint),
        "signature_pack_selected" | "signature_context_selected" => {
            Some(CodexManagedEventType::SignaturePackSelected)
        }
        "codex_package_rendered" => Some(CodexManagedEventType::CodexPackageRendered),
        "codex_package_validated" => Some(CodexManagedEventType::CodexPackageValidated),
        "codex_package_loaded" => Some(CodexManagedEventType::CodexPackageLoaded),
        "redacted" => Some(CodexManagedEventType::Redacted),
        "run_failed" => Some(CodexManagedEventType::RunFailed),
        "run_timed_out" | "run_timeout" => Some(CodexManagedEventType::RunTimedOut),
        "run_cancelled" | "run_canceled" => Some(CodexManagedEventType::RunCancelled),
        "run_completed" => Some(CodexManagedEventType::RunCompleted),
        _ => None,
    }
}

fn payload_from_value(
    event_type: CodexManagedEventType,
    value: &Value,
    artifact_refs: &[CodexManagedArtifactRef],
    receipt_refs: &[CodexManagedReceiptRef],
) -> CodexManagedEventPayload {
    match event_type {
        CodexManagedEventType::RunQueued
        | CodexManagedEventType::RunStarted
        | CodexManagedEventType::RunHeartbeat
        | CodexManagedEventType::RunWaitingForInput
        | CodexManagedEventType::RunFailed
        | CodexManagedEventType::RunTimedOut
        | CodexManagedEventType::RunCancelled
        | CodexManagedEventType::RunCompleted => CodexManagedEventPayload::RunLifecycle {
            status: format!("{event_type:?}"),
            message: string_field(value, &["message", "summary", "detail"])
                .map(|text| redact_codex_workroom_text(&text)),
            details: redacted_details(value),
        },
        CodexManagedEventType::TurnStarted => CodexManagedEventPayload::TurnLifecycle {
            turn_id: string_field(value, &["turnId", "turn_id"])
                .unwrap_or_else(|| String::from("turn-unknown")),
            prompt_sha256: string_field(value, &["promptSha256", "prompt_sha256"]),
        },
        CodexManagedEventType::MessageDelta | CodexManagedEventType::MessageCompleted => {
            CodexManagedEventPayload::AssistantMessage {
                message_id: string_field(value, &["messageId", "message_id", "itemId", "item_id"]),
                role: string_field(value, &["role"]).unwrap_or_else(|| String::from("assistant")),
                delta: string_field(value, &["delta"])
                    .map(|text| redact_codex_workroom_text(&text)),
                content: string_field(value, &["content", "message", "summary"])
                    .map(|text| redact_codex_workroom_text(&text)),
                content_sha256: string_field(value, &["contentSha256", "content_sha256"]),
            }
        }
        CodexManagedEventType::ToolCallStarted
        | CodexManagedEventType::ToolCallDelta
        | CodexManagedEventType::ToolCallCompleted => CodexManagedEventPayload::ToolCall {
            call_id: string_field(value, &["callId", "call_id"])
                .unwrap_or_else(|| String::from("tool-call-unknown")),
            tool_name: string_field(value, &["toolName", "tool_name", "name"])
                .unwrap_or_else(|| String::from("unknown_tool")),
            status: format!("{event_type:?}"),
            arguments: value
                .get("arguments")
                .or_else(|| value.get("args"))
                .cloned()
                .map(redact_codex_workroom_value),
            result: value
                .get("result")
                .or_else(|| value.get("output"))
                .cloned()
                .map(redact_codex_workroom_value),
            summary: string_field(value, &["summary"])
                .map(|text| redact_codex_workroom_text(&text)),
        },
        CodexManagedEventType::ShellCommandStarted
        | CodexManagedEventType::ShellCommandCompleted => CodexManagedEventPayload::ShellCommand {
            command_id: string_field(value, &["commandId", "command_id"])
                .unwrap_or_else(|| String::from("command-unknown")),
            status: format!("{event_type:?}"),
            command_sha256: string_field(value, &["commandSha256", "command_sha256"]),
            argv_summary: string_field(value, &["argvSummary", "argv_summary", "command"])
                .map(|text| redact_codex_workroom_text(&text)),
            cwd: string_field(value, &["cwd"]).map(|text| redact_codex_workroom_text(&text)),
            exit_code: i64_field(value, &["exitCode", "exit_code"])
                .and_then(|code| i32::try_from(code).ok()),
            duration_ms: i64_field(value, &["durationMs", "duration_ms"])
                .and_then(|value| u64::try_from(value).ok()),
        },
        CodexManagedEventType::ShellOutputDelta => CodexManagedEventPayload::ShellOutput {
            command_id: string_field(value, &["commandId", "command_id"])
                .unwrap_or_else(|| String::from("command-unknown")),
            stream: string_field(value, &["stream"]).unwrap_or_else(|| String::from("stdout")),
            text: string_field(value, &["text", "delta", "output"])
                .map(|text| redact_codex_workroom_text(&text)),
            text_sha256: string_field(value, &["textSha256", "text_sha256"]),
            truncated: bool_field(value, &["truncated"]).unwrap_or(false),
        },
        CodexManagedEventType::FileEdit => CodexManagedEventPayload::FileEdit {
            path: string_field(value, &["path"]).map_or_else(
                || String::from("unknown"),
                |path| redact_codex_workroom_text(&path),
            ),
            operation: string_field(value, &["operation"])
                .unwrap_or_else(|| String::from("update")),
            before_digest: string_field(value, &["beforeDigest", "before_digest"]),
            after_digest: string_field(value, &["afterDigest", "after_digest", "digest"]),
            diff_sha256: string_field(value, &["diffSha256", "diff_sha256"]),
        },
        CodexManagedEventType::ArtifactCreated => CodexManagedEventPayload::Artifact {
            artifact: artifact_refs.first().cloned(),
        },
        CodexManagedEventType::ReceiptCreated => CodexManagedEventPayload::Receipt {
            receipt: receipt_refs.first().cloned(),
        },
        CodexManagedEventType::UsageUnavailable => CodexManagedEventPayload::UsageUnavailable {
            reason: string_field(value, &["reason", "message"])
                .unwrap_or_else(|| String::from("usage_not_reported")),
            provider: string_field(value, &["provider"]),
            model: string_field(value, &["model"]),
        },
        CodexManagedEventType::FailureClassified => {
            CodexManagedEventPayload::FailureClassification {
                failure_type: string_field(value, &["failureType", "failure_type"])
                    .unwrap_or_else(|| String::from("unknown")),
                fingerprint: string_field(value, &["fingerprint"])
                    .unwrap_or_else(|| String::from("unknown")),
                retryable: bool_field(value, &["retryable"]).unwrap_or(false),
                evidence_refs: string_vec_field(value, &["evidenceRefs", "evidence_refs"]),
            }
        }
        CodexManagedEventType::SignaturePackSelected
        | CodexManagedEventType::CodexPackageRendered
        | CodexManagedEventType::CodexPackageValidated
        | CodexManagedEventType::CodexPackageLoaded => CodexManagedEventPayload::SignatureContext {
            signature_context: value
                .get("signatureContext")
                .or_else(|| value.get("signature_context"))
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok()),
            selected_signature_ids: string_vec_field(
                value,
                &["selectedSignatureIds", "selected_signature_ids"],
            ),
            package_evidence: codex_package_evidence_from_value(value),
        },
        CodexManagedEventType::ContinuationCheckpoint => {
            CodexManagedEventPayload::ContinuationCheckpoint {
                checkpoint_ref: string_field(value, &["checkpointRef", "checkpoint_ref"])
                    .unwrap_or_else(|| String::from("probe://checkpoints/unknown")),
                after_sequence: i64_field(value, &["afterSequence", "after_sequence"])
                    .and_then(|value| u64::try_from(value).ok())
                    .unwrap_or(0),
                resume_hint: string_field(value, &["resumeHint", "resume_hint"])
                    .map(|text| redact_codex_workroom_text(&text)),
            }
        }
        CodexManagedEventType::Redacted => CodexManagedEventPayload::Redacted {
            reason: string_field(value, &["reason", "message"])
                .unwrap_or_else(|| String::from("redacted_by_runner")),
        },
    }
}

fn codex_package_evidence_from_value(value: &Value) -> Option<CodexPackageEvidence> {
    let package = value
        .get("packageEvidence")
        .or_else(|| value.get("package_evidence"))
        .unwrap_or(value);
    let package_id = string_field(package, &["packageId", "package_id"])?;
    Some(CodexPackageEvidence {
        package_id,
        adapter_kind: string_field(package, &["adapterKind", "adapter_kind"])
            .unwrap_or_else(|| String::from("codex_skill_adapter")),
        rendered_ref: string_field(package, &["renderedRef", "rendered_ref"])
            .map(|text| redact_codex_workroom_text(&text)),
        rendered_digest: string_field(package, &["renderedDigest", "rendered_digest"]),
        validation_status: string_field(package, &["validationStatus", "validation_status"]),
        loaded_at_ms: i64_field(package, &["loadedAtMs", "loaded_at_ms"])
            .and_then(|value| u64::try_from(value).ok()),
        source_signature_ids: string_vec_field(
            package,
            &["sourceSignatureIds", "source_signature_ids"],
        ),
        evidence_refs: evidence_refs_from_value(package),
    })
}

fn evidence_refs_from_value(value: &Value) -> Vec<CodexManagedArtifactRef> {
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    let Some(raw_refs) = object
        .get("evidenceRefs")
        .or_else(|| object.get("evidence_refs"))
        .and_then(Value::as_array)
    else {
        return artifact_refs_from_value(value);
    };
    raw_refs
        .iter()
        .filter_map(artifact_ref_from_value)
        .collect()
}

fn artifact_refs_from_value(value: &Value) -> Vec<CodexManagedArtifactRef> {
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    let raw_refs = object
        .get("artifactRefs")
        .or_else(|| object.get("artifact_refs"))
        .and_then(Value::as_array);
    if let Some(raw_refs) = raw_refs {
        return raw_refs
            .iter()
            .filter_map(artifact_ref_from_value)
            .collect::<Vec<_>>();
    }
    artifact_ref_from_value(value).into_iter().collect()
}

fn artifact_ref_from_value(value: &Value) -> Option<CodexManagedArtifactRef> {
    if let Some(resource_ref) = value.as_str() {
        return Some(CodexManagedArtifactRef {
            resource_ref: redact_codex_workroom_text(resource_ref),
            digest: None,
            kind: None,
            visibility: None,
            retention_mode: CodexManagedRetentionMode::Retained,
            redaction_state: None,
        });
    }
    let resource_ref = string_field(value, &["resourceRef", "resource_ref", "path", "url"])?;
    Some(CodexManagedArtifactRef {
        resource_ref: redact_codex_workroom_text(&resource_ref),
        digest: string_field(value, &["digest", "stableDigest", "stable_digest"]),
        kind: string_field(
            value,
            &[
                "kind",
                "artifactKind",
                "artifact_kind",
                "mimeType",
                "mime_type",
            ],
        ),
        visibility: string_field(value, &["visibility"]),
        retention_mode: retention_mode_from_value(value)
            .unwrap_or(CodexManagedRetentionMode::Retained),
        redaction_state: string_field(value, &["redactionState", "redaction_state"]),
    })
}

fn receipt_refs_from_value(value: &Value) -> Vec<CodexManagedReceiptRef> {
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
            .filter_map(receipt_ref_from_value)
            .collect::<Vec<_>>();
    }
    receipt_ref_from_value(value).into_iter().collect()
}

fn receipt_ref_from_value(value: &Value) -> Option<CodexManagedReceiptRef> {
    if let Some(resource_ref) = value.as_str() {
        return Some(CodexManagedReceiptRef {
            receipt_type: String::from("workroom.receipt"),
            resource_ref: redact_codex_workroom_text(resource_ref),
            digest: None,
        });
    }
    let resource_ref = string_field(
        value,
        &["resourceRef", "resource_ref", "receiptRef", "receipt_ref"],
    )?;
    Some(CodexManagedReceiptRef {
        receipt_type: string_field(value, &["receiptType", "receipt_type"])
            .unwrap_or_else(|| String::from("workroom.receipt")),
        resource_ref: redact_codex_workroom_text(&resource_ref),
        digest: string_field(value, &["digest", "stableDigest", "stable_digest"]),
    })
}

fn redacted_details(value: &Value) -> Map<String, Value> {
    match redact_codex_workroom_value(value.clone()) {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn redact_payload(payload: CodexManagedEventPayload) -> (CodexManagedEventPayload, bool) {
    let raw = serde_json::to_value(&payload).unwrap_or(Value::Null);
    let redacted = redact_codex_workroom_value(raw.clone());
    let payload =
        serde_json::from_value(redacted.clone()).unwrap_or(CodexManagedEventPayload::Generic {
            details: Map::new(),
        });
    (payload, raw != redacted)
}

fn retention_mode_from_value(value: &Value) -> Option<CodexManagedRetentionMode> {
    let raw = string_field(value, &["retentionMode", "retention_mode"])?;
    match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "retained" => Some(CodexManagedRetentionMode::Retained),
        "metadata_only" => Some(CodexManagedRetentionMode::MetadataOnly),
        "local_only" => Some(CodexManagedRetentionMode::LocalOnly),
        _ => None,
    }
}

fn training_use_from_value(value: &Value) -> Option<CodexManagedTrainingUse> {
    let raw = string_field(value, &["trainingUse", "training_use"])?;
    match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "allowed" => Some(CodexManagedTrainingUse::Allowed),
        "denied" => Some(CodexManagedTrainingUse::Denied),
        "needs_review" => Some(CodexManagedTrainingUse::NeedsReview),
        _ => None,
    }
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    keys.iter()
        .find_map(|key| object.get(*key)?.as_str().map(ToOwned::to_owned))
}

fn string_vec_field(value: &Value, keys: &[&str]) -> Vec<String> {
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    let Some(values) = keys
        .iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    values
        .iter()
        .filter_map(Value::as_str)
        .map(|value| redact_codex_workroom_text(value))
        .collect()
}

fn i64_field(value: &Value, keys: &[&str]) -> Option<i64> {
    let object = value.as_object()?;
    keys.iter().find_map(|key| object.get(*key)?.as_i64())
}

fn bool_field(value: &Value, keys: &[&str]) -> Option<bool> {
    let object = value.as_object()?;
    keys.iter().find_map(|key| object.get(*key)?.as_bool())
}
