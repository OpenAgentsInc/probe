use std::path::Path;

use probe_protocol::backend::BackendKind;
use probe_protocol::session::SessionHarnessProfile;
use probe_protocol::signature_context::{SessionSignatureContext, SignatureSelectorMode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::signature_registry::render_signature_set_context;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedHarnessProfile {
    pub profile: SessionHarnessProfile,
    pub system_prompt: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessCandidateManifest {
    pub schema_version: u16,
    pub candidate_id: String,
    pub tool_set: String,
    pub profile_name: String,
    pub profile_version: String,
    pub description: String,
    pub system_prompt_template: String,
    pub manifest_digest: String,
}

impl HarnessCandidateManifest {
    #[must_use]
    pub fn new(
        candidate_id: impl Into<String>,
        tool_set: impl Into<String>,
        profile_name: impl Into<String>,
        profile_version: impl Into<String>,
        description: impl Into<String>,
        system_prompt_template: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: 1,
            candidate_id: candidate_id.into(),
            tool_set: tool_set.into(),
            profile_name: profile_name.into(),
            profile_version: profile_version.into(),
            description: description.into(),
            system_prompt_template: system_prompt_template.into(),
            manifest_digest: String::new(),
        }
        .with_stable_digest()
    }

    #[must_use]
    pub fn stable_digest(&self) -> String {
        let mut digestible = self.clone();
        digestible.manifest_digest.clear();
        let mut hasher = Sha256::new();
        hasher.update(b"probe_harness_candidate_manifest|");
        hasher.update(
            serde_json::to_string(&digestible)
                .expect("harness manifest should serialize")
                .as_bytes(),
        );
        hex::encode(hasher.finalize())
    }

    #[must_use]
    pub fn with_stable_digest(mut self) -> Self {
        self.manifest_digest = self.stable_digest();
        self
    }
}

#[must_use]
pub fn builtin_harness_candidate_manifests() -> Vec<HarnessCandidateManifest> {
    vec![
        coding_bootstrap_default_manifest(),
        coding_bootstrap_codex_manifest(),
        coding_bootstrap_patch_guard_manifest(),
        coding_bootstrap_verify_first_manifest(),
    ]
}

pub fn resolve_harness_profile(
    tool_set: Option<&str>,
    requested_profile: Option<&str>,
    cwd: &Path,
    operator_system: Option<&str>,
) -> Result<Option<ResolvedHarnessProfile>, String> {
    let base = match (tool_set, requested_profile) {
        (Some("coding_bootstrap"), Some(profile)) => Some(
            builtin_harness_candidate_manifests()
                .into_iter()
                .find(|candidate| candidate.profile_name == profile)
                .map(|candidate| resolve_manifest(candidate, cwd))
                .ok_or_else(|| {
                    format!("unknown harness profile for coding_bootstrap: {profile}")
                })?,
        ),
        (Some("coding_bootstrap"), None) => {
            Some(resolve_manifest(coding_bootstrap_default_manifest(), cwd))
        }
        (Some(other), Some(profile)) => {
            return Err(format!(
                "harness profile `{profile}` is not available for tool set `{other}`"
            ));
        }
        (None, Some(profile)) => {
            return Err(format!(
                "harness profile `{profile}` requires a compatible tool set; the first supported pairing is `--tool-set coding_bootstrap --harness-profile coding_bootstrap_default`"
            ));
        }
        (_, None) => None,
    };

    Ok(base.map(|mut resolved| {
        if let Some(operator_system) = operator_system.filter(|value| !value.trim().is_empty()) {
            resolved.system_prompt = format!(
                "{}\n\nOperator Addendum:\n{}",
                resolved.system_prompt, operator_system
            );
        }
        resolved
    }))
}

#[must_use]
pub fn render_harness_profile(profile: &SessionHarnessProfile) -> String {
    format!("{}@{}", profile.name, profile.version)
}

pub fn resolve_prompt_contract(
    tool_set: Option<&str>,
    requested_profile: Option<&str>,
    cwd: &Path,
    operator_system: Option<&str>,
    backend_kind: BackendKind,
) -> Result<(Option<String>, Option<SessionHarnessProfile>), String> {
    let requested_profile =
        requested_profile.or_else(|| default_harness_profile_for_backend(tool_set, backend_kind));
    match resolve_harness_profile(tool_set, requested_profile, cwd, operator_system)? {
        Some(resolved) => Ok((Some(resolved.system_prompt), Some(resolved.profile))),
        None => Ok((
            append_operator_addendum(
                default_system_prompt_for_backend(backend_kind, cwd),
                operator_system,
            ),
            None,
        )),
    }
}

#[must_use]
pub fn attach_signature_context_to_system_prompt(
    base: Option<String>,
    signature_context: Option<&SessionSignatureContext>,
) -> Option<String> {
    let Some(addendum) = signature_context.and_then(render_signature_harness_addendum) else {
        return base;
    };
    Some(match base {
        Some(base) if !base.trim().is_empty() => {
            format!("{base}\n\nProbe Signature Addendum:\n{addendum}")
        }
        _ => format!("Probe Signature Addendum:\n{addendum}"),
    })
}

#[must_use]
pub fn render_signature_harness_addendum(
    signature_context: &SessionSignatureContext,
) -> Option<String> {
    if signature_context.signature_pack.entries.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    if let Some(decision) = signature_context.selection_decision.as_ref() {
        lines.push(format!(
            "- decision: {} mode={} digest={}",
            decision.decision_id,
            selector_mode_label(decision.selector_mode),
            decision.task_envelope_digest.as_deref().unwrap_or("none")
        ));
        if let Some(tool_set) = decision.recommended_tool_set.as_ref() {
            lines.push(format!("- recommended_tool_set: {}", compact(tool_set, 80)));
        }
        if let Some(tool_choice) = decision.recommended_tool_choice.as_ref() {
            lines.push(format!(
                "- recommended_tool_choice: {}",
                compact(tool_choice, 80)
            ));
        }
    }
    if let Some(decision) = signature_context.selection_decision.as_ref() {
        if let Some(budget_mode) = decision.budget_mode.as_ref() {
            lines.push(format!("- budget_mode: {}", compact(budget_mode, 80)));
        }
        if let Some(budget) = decision.selected_signature_budget {
            lines.push(format!("- selected_signature_budget: {budget}"));
        }
    }
    lines.push(String::from("- authority: signatures are context and evidence hints only; Probe tool policy and approvals still control all tool use."));
    lines.push(String::from("- selected_signatures:"));
    if let Some(rendered_context) = signature_context
        .selection_decision
        .as_ref()
        .and_then(|decision| decision.rendered_context.as_ref())
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(rendered_context.clone());
    } else if let Some(rendered_context) =
        render_signature_set_context(&signature_context.signature_pack.entries)
    {
        lines.push(rendered_context);
    }
    Some(lines.join("\n"))
}

