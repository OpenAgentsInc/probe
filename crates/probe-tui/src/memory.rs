use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const USER_MEMORY_RELATIVE_PATH: [&str; 2] = ["memory", "USER.md"];
const REPO_MEMORY_FILE: &str = "PROBE.md";
const LEGACY_REPO_MEMORY_FILE: &str = "AGENTS.md";
const MAX_MEMORY_CHARS: usize = 4_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryLayerKind {
    User,
    Repo,
    Directory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    User,
    Repo,
    Directory,
}

impl MemoryScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user memory",
            Self::Repo => "repo memory",
            Self::Directory => "folder memory",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryLayer {
    pub kind: MemoryLayerKind,
    pub label: String,
    pub path: PathBuf,
    pub source_label: String,
    pub body: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeMemoryStack {
    pub layers: Vec<MemoryLayer>,
    pub issues: Vec<String>,
    pub suggested_user_path: Option<PathBuf>,
    pub suggested_repo_path: Option<PathBuf>,
    pub suggested_directory_path: Option<PathBuf>,
}

impl ProbeMemoryStack {
    pub fn active_label(&self) -> String {
        let has_user = self
            .layers
            .iter()
            .any(|layer| layer.kind == MemoryLayerKind::User);
        let repo_layer = self
            .layers
            .iter()
            .find(|layer| layer.kind == MemoryLayerKind::Repo);
        let directory_count = self
            .layers
            .iter()
            .filter(|layer| layer.kind == MemoryLayerKind::Directory)
            .count();

        let mut parts = Vec::new();
        if has_user {
            parts.push(String::from("user"));
        }
        if let Some(repo_layer) = repo_layer {
            if repo_layer.source_label.contains(LEGACY_REPO_MEMORY_FILE) {
                parts.push(String::from("repo/AGENTS"));
            } else {
                parts.push(String::from("repo"));
            }
        }
        if directory_count == 1 {
            parts.push(String::from("1 dir"));
        } else if directory_count > 1 {
            parts.push(format!("{directory_count} dir"));
        }

        if parts.is_empty() {
            String::from("none")
        } else {
            parts.join(" + ")
        }
    }

    pub fn first_issue_line(&self) -> Option<&str> {
        self.issues.first().map(String::as_str)
    }

    pub fn layer_for_scope(&self, scope: MemoryScope) -> Option<&MemoryLayer> {
        match scope {
            MemoryScope::User => self
                .layers
                .iter()
                .find(|layer| layer.kind == MemoryLayerKind::User),
            MemoryScope::Repo => self
                .layers
                .iter()
                .find(|layer| layer.kind == MemoryLayerKind::Repo),
            MemoryScope::Directory => self
                .suggested_directory_path
                .as_ref()
                .and_then(|path| self.layers.iter().find(|layer| layer.path == *path)),
        }
    }

    pub fn editable_path_for_scope(&self, scope: MemoryScope) -> Option<PathBuf> {
        self.layer_for_scope(scope)
            .map(|layer| layer.path.clone())
            .or_else(|| match scope {
                MemoryScope::User => self.suggested_user_path.clone(),
                MemoryScope::Repo => self.suggested_repo_path.clone(),
                MemoryScope::Directory => self.suggested_directory_path.clone(),
            })
    }

    pub fn recovery_hint_for_scope(&self, scope: MemoryScope) -> Option<String> {
        let scope_label = scope.label();
        if let Some(layer) = self.layer_for_scope(scope)
            && layer.truncated
        {
            return Some(format!(
                "Probe truncated the loaded {scope_label} preview. Open it here to review or replace the full file."
            ));
        }
        self.first_issue_line().map(|_| {
            format!(
                "Probe had trouble loading some memory files. You can open {scope_label} here and save valid UTF-8 markdown text to recover."
            )
        })
    }

    pub fn prompt_addendum(&self) -> Option<String> {
        if self.layers.is_empty() {
            return None;
        }

        let mut parts = vec![String::from(
            "Persistent memory and rules are active for this turn. Follow them unless the operator explicitly overrides them.",
        )];
        parts.push(String::from(
            "If these layers conflict, prefer narrower directory guidance over repo guidance, and repo guidance over general user preferences for repo-specific work.",
        ));

        for layer in &self.layers {
            parts.push(format!(
                "[{} memory] {}\n{}",
                layer.label,
                layer.path.display(),
                layer.body
            ));
        }

        Some(parts.join("\n\n"))
    }
}

pub fn load_probe_memory_stack(probe_home: Option<&Path>, cwd: &Path) -> ProbeMemoryStack {
    let mut stack = ProbeMemoryStack::default();

    if let Some(probe_home) = probe_home {
        let user_path = probe_home
            .join(USER_MEMORY_RELATIVE_PATH[0])
            .join(USER_MEMORY_RELATIVE_PATH[1]);
        stack.suggested_user_path = Some(user_path.clone());
        if user_path.exists() {
            push_memory_layer(
                &mut stack,
                MemoryLayerKind::User,
                String::from("user"),
                String::from(USER_MEMORY_RELATIVE_PATH[1]),
                user_path,
            );
        }
    }

    let repo_root = resolve_git_repo_root(cwd);
    if let Some(repo_root) = repo_root.as_ref() {
        let repo_probe_path = repo_root.join(REPO_MEMORY_FILE);
        let repo_agents_path = repo_root.join(LEGACY_REPO_MEMORY_FILE);
        stack.suggested_repo_path = Some(repo_probe_path.clone());
        if repo_probe_path.exists() {
            push_memory_layer(
                &mut stack,
                MemoryLayerKind::Repo,
                String::from("repo"),
                String::from(REPO_MEMORY_FILE),
                repo_probe_path.clone(),
            );
        } else if repo_agents_path.exists() {
            push_memory_layer(
                &mut stack,
                MemoryLayerKind::Repo,
                String::from("repo"),
                format!("{LEGACY_REPO_MEMORY_FILE} fallback"),
                repo_agents_path,
            );
        }

        let mut directory_layers = repo_relative_ancestors(repo_root.as_path(), cwd);
        directory_layers.retain(|directory| directory != repo_root);
        if let Some(current_directory_path) = cwd.canonicalize().ok().and_then(|resolved| {
            resolved
                .starts_with(repo_root)
                .then_some(resolved.join(REPO_MEMORY_FILE))
        }) {
            if !current_directory_path.starts_with(repo_root) {
                stack.suggested_directory_path = None;
            } else if current_directory_path != repo_probe_path {
                stack.suggested_directory_path = Some(current_directory_path);
            }
        }

        for directory in directory_layers {
            let path = directory.join(REPO_MEMORY_FILE);
            if path.exists() {
                let label = directory
                    .strip_prefix(repo_root)
                    .ok()
                    .map(|relative| relative.display().to_string())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| String::from("."));
                push_memory_layer(
                    &mut stack,
                    MemoryLayerKind::Directory,
                    format!("dir:{label}"),
                    String::from(REPO_MEMORY_FILE),
                    path,
                );
            }
        }
    }

    stack
}

