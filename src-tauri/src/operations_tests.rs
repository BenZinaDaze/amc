use super::*;
use std::{fs, path::Path, process::Command};

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("amc-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self {
            root: fs::canonicalize(root).unwrap(),
        }
    }

    fn workspace(&self) -> Workspace {
        let path = self.root.join("project");
        fs::create_dir_all(&path).unwrap();
        Workspace {
            path: path.to_string_lossy().into_owned(),
        }
    }

    fn core(&self) -> Core {
        Core::new(self.root.join("data")).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn git(repository: &Path, args: &[&str]) {
    let result = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn check_all_updates_reports_empty_repository_set() {
    let fixture = Fixture::new();
    let mut core = fixture.core();
    let message = core.check_all_updates().unwrap();
    assert_eq!(message.message, "没有已添加的仓库");
}

#[test]
fn state_uses_canonical_workspace_and_generic_sources() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let supplied = PathBuf::from(&workspace.path);
    let canonical = fs::canonicalize(&supplied).unwrap();
    fs::write(
        canonical.join(".mcp.json"),
        r#"{"mcpServers":{"readonly":{"command":"true"}}}"#,
    )
    .unwrap();
    fs::create_dir_all(canonical.join(".claude/skills/example")).unwrap();
    fs::write(
        canonical.join(".claude/skills/example/SKILL.md"),
        "---\nname: example\ndescription: Read-only\n---\n",
    )
    .unwrap();
    let core = fixture.core();
    let state = core.state(workspace).unwrap();
    assert_eq!(state.workspace.path, canonical.to_string_lossy());
    let serialized = serde_json::to_value(&state).unwrap();
    assert_eq!(
        serialized["workspace"]["path"],
        canonical.to_string_lossy().as_ref()
    );
    assert_eq!(state.mcp[0].name, "readonly");
    assert!(!state.mcp[0].managed);
    assert_eq!(state.skills[0].source, "Claude · detected");
    assert!(!state.skills[0].managed);
}

#[test]
fn mcp_preserves_unrelated_keys_redacts_secrets_and_rejects_stale_preview() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let path = Path::new(&workspace.path).join("mcp.json");
    fs::write(&path, "{\"disabledServers\":[\"elsewhere\"],\"mcpServers\":{\"existing\":{\"command\":\"true\"}}}").unwrap();
    let mut core = fixture.core();
    let config = json!({"type":"http", "url":"https://example.com/mcp", "headers":{"Authorization":"Bearer private-token"}, "auth":{"type":"oauth", "clientId":"public-id", "clientSecret":"private-client", "credentialId":"private-id"}});
    let preview = core
        .plan_mcp(workspace.clone(), "new".into(), Some(config.clone()))
        .unwrap();
    assert!(!preview.changes[0].after.contains("private-token"));
    assert!(preview.changes[0].after.contains("disabledServers"));
    fs::write(&path, "{\"mcpServers\":{}}\n").unwrap();
    assert!(core.apply(preview.id).is_err());
    let preview = core
        .plan_mcp(workspace.clone(), "new".into(), Some(config))
        .unwrap();
    core.apply(preview.id).unwrap();
    let document: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        document["mcpServers"]["new"]["headers"]["Authorization"],
        "Bearer private-token"
    );
    assert_eq!(document["mcpServers"]["existing"], Value::Null);
    let state = core.state(workspace.clone()).unwrap();
    let state_json = serde_json::to_string(&state).unwrap();
    assert!(!state_json.contains("private-token"));
    assert!(!state_json.contains("private-client"));
    assert!(!state_json.contains("private-id"));
    assert!(state_json.contains("public-id"));
    let visible = state
        .mcp
        .iter()
        .find(|server| server.name == "new")
        .unwrap();
    assert_eq!(visible.config["auth"]["clientId"], "public-id");
    assert_eq!(
        visible.config["headers"]["Authorization"],
        omp::HIDDEN_MCP_VALUE
    );
    assert_eq!(
        visible.config["auth"]["clientSecret"],
        omp::HIDDEN_MCP_VALUE
    );
    assert_eq!(
        visible.config["auth"]["credentialId"],
        omp::HIDDEN_MCP_VALUE
    );
    let mut redacted = visible.config.clone();
    redacted["enabled"] = Value::Bool(false);
    let preview = core
        .plan_mcp(workspace.clone(), "new".into(), Some(redacted))
        .unwrap();
    assert!(!preview.changes[0].after.contains("private-token"));
    core.apply(preview.id).unwrap();
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        saved["mcpServers"]["new"]["headers"]["Authorization"],
        "Bearer private-token"
    );
    assert_eq!(saved["mcpServers"]["new"]["enabled"], false);
    assert_eq!(
        saved["mcpServers"]["new"]["auth"]["clientSecret"],
        "private-client"
    );
    assert_eq!(
        saved["mcpServers"]["new"]["auth"]["credentialId"],
        "private-id"
    );
    assert_eq!(saved["mcpServers"]["new"]["auth"]["clientId"], "public-id");
}

#[test]
fn skill_sync_blocks_local_edits_and_rollback_restores_previous_version() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let repo = fixture.root.join("source");
    fs::create_dir_all(repo.join("skills/example")).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.name", "AMC test"]);
    git(&repo, &["config", "user.email", "amc@example.invalid"]);
    let source = repo.join("skills/example/SKILL.md");
    fs::write(
        &source,
        "---\nname: example\ndescription: First version\n---\nOriginal\n",
    )
    .unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "first"]);

    let mut core = fixture.core();
    let registered = core
        .add_repository(repo.to_string_lossy().into_owned(), "main".into())
        .unwrap();
    let plan = core
        .plan_skill(workspace.clone(), registered.id, "skills/example".into())
        .unwrap();
    core.apply(plan.id).unwrap();
    let installed = Path::new(&workspace.path).join(".agents/skills/example/SKILL.md");
    assert!(fs::read_to_string(&installed).unwrap().contains("Original"));
    let record = core.store.records().unwrap().remove(0);

    fs::write(&installed, "Local edit\n").unwrap();
    assert!(core.plan_sync_record(record.clone()).is_err());
    fs::write(
        &installed,
        "---\nname: example\ndescription: First version\n---\nOriginal\n",
    )
    .unwrap();
    fs::write(
        &source,
        "---\nname: example\ndescription: Second version\n---\nUpdated\n",
    )
    .unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "second"]);
    let message = core.check_all_updates().unwrap();
    assert!(message.message.contains("1 个仓库"));
    assert!(core.store.installations().unwrap()[0].update_available);
    let plan = core.plan_sync_record(record).unwrap();
    core.apply(plan.id).unwrap();
    assert!(fs::read_to_string(&installed).unwrap().contains("Updated"));
    let plan = core
        .rollback_skill(core.store.records().unwrap()[0].id)
        .unwrap();
    core.apply(plan.id).unwrap();
    assert!(fs::read_to_string(&installed).unwrap().contains("Original"));
}