fn resolve_manifest(manifest: HarnessCandidateManifest, cwd: &Path) -> ResolvedHarnessProfile {
    let shell = if cfg!(target_family = "windows") {
        "cmd"
    } else {
        "sh"
    };
    let prompt = manifest
        .system_prompt_template
        .replace("{cwd}", &cwd.display().to_string())
        .replace("{shell}", shell)
        .replace("{operating_system}", std::env::consts::OS);
    ResolvedHarnessProfile {
        profile: SessionHarnessProfile {
            name: manifest.profile_name,
            version: manifest.profile_version,
        },
        system_prompt: prompt,
    }
}

fn default_harness_profile_for_backend(
    tool_set: Option<&str>,
    backend_kind: BackendKind,
) -> Option<&'static str> {
    match (tool_set, backend_kind) {
        (Some("coding_bootstrap"), BackendKind::OpenAiCodexSubscription) => {
            Some("coding_bootstrap_codex")
        }
        _ => None,
    }
}

fn default_system_prompt_for_backend(backend_kind: BackendKind, cwd: &Path) -> Option<String> {
    match backend_kind {
        BackendKind::OpenAiCodexSubscription => Some(codex_plain_system_prompt(cwd)),
        BackendKind::OpenAiChatCompletions | BackendKind::AppleFmBridge => None,
    }
}

fn append_operator_addendum(base: Option<String>, operator_system: Option<&str>) -> Option<String> {
    let operator_system = operator_system
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match (base, operator_system) {
        (Some(base), Some(operator_system)) => {
            Some(format!("{base}\n\nOperator Addendum:\n{operator_system}"))
        }
        (Some(base), None) => Some(base),
        (None, Some(operator_system)) => Some(operator_system.to_string()),
        (None, None) => None,
    }
}

fn selector_mode_label(mode: SignatureSelectorMode) -> &'static str {
    match mode {
        SignatureSelectorMode::ExactRef => "exact_ref",
        SignatureSelectorMode::SemanticEmbedding => "semantic_embedding",
        SignatureSelectorMode::StructuredQuery => "structured_query",
        SignatureSelectorMode::Hybrid => "hybrid",
        SignatureSelectorMode::Manual => "manual",
        SignatureSelectorMode::NoMatch => "no_match",
    }
}

