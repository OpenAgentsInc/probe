//! Fixture tests for the native tool set (spec Lesson 3: no tool merges
//! without them). Each test runs against its own temp workspace.

use probe_bin::tools::{execute, tool_definitions, tool_kinds};
use probe_core::contract::message::ToolResultValue;
use probe_core::permission::ToolKind;
use probe_core::redact::SecretSet;

struct Workspace(std::path::PathBuf);

impl Workspace {
    fn new(name: &str) -> Workspace {
        let path = std::env::temp_dir().join(format!(
            "probe-tools-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Workspace(path)
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.0.join(relative)).unwrap()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn no_secrets() -> SecretSet {
    SecretSet::new()
}

fn text_of(result: &ToolResultValue) -> &str {
    match result {
        ToolResultValue::Text { value } | ToolResultValue::Error { value } => value,
        ToolResultValue::Json { .. } => panic!("expected text"),
    }
}

fn is_error(result: &ToolResultValue) -> bool {
    matches!(result, ToolResultValue::Error { .. })
}

#[test]
fn catalog_covers_every_tool_with_a_kind() {
    let definitions = tool_definitions();
    let kinds = tool_kinds();
    assert!(!definitions.is_empty());
    for definition in &definitions {
        assert!(kinds.contains_key(&definition.name), "{} has no ACP kind", definition.name);
    }
    assert_eq!(kinds["shell"], ToolKind::Execute);
    assert_eq!(kinds["edit_file"], ToolKind::Edit);
    assert_eq!(kinds["read_file"], ToolKind::Read);
}

#[test]
fn shell_runs_captures_and_reports_exit_failures() {
    let workspace = Workspace::new("shell");
    let result = execute("shell", &serde_json::json!({ "command": "echo hello" }), &workspace.0, &no_secrets());
    assert!(!is_error(&result));
    assert!(text_of(&result).contains("hello"));

    let result = execute("shell", &serde_json::json!({ "command": "echo oops >&2; exit 3" }), &workspace.0, &no_secrets());
    assert!(is_error(&result));
    let text = text_of(&result);
    assert!(text.contains("exit") && text.contains("oops"), "{text}");
}

#[test]
fn shell_times_out_and_kills_the_process() {
    let workspace = Workspace::new("shell-timeout");
    let started = std::time::Instant::now();
    let result = execute(
        "shell",
        &serde_json::json!({ "command": "sleep 30", "timeoutMs": 300 }),
        &workspace.0,
        &no_secrets(),
    );
    assert!(is_error(&result));
    assert!(text_of(&result).contains("timed out"));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn shell_children_never_see_the_grant() {
    let workspace = Workspace::new("shell-grant");
    // The scrubber is the second wall; this asserts the first: the variable
    // is removed from the child environment entirely.
    std::env::set_var("PROBE_INFERENCE_GRANT", "grant_supersecret_value");
    let result = execute(
        "shell",
        &serde_json::json!({ "command": "printenv PROBE_INFERENCE_GRANT || echo ABSENT" }),
        &workspace.0,
        &no_secrets(),
    );
    std::env::remove_var("PROBE_INFERENCE_GRANT");
    assert!(text_of(&result).contains("ABSENT"), "{}", text_of(&result));
}

#[test]
fn read_list_and_grep_are_workspace_confined() {
    let workspace = Workspace::new("read");
    workspace.write("src/lib.rs", "fn main() {}\nlet needle = 42;\n");
    workspace.write("notes.txt", "the needle is here\n");

    let result = execute("read_file", &serde_json::json!({ "path": "src/lib.rs" }), &workspace.0, &no_secrets());
    assert!(text_of(&result).contains("needle = 42"));

    let result = execute("list_files", &serde_json::json!({}), &workspace.0, &no_secrets());
    let text = text_of(&result);
    assert!(text.contains("src/") && text.contains("notes.txt"));

    let result = execute("grep_files", &serde_json::json!({ "pattern": "needle" }), &workspace.0, &no_secrets());
    let text = text_of(&result);
    assert!(text.contains("src/lib.rs:2:") && text.contains("notes.txt:1:"), "{text}");

    for escape in ["../outside.txt", "/etc/passwd"] {
        let result = execute("read_file", &serde_json::json!({ "path": escape }), &workspace.0, &no_secrets());
        assert!(is_error(&result), "{escape} should be refused");
    }
}

#[test]
fn write_preserves_bom_and_refuses_git_mutation() {
    let workspace = Workspace::new("write");
    workspace.write("bom.txt", "\u{feff}old\n");
    let result = execute(
        "write_file",
        &serde_json::json!({ "path": "bom.txt", "content": "new\n" }),
        &workspace.0,
        &no_secrets(),
    );
    assert!(!is_error(&result));
    assert_eq!(workspace.read("bom.txt"), "\u{feff}new\n");

    let result = execute(
        "write_file",
        &serde_json::json!({ "path": ".git/config", "content": "x" }),
        &workspace.0,
        &no_secrets(),
    );
    assert!(is_error(&result));
    assert!(text_of(&result).contains(".git"));
}

#[test]
fn edit_applies_the_exact_match_policy_with_crlf_preserved() {
    let workspace = Workspace::new("edit");
    workspace.write("code.txt", "alpha\r\nbeta\r\n");
    let result = execute(
        "edit_file",
        &serde_json::json!({ "path": "code.txt", "oldString": "beta", "newString": "gamma" }),
        &workspace.0,
        &no_secrets(),
    );
    assert!(!is_error(&result), "{}", text_of(&result));
    assert_eq!(workspace.read("code.txt"), "alpha\r\ngamma\r\n");

    workspace.write("multi.txt", "x x\n");
    let result = execute(
        "edit_file",
        &serde_json::json!({ "path": "multi.txt", "oldString": "x", "newString": "y" }),
        &workspace.0,
        &no_secrets(),
    );
    assert!(is_error(&result));
    assert!(text_of(&result).contains("multiple exact matches"));

    let result = execute(
        "edit_file",
        &serde_json::json!({ "path": "multi.txt", "oldString": "zzz", "newString": "y" }),
        &workspace.0,
        &no_secrets(),
    );
    assert!(is_error(&result));
    assert!(text_of(&result).contains("Could not find oldString"));
}

#[test]
fn tool_output_is_scrubbed_of_registered_secrets() {
    let workspace = Workspace::new("scrub");
    workspace.write("leak.txt", "token grant_abcdef123456 lives here\n");
    let mut secrets = SecretSet::new();
    secrets.register("grant_abcdef123456");
    let result = execute("read_file", &serde_json::json!({ "path": "leak.txt" }), &workspace.0, &secrets);
    let text = text_of(&result);
    assert!(!text.contains("grant_abcdef123456"));
    assert!(text.contains("[redacted]"));
}
