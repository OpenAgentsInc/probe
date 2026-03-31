use std::path::Path;

use probe_protocol::session::SessionHarnessProfile;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedHarnessProfile {
    pub profile: SessionHarnessProfile,
    pub system_prompt: String,
}

pub fn resolve_harness_profile(
    tool_set: Option<&str>,
    requested_profile: Option<&str>,
    cwd: &Path,
    operator_system: Option<&str>,
) -> Result<Option<ResolvedHarnessProfile>, String> {
    let base = match (tool_set, requested_profile) {
        (Some("coding_bootstrap"), Some("coding_bootstrap_default"))
        | (Some("coding_bootstrap"), None) => Some(coding_bootstrap_default(cwd)),
        (Some("coding_bootstrap"), Some(profile)) => {
            return Err(format!(
                "unknown harness profile for coding_bootstrap: {profile}"
            ));
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

fn coding_bootstrap_default(cwd: &Path) -> ResolvedHarnessProfile {
    let shell = if cfg!(target_family = "windows") {
        "cmd"
    } else {
        "sh"
    };
    let prompt = format!(
        "You are operating inside Probe's coding_bootstrap harness profile v1.\n\
         \n\
         Environment:\n\
         - cwd: {}\n\
         - shell: {}\n\
         - operating_system: {}\n\
         \n\
         Operating rules:\n\
         - Treat this session as one coding activity and stay focused on that activity.\n\
         - Prefer read_file, list_files, and code_search before using shell.\n\
         - Use apply_patch for deterministic text changes instead of describing edits abstractly.\n\
         - Verify relevant files after editing before claiming success.\n\
         - Keep tool usage bounded and avoid repeating large reads when a narrower read or search would do.\n\
         - If a tool returns truncated output, narrow the next call instead of guessing.\n\
         - Ground final answers in observed tool output.",
        cwd.display(),
        shell,
        std::env::consts::OS
    );
    ResolvedHarnessProfile {
        profile: SessionHarnessProfile {
            name: String::from("coding_bootstrap_default"),
            version: String::from("v1"),
        },
        system_prompt: prompt,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{render_harness_profile, resolve_harness_profile};

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
}
