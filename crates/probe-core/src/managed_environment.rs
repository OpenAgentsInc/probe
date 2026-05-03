use std::fmt::{Display, Formatter};

use probe_protocol::managed_environment::{
    ManagedEnvironmentCapabilities, ManagedEnvironmentCompatibilityReport,
    ManagedEnvironmentCompatibilityStatus, ManagedEnvironmentConstraints,
    ManagedEnvironmentIncompatibilityReason, ManagedEnvironmentRequiredLanguage,
    ManagedEnvironmentRequiredTool, ManagedEnvironmentWorkerAdvertisement, incompatibility_reason,
};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedEnvironmentMatchError {
    NoCompatibleWorker {
        reports: Vec<ManagedEnvironmentCompatibilityReport>,
    },
}

impl Display for ManagedEnvironmentMatchError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCompatibleWorker { reports } => {
                write!(
                    f,
                    "no managed environment worker satisfies constraints; checked {} workers",
                    reports.len()
                )
            }
        }
    }
}

impl std::error::Error for ManagedEnvironmentMatchError {}

#[must_use]
pub fn evaluate_managed_environment_compatibility(
    advertisement: &ManagedEnvironmentWorkerAdvertisement,
    constraints: &ManagedEnvironmentConstraints,
) -> ManagedEnvironmentCompatibilityReport {
    let capabilities = &advertisement.capabilities;
    let mut reasons = Vec::new();

    if let Some(required_class) = constraints.environment_class.as_ref()
        && capabilities.environment_class != *required_class
    {
        reasons.push(reason(
            "environment_class_mismatch",
            "environmentClass",
            "worker environment class does not match the requested environment",
            Some(required_class),
            Some(capabilities.environment_class.as_str()),
        ));
    }

    if !constraints.allowed_providers.is_empty()
        && !constraints
            .allowed_providers
            .contains(&capabilities.provider)
    {
        reasons.push(reason(
            "provider_not_allowed",
            "provider",
            "worker provider is not allowed by the environment record",
            Some(&constraints.allowed_providers),
            Some(&capabilities.provider),
        ));
    }

    if !constraints.allowed_host_classes.is_empty()
        && !constraints
            .allowed_host_classes
            .contains(&capabilities.host_class)
    {
        reasons.push(reason(
            "host_class_not_allowed",
            "hostClass",
            "worker host class is not allowed by the environment record",
            Some(&constraints.allowed_host_classes),
            Some(&capabilities.host_class),
        ));
    }

    if !constraints.allowed_network_egress.is_empty()
        && !constraints
            .allowed_network_egress
            .contains(&capabilities.network_egress)
    {
        reasons.push(reason(
            "network_egress_not_allowed",
            "networkEgress",
            "worker network egress policy is not allowed by the environment record",
            Some(&constraints.allowed_network_egress),
            Some(&capabilities.network_egress),
        ));
    }

    compare_limit(
        &mut reasons,
        "cpu_millicores",
        "minResources.cpuMillicores",
        constraints.min_resources.cpu_millicores.map(u64::from),
        capabilities.resource_limits.cpu_millicores.map(u64::from),
    );
    compare_limit(
        &mut reasons,
        "memory_mib",
        "minResources.memoryMib",
        constraints.min_resources.memory_mib,
        capabilities.resource_limits.memory_mib,
    );
    compare_limit(
        &mut reasons,
        "disk_mib",
        "minResources.diskMib",
        constraints.min_resources.disk_mib,
        capabilities.resource_limits.disk_mib,
    );
    compare_limit(
        &mut reasons,
        "gpu_count",
        "minResources.gpuCount",
        constraints.min_resources.gpu_count.map(u64::from),
        capabilities.resource_limits.gpu_count.map(u64::from),
    );

    for language in &constraints.required_languages {
        if !supports_language(capabilities, language) {
            reasons.push(reason(
                "missing_language",
                "requiredLanguages",
                "worker does not advertise the required language/version",
                Some(language),
                Some(&capabilities.languages),
            ));
        }
    }

    for tool in &constraints.required_tools {
        if !supports_tool(capabilities, tool) {
            reasons.push(reason(
                "missing_tool",
                "requiredTools",
                "worker does not advertise the required tool/version/risk capability",
                Some(tool),
                Some(&capabilities.tools),
            ));
        }
    }

    for profile in &constraints.required_backend_profiles {
        if !capabilities
            .backend_profiles
            .iter()
            .any(|offered| offered == profile)
        {
            reasons.push(reason(
                "missing_backend_profile",
                "requiredBackendProfiles",
                "worker does not advertise the required backend profile",
                Some(profile),
                Some(&capabilities.backend_profiles),
            ));
        }
    }

    if let Some(required) = constraints.working_directory
        && capabilities.working_directory != required
    {
        reasons.push(reason(
            "working_directory_policy_mismatch",
            "workingDirectory",
            "worker working-directory policy does not match the environment record",
            Some(&required),
            Some(&capabilities.working_directory),
        ));
    }

    if let Some(required) = constraints.package_cache
        && capabilities.package_cache != required
    {
        reasons.push(reason(
            "package_cache_policy_mismatch",
            "packageCache",
            "worker package-cache policy does not match the environment record",
            Some(&required),
            Some(&capabilities.package_cache),
        ));
    }

    if let Some(required) = constraints.persistence
        && capabilities.persistence != required
    {
        reasons.push(reason(
            "persistence_policy_mismatch",
            "persistence",
            "worker persistence policy does not match the environment record",
            Some(&required),
            Some(&capabilities.persistence),
        ));
    }

    if let Some(required) = constraints.checkpoint
        && capabilities.checkpoint != required
    {
        reasons.push(reason(
            "checkpoint_policy_mismatch",
            "checkpoint",
            "worker checkpoint policy does not match the environment record",
            Some(&required),
            Some(&capabilities.checkpoint),
        ));
    }

    for label in &constraints.required_labels {
        if !capabilities.labels.iter().any(|offered| offered == label) {
            reasons.push(reason(
                "missing_label",
                "requiredLabels",
                "worker does not advertise the required label",
                Some(label),
                Some(&capabilities.labels),
            ));
        }
    }

    ManagedEnvironmentCompatibilityReport {
        status: if reasons.is_empty() {
            ManagedEnvironmentCompatibilityStatus::Compatible
        } else {
            ManagedEnvironmentCompatibilityStatus::Incompatible
        },
        worker_id: Some(advertisement.worker_id.clone()),
        environment_class: capabilities.environment_class.clone(),
        provider: capabilities.provider,
        host_class: capabilities.host_class,
        reasons,
    }
}