fn push_memory_layer(
    stack: &mut ProbeMemoryStack,
    kind: MemoryLayerKind,
    label: String,
    source_label: String,
    path: PathBuf,
) {
    match read_memory_file(path.as_path()) {
        Ok(Some((body, truncated))) => stack.layers.push(MemoryLayer {
            kind,
            label,
            path,
            source_label,
            body,
            truncated,
        }),
        Ok(None) => stack
            .issues
            .push(format!("ignored empty memory file: {}", path.display())),
        Err(error) => stack.issues.push(format!(
            "failed to read memory file {}: {error}",
            path.display()
        )),
    }
}

fn read_memory_file(path: &Path) -> Result<Option<(String, bool)>, String> {
    let raw = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let truncated = trimmed.chars().count() > MAX_MEMORY_CHARS;
    let body = if truncated {
        let mut value = trimmed.chars().take(MAX_MEMORY_CHARS).collect::<String>();
        value.push_str("\n\n[truncated by Probe for prompt safety]");
        value
    } else {
        trimmed.to_string()
    };
    Ok(Some((body, truncated)))
}

fn resolve_git_repo_root(path: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!root.is_empty()).then(|| PathBuf::from(root))
}

fn repo_relative_ancestors(repo_root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let Ok(mut current) = cwd.canonicalize() else {
        return Vec::new();
    };
    let Ok(repo_root) = repo_root.canonicalize() else {
        return Vec::new();
    };
    if !current.starts_with(&repo_root) {
        return Vec::new();
    }

    let mut layers = Vec::new();
    loop {
        layers.push(current.clone());
        if current == repo_root {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent.to_path_buf();
    }
    layers.reverse();
    layers
}

#[cfg(test)]
mod tests {
    use super::{
        LEGACY_REPO_MEMORY_FILE, REPO_MEMORY_FILE, USER_MEMORY_RELATIVE_PATH,
        load_probe_memory_stack,
    };
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn memory_stack_loads_user_repo_and_directory_layers() {
        let temp = tempdir().expect("tempdir");
        let probe_home = temp.path().join("probe-home");
        let repo = temp.path().join("repo");
        let nested = repo.join("src/features");
        fs::create_dir_all(probe_home.join(USER_MEMORY_RELATIVE_PATH[0])).expect("probe home");
        fs::create_dir_all(&nested).expect("nested dir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(&repo)
            .output()
            .expect("git init");

        fs::write(
            probe_home
                .join(USER_MEMORY_RELATIVE_PATH[0])
                .join(USER_MEMORY_RELATIVE_PATH[1]),
            "Prefer concise handoff text.",
        )
        .expect("write user memory");
        fs::write(repo.join(LEGACY_REPO_MEMORY_FILE), "Repo fallback rules.")
            .expect("write agents");
        fs::write(nested.join(REPO_MEMORY_FILE), "Nested directory rules.").expect("write nested");

        let stack = load_probe_memory_stack(Some(probe_home.as_path()), nested.as_path());

        assert_eq!(stack.layers.len(), 3);
        assert_eq!(stack.active_label(), "user + repo/AGENTS + 1 dir");
        let prompt = stack.prompt_addendum().expect("prompt addendum");
        assert!(prompt.contains("Prefer concise handoff text."));
        assert!(prompt.contains("Repo fallback rules."));
        assert!(prompt.contains("Nested directory rules."));
    }
}