fn compact(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut truncated = normalized
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn codex_plain_system_prompt(cwd: &Path) -> String {
    format!(
        "You are operating inside Probe's hosted Codex backend lane.\n\
         \n\
         Environment:\n\
         - cwd: {}\n\
         - operating_system: {}\n\
         \n\
         Operating rules:\n\
         - Default to direct software-engineering help instead of long preambles.\n\
         - When the task depends on repository truth, inspect the workspace before deciding.\n\
         - Keep replies terse, concrete, and action-oriented.\n\
         - Do not claim edits, tests, or command results that you did not actually verify.\n\
         - If you edit code, prefer deterministic changes and explicit verification.\n\
         - If the request is unclear, say what is missing instead of guessing.",
        cwd.display(),
        std::env::consts::OS
    )
}

fn coding_bootstrap_default_manifest() -> HarnessCandidateManifest {
    HarnessCandidateManifest::new(
        "coding_bootstrap_default@v1",
        "coding_bootstrap",
        "coding_bootstrap_default",
        "v1",
        "Baseline Probe coding harness profile.",
        "You are operating inside Probe's coding_bootstrap harness profile v1.\n\
         \n\
         Environment:\n\
         - cwd: {cwd}\n\
         - shell: {shell}\n\
         - operating_system: {operating_system}\n\
         \n\
         Operating rules:\n\
         - Treat this session as one coding activity and stay focused on that activity.\n\
         - Answer directly when the request does not require workspace evidence or tool output.\n\
         - If the user input is unclear, noisy, or gibberish, say that plainly and ask for clarification.\n\
         - Do not invent fake limitations about being unable to read ordinary ASCII punctuation or symbols.\n\
         - Prefer read_file, list_files, and code_search before using shell.\n\
         - Do not call tools for identity, general-knowledge, or stylistic questions that you can answer honestly without local evidence.\n\
         - Use apply_patch for deterministic text changes instead of describing edits abstractly.\n\
         - Verify relevant files after editing before claiming success.\n\
         - Keep tool usage bounded and avoid repeating large reads when a narrower read or search would do.\n\
         - If a tool returns truncated output, narrow the next call instead of guessing.\n\
         - Ground final answers in observed tool output.",
    )
}

fn coding_bootstrap_codex_manifest() -> HarnessCandidateManifest {
    HarnessCandidateManifest::new(
        "coding_bootstrap_codex@v1",
        "coding_bootstrap",
        "coding_bootstrap_codex",
        "v1",
        "Codex-tuned Probe coding harness profile.",
        "You are operating inside Probe's coding_bootstrap Codex harness profile v1.\n\
         \n\
         Environment:\n\
         - cwd: {cwd}\n\
         - shell: {shell}\n\
         - operating_system: {operating_system}\n\
         \n\
         Operating rules:\n\
         - Treat this session as one coding activity and stay focused on that activity.\n\
         - Default to concise, action-oriented software-engineering help.\n\
         - Prefer read_file, list_files, and code_search before using shell.\n\
         - Use apply_patch for deterministic text changes instead of describing edits abstractly.\n\
         - Verify the relevant file or test path after editing before claiming success.\n\
         - Avoid repeating large reads when a narrower read or search would do.\n\
         - If a tool returns truncated output, narrow the next call instead of guessing.\n\
         - Do not claim edits, test results, or repo facts that you have not actually observed.\n\
         - Ground final answers in observed tool output.",
    )
}

fn coding_bootstrap_patch_guard_manifest() -> HarnessCandidateManifest {
    HarnessCandidateManifest::new(
        "coding_bootstrap_patch_guard@v1",
        "coding_bootstrap",
        "coding_bootstrap_patch_guard",
        "v1",
        "Variant that pushes harder on evidence gathering before edits.",
        "You are operating inside Probe's coding_bootstrap patch-guard harness profile v1.\n\
         \n\
         Environment:\n\
         - cwd: {cwd}\n\
         - shell: {shell}\n\
         - operating_system: {operating_system}\n\
         \n\
         Operating rules:\n\
         - Treat this session as one coding activity and stay focused on that activity.\n\
         - Answer directly when the request does not require workspace evidence or tool output.\n\
         - If the user input is unclear, noisy, or gibberish, say that plainly and ask for clarification.\n\
         - Do not invent fake limitations about being unable to read ordinary ASCII punctuation or symbols.\n\
         - Prefer read_file, list_files, and code_search before using shell.\n\
         - Do not call tools for identity, general-knowledge, or stylistic questions that you can answer honestly without local evidence.\n\
         - Do not use apply_patch until you have concrete file evidence and can name the edit target precisely.\n\
         - Verify relevant files after editing before claiming success.\n\
         - Keep tool usage bounded and avoid repeating large reads when a narrower read or search would do.\n\
         - If a tool returns truncated output, narrow the next call instead of guessing.\n\
         - Ground final answers in observed tool output.",
    )
}

fn coding_bootstrap_verify_first_manifest() -> HarnessCandidateManifest {
    HarnessCandidateManifest::new(
        "coding_bootstrap_verify_first@v1",
        "coding_bootstrap",
        "coding_bootstrap_verify_first",
        "v1",
        "Variant that explicitly foregrounds post-edit verification and risk narration.",
        "You are operating inside Probe's coding_bootstrap verify-first harness profile v1.\n\
         \n\
         Environment:\n\
         - cwd: {cwd}\n\
         - shell: {shell}\n\
         - operating_system: {operating_system}\n\
         \n\
         Operating rules:\n\
         - Treat this session as one coding activity and stay focused on that activity.\n\
         - Answer directly when the request does not require workspace evidence or tool output.\n\
         - If the user input is unclear, noisy, or gibberish, say that plainly and ask for clarification.\n\
         - Do not invent fake limitations about being unable to read ordinary ASCII punctuation or symbols.\n\
         - Prefer read_file, list_files, and code_search before using shell.\n\
         - Do not call tools for identity, general-knowledge, or stylistic questions that you can answer honestly without local evidence.\n\
         - Use apply_patch for deterministic text changes instead of describing edits abstractly.\n\
         - After every edit, schedule an explicit verification step before finalizing.\n\
         - Keep tool usage bounded and avoid repeating large reads when a narrower read or search would do.\n\
         - If a tool returns truncated output, narrow the next call instead of guessing.\n\
         - Ground final answers in observed tool output and mention the verification step you ran.",
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use probe_protocol::backend::BackendKind;
    use probe_protocol::signature_context::{
        PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION, SessionSignatureContext, SignatureAdoptionState,
        SignatureEvidenceRequirement, SignaturePack, SignaturePackEntry, SignatureRef,
        SignatureSelectionDecision, SignatureSelectionScore, SignatureSelectorMode,
    };

    use super::{
        attach_signature_context_to_system_prompt, render_harness_profile,
        render_signature_harness_addendum, resolve_harness_profile, resolve_prompt_contract,
    };

    #[test]
    fn coding_bootstrap_default_profile_is_selected_automatically() {
        let resolved = resolve_harness_profile(
            Some("coding_bootstrap"),
            None,
            Path::new("/tmp/probe"),
            None,
        )
        .expect("resolve harness")
        .expect("default harness should exist");
        assert_eq!(resolved.profile.name, "coding_bootstrap_default");
        assert_eq!(resolved.profile.version, "v1");
        assert!(resolved.system_prompt.contains("cwd: /tmp/probe"));
        assert!(
            resolved
                .system_prompt
                .contains("Do not invent fake limitations")
        );
    }

    #[test]
    fn operator_system_is_appended_to_profile_prompt() {
        let resolved = resolve_harness_profile(
            Some("coding_bootstrap"),
            Some("coding_bootstrap_default"),
            Path::new("/tmp/probe"),
            Some("Always explain the next verification step."),
        )
        .expect("resolve harness")
        .expect("default harness should exist");
        assert!(resolved.system_prompt.contains("Operator Addendum"));
        assert!(
            resolved
                .system_prompt
                .contains("Always explain the next verification step.")
        );
    }

    #[test]
    fn signature_context_addendum_is_compact_and_policy_bound() {
        let context = signature_context_fixture();
        let prompt = attach_signature_context_to_system_prompt(
            Some(String::from("Base Codex prompt.")),
            Some(&context),
        )
        .expect("signature addendum prompt");

        assert!(prompt.contains("Base Codex prompt."));
        assert!(prompt.contains("Probe Signature Addendum"));
        assert!(prompt.contains("coding.service_readiness@candidate"));
        assert!(prompt.contains("recommended_tool_choice: auto"));
        assert!(prompt.contains("budget_mode: adaptive_threshold"));
        assert!(prompt.contains("Use for:"));
        assert!(prompt.contains("Neighbor boundaries:"));
        assert!(prompt.contains("Probe tool policy and approvals still control all tool use"));
        assert!(!prompt.contains("apply_patch is required"));
    }

    #[test]
    fn no_signature_context_leaves_prompt_unchanged() {
        assert_eq!(
            attach_signature_context_to_system_prompt(Some(String::from("Base")), None),
            Some(String::from("Base"))
        );
        assert!(
            render_signature_harness_addendum(
                &SessionSignatureContext::new(SignaturePack::empty())
            )
            .is_none()
        );
    }

    #[test]
    fn harness_profile_requires_compatible_tool_set() {
        let error = resolve_harness_profile(
            None,
            Some("coding_bootstrap_default"),
            Path::new("/tmp/probe"),
            None,
        )
        .expect_err("harness profile should require a tool set");
        assert!(error.contains("requires a compatible tool set"));
    }

    #[test]
    fn harness_profile_renders_name_and_version() {
        let resolved = resolve_harness_profile(
            Some("coding_bootstrap"),
            None,
            Path::new("/tmp/probe"),
            None,
        )
        .expect("resolve harness")
        .expect("default harness should exist");
        assert_eq!(
            render_harness_profile(&resolved.profile),
            "coding_bootstrap_default@v1"
        );
    }

    #[test]
    fn codex_backend_uses_codex_harness_by_default_for_coding_bootstrap() {
        let (system_prompt, harness_profile) = resolve_prompt_contract(
            Some("coding_bootstrap"),
            None,
            Path::new("/tmp/probe"),
            None,
            BackendKind::OpenAiCodexSubscription,
        )
        .expect("resolve prompt contract");
        let harness_profile = harness_profile.expect("codex harness should exist");
        assert_eq!(harness_profile.name, "coding_bootstrap_codex");
        assert_eq!(harness_profile.version, "v1");
        assert!(
            system_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains("Codex harness profile v1"))
        );
    }

    #[test]
    fn codex_backend_uses_plain_system_prompt_without_tool_set() {
        let (system_prompt, harness_profile) = resolve_prompt_contract(
            None,
            None,
            Path::new("/tmp/probe"),
            None,
            BackendKind::OpenAiCodexSubscription,
        )
        .expect("resolve prompt contract");
        assert!(harness_profile.is_none());
        assert!(
            system_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains("hosted Codex backend lane"))
        );
    }

    fn signature_context_fixture() -> SessionSignatureContext {
        let signature = SignatureRef {
            id: String::from("coding.service_readiness"),
            version: String::from("candidate"),
            adoption_state: SignatureAdoptionState::Candidate,
            source_ref: Some(String::from("vortex://signatures/coding.service_readiness")),
        };
        SessionSignatureContext::new(SignaturePack {
            schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
            pack_id: Some(String::from("probe.seed_failure_signatures.v1")),
            selected_by: Some(String::from("probe_signature_selector")),
            selected_at_ms: None,
            max_signature_count: Some(4),
            entries: vec![SignaturePackEntry {
                signature: signature.clone(),
                task_classes: vec![String::from("service_orchestration")],
                benchmark_families: vec![String::from("terminal-bench")],
                required_evidence: vec![SignatureEvidenceRequirement {
                    kind: String::from("port_probe"),
                    required: true,
                    description: None,
                }],
                recommended_tools: Vec::new(),
                forbidden_tools: vec![String::from("destructive_shell")],
                closeout_artifacts: vec![String::from("service-readiness.json")],
                failure_fingerprints: vec![String::from("port_not_ready")],
                fixture_refs: vec![String::from("terminal-bench:2.0/pypi-server")],
                rendered_description: Some(String::from(
                    "Use only to prove service readiness with logs and bounded health checks.",
                )),
            }],
        })
        .with_selection_decision(SignatureSelectionDecision {
            schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
            decision_id: String::from("sigsel-test"),
            selector_mode: SignatureSelectorMode::Hybrid,
            task_envelope_digest: Some(String::from("sha256:test")),
            selected_signature_budget: Some(1),
            budget_mode: Some(String::from("adaptive_threshold")),
            selected_signatures: vec![SignatureSelectionScore {
                signature,
                rank: 1,
                score_bps: 9_000,
                reason_code: Some(String::from("structured_match")),
            }],
            runner_up_signatures: Vec::new(),
            rejected_high_score_signatures: Vec::new(),
            rendered_context: Some(String::from(
                "- coding.service_readiness@candidate state=candidate\n  Use for: service readiness\n  Do not use for: unrelated tasks\n  Required evidence: port_probe\n  Neighbor boundaries: single selected signature",
            )),
            recommended_harness_profile: Some(String::from("coding_bootstrap_codex@v1")),
            recommended_tool_set: Some(String::from("coding_bootstrap")),
            recommended_tool_choice: Some(String::from("auto")),
            forbidden_tools: vec![String::from("destructive_shell")],
            fallback_reason_code: None,
        })
    }
}