pub fn select_compatible_managed_environment(
    advertisements: &[ManagedEnvironmentWorkerAdvertisement],
    constraints: &ManagedEnvironmentConstraints,
) -> Result<ManagedEnvironmentCompatibilityReport, ManagedEnvironmentMatchError> {
    let mut reports = Vec::new();
    for advertisement in advertisements {
        let report = evaluate_managed_environment_compatibility(advertisement, constraints);
        if report.is_compatible() {
            return Ok(report);
        }
        reports.push(report);
    }
    Err(ManagedEnvironmentMatchError::NoCompatibleWorker { reports })
}

fn supports_language(
    capabilities: &ManagedEnvironmentCapabilities,
    required: &ManagedEnvironmentRequiredLanguage,
) -> bool {
    capabilities.languages.iter().any(|offered| {
        offered
            .language
            .eq_ignore_ascii_case(required.language.as_str())
            && (required.versions.is_empty()
                || required
                    .versions
                    .iter()
                    .any(|version| offered.versions.iter().any(|offered| offered == version)))
    })
}

fn supports_tool(
    capabilities: &ManagedEnvironmentCapabilities,
    required: &ManagedEnvironmentRequiredTool,
) -> bool {
    capabilities.tools.iter().any(|offered| {
        offered.name == required.name
            && (required.versions.is_empty()
                || required
                    .versions
                    .iter()
                    .any(|version| offered.versions.iter().any(|offered| offered == version)))
            && required
                .risk_class
                .is_none_or(|risk_class| offered.risk_classes.contains(&risk_class))
    })
}

fn compare_limit(
    reasons: &mut Vec<ManagedEnvironmentIncompatibilityReason>,
    code: &str,
    field: &str,
    required: Option<u64>,
    offered: Option<u64>,
) {
    let Some(required) = required else {
        return;
    };
    if offered.is_some_and(|offered| offered >= required) {
        return;
    }
    reasons.push(incompatibility_reason(
        format!("insufficient_{code}"),
        field,
        "worker does not advertise enough capacity for the requested environment",
        Some(Value::from(required)),
        offered.map(Value::from),
    ));
}

fn reason<R: Serialize, O: Serialize>(
    code: &'static str,
    field: &'static str,
    message: &'static str,
    required: Option<R>,
    offered: Option<O>,
) -> ManagedEnvironmentIncompatibilityReason {
    incompatibility_reason(
        code,
        field,
        message,
        required.and_then(to_value),
        offered.and_then(to_value),
    )
}

fn to_value<T: Serialize>(value: T) -> Option<Value> {
    serde_json::to_value(value).ok()
}

#[cfg(test)]
mod tests {
    use probe_protocol::managed_environment::{
        ManagedEnvironmentCapabilities, ManagedEnvironmentConstraints, ManagedEnvironmentHostClass,
        ManagedEnvironmentLanguageCapability, ManagedEnvironmentNetworkEgressPolicy,
        ManagedEnvironmentProviderKind, ManagedEnvironmentRequiredLanguage,
        ManagedEnvironmentRequiredTool, ManagedEnvironmentResourceLimits,
        ManagedEnvironmentToolCapability, ManagedEnvironmentWorkerAdvertisement,
    };
    use probe_protocol::session::ToolRiskClass;

    use super::{
        evaluate_managed_environment_compatibility, select_compatible_managed_environment,
    };

    #[test]
    fn gcp_worker_pool_satisfies_provider_neutral_constraints() {
        let advertisement = ManagedEnvironmentWorkerAdvertisement::new(
            "worker-gcp-1",
            1_777_777_777_000,
            rust_gcp_worker(),
        );
        let constraints = coding_constraints();

        let report = evaluate_managed_environment_compatibility(&advertisement, &constraints);

        assert!(report.is_compatible(), "{:?}", report.reasons);
        assert_eq!(report.worker_id.as_deref(), Some("worker-gcp-1"));
    }

    #[test]
    fn missing_capabilities_return_actionable_reasons() {
        let mut pylon = ManagedEnvironmentCapabilities::pylon_hosted("pylon-1", "small-pylon");
        pylon.resource_limits = ManagedEnvironmentResourceLimits {
            cpu_millicores: Some(500),
            memory_mib: Some(512),
            disk_mib: Some(2_048),
            gpu_count: None,
        };
        let advertisement =
            ManagedEnvironmentWorkerAdvertisement::new("pylon-1", 1_777_777_777_000, pylon);

        let report =
            evaluate_managed_environment_compatibility(&advertisement, &coding_constraints());

        assert!(!report.is_compatible());
        let codes = report
            .reasons
            .iter()
            .map(|reason| reason.code.as_str())
            .collect::<Vec<_>>();
        assert!(codes.contains(&"provider_not_allowed"));
        assert!(codes.contains(&"insufficient_cpu_millicores"));
        assert!(codes.contains(&"missing_language"));
        assert!(codes.contains(&"missing_tool"));
    }

    #[test]
    fn selector_returns_first_compatible_worker_after_rejections() {
        let advertisements = vec![
            ManagedEnvironmentWorkerAdvertisement::new(
                "pylon-1",
                1_777_777_777_000,
                ManagedEnvironmentCapabilities::pylon_hosted("pylon-1", "small-pylon"),
            ),
            ManagedEnvironmentWorkerAdvertisement::new(
                "worker-gcp-1",
                1_777_777_777_001,
                rust_gcp_worker(),
            ),
        ];

        let report = select_compatible_managed_environment(&advertisements, &coding_constraints())
            .expect("gcp worker should match");

        assert_eq!(report.worker_id.as_deref(), Some("worker-gcp-1"));
    }

    fn coding_constraints() -> ManagedEnvironmentConstraints {
        ManagedEnvironmentConstraints {
            allowed_providers: vec![ManagedEnvironmentProviderKind::GoogleCloud],
            allowed_host_classes: vec![ManagedEnvironmentHostClass::CloudRunWorkerPool],
            allowed_network_egress: vec![ManagedEnvironmentNetworkEgressPolicy::Restricted],
            min_resources: ManagedEnvironmentResourceLimits {
                cpu_millicores: Some(2_000),
                memory_mib: Some(4_096),
                disk_mib: Some(20_480),
                gpu_count: None,
            },
            required_languages: vec![ManagedEnvironmentRequiredLanguage {
                language: String::from("rust"),
                versions: vec![String::from("1.86")],
            }],
            required_tools: vec![
                ManagedEnvironmentRequiredTool {
                    name: String::from("git"),
                    versions: Vec::new(),
                    risk_class: Some(ToolRiskClass::ReadOnly),
                },
                ManagedEnvironmentRequiredTool {
                    name: String::from("shell"),
                    versions: Vec::new(),
                    risk_class: Some(ToolRiskClass::Write),
                },
            ],
            required_backend_profiles: vec![String::from("openai-codex-subscription")],
            ..ManagedEnvironmentConstraints::empty()
        }
    }

    fn rust_gcp_worker() -> ManagedEnvironmentCapabilities {
        let mut capabilities =
            ManagedEnvironmentCapabilities::gcp_cloud_run_worker_pool("worker-gcp-1", "gcp-rust");
        capabilities.resource_limits = ManagedEnvironmentResourceLimits {
            cpu_millicores: Some(4_000),
            memory_mib: Some(8_192),
            disk_mib: Some(51_200),
            gpu_count: None,
        };
        capabilities.languages = vec![ManagedEnvironmentLanguageCapability {
            language: String::from("rust"),
            versions: vec![String::from("1.86")],
            default_version: Some(String::from("1.86")),
        }];
        capabilities.tools = vec![
            ManagedEnvironmentToolCapability {
                name: String::from("git"),
                kind: String::from("vcs"),
                versions: Vec::new(),
                risk_classes: vec![ToolRiskClass::ReadOnly],
            },
            ManagedEnvironmentToolCapability {
                name: String::from("shell"),
                kind: String::from("executor"),
                versions: Vec::new(),
                risk_classes: vec![ToolRiskClass::ShellReadOnly, ToolRiskClass::Write],
            },
        ];
        capabilities.backend_profiles = vec![String::from("openai-codex-subscription")];
        capabilities
    }
}
