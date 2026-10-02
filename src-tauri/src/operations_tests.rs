use super::*;
use crate::store::McpRecord;
use serde_json::json;
use std::{fs, path::{Path, PathBuf}, process::Command};

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

/// 将三个 Agent 的配置目录指向临时目录，避免测试触碰真实用户配置。
/// 持有进程级互斥锁，防止并行测试互相篡改环境变量。
struct McpEnv {
    // 声明顺序即 drop 顺序：先恢复环境变量，再释放互斥锁。
    saved: Vec<(String, Option<std::ffi::OsString>)>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl McpEnv {
    fn new(home: &Path) -> Self {
        let lock = crate::test_support::env_lock();
        let mut saved = Vec::new();
        // OMP 把 PI_CONFIG_DIR 视为相对 home 的路径（Node path.join 语义，
        // 前导 / 会被剥掉），所以同时伪造 HOME 并用相对值。
        let settings: [(&str, std::ffi::OsString); 5] = [
            ("HOME", home.as_os_str().to_owned()),
            ("USERPROFILE", home.as_os_str().to_owned()),
            ("CLAUDE_CONFIG_DIR", home.join("claude").into_os_string()),
            ("CODEX_HOME", home.join("codex").into_os_string()),
            ("PI_CONFIG_DIR", std::ffi::OsString::from("omp")),
        ];
        for (name, value) in settings {
            saved.push((name.into(), std::env::var_os(name)));
            std::env::set_var(name, &value);
        }
        for name in ["PI_CODING_AGENT_DIR", "PI_PROFILE", "OMP_PROFILE"] {
            saved.push((name.into(), std::env::var_os(name)));
            std::env::remove_var(name);
        }
        Self { saved, _lock: lock }
    }
}

impl Drop for McpEnv {
    fn drop(&mut self) {
        for (name, value) in std::mem::take(&mut self.saved) {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

fn seed_agent_configs(home: &Path) {
    let claude = home.join("claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(
        claude.join(".claude.json"),
        r#"{"foo":1,"mcpServers":{"old":{"command":"keep"}}}"#,
    )
    .unwrap();
    let codex = home.join("codex");
    fs::create_dir_all(&codex).unwrap();
    fs::write(
        codex.join("config.toml"),
        "model = \"test\"\n\n[mcp_servers.legacy]\ncommand = \"legacy\"\n",
    )
    .unwrap();
    let omp = home.join("omp");
    fs::create_dir_all(omp.join("agent")).unwrap();
    fs::write(
        omp.join("agent/mcp.json"),
        r#"{"mcpServers":{"omp-only":{"command":"omp-cmd"}}}"#,
    )
    .unwrap();
}

#[test]
fn state_uses_canonical_workspace_and_only_lists_amc_managed_mcp() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let supplied = PathBuf::from(&workspace.path);
    let canonical = fs::canonicalize(&supplied).unwrap();
    let _env = McpEnv::new(&fixture.root);
    seed_agent_configs(&fixture.root);
    fs::create_dir_all(canonical.join(".claude/skills/example")).unwrap();
    fs::write(
        canonical.join(".claude/skills/example/SKILL.md"),
        "---\nname: example\ndescription: Read-only\n---\n",
    )
    .unwrap();
    let mut core = fixture.core();
    // AMC 只管理通过它保存的服务；Agent 现有配置不进入列表。
    let state = core.state(workspace.clone()).unwrap();
    assert!(state.mcp.is_empty());
    // 保存后出现在列表中。
    let plan = core
        .plan_mcp(
            "demo".into(),
            Some(json!({"type":"stdio","command":"true"})),
            vec![mcp::Agent::Claude],
        )
        .unwrap();
    core.apply(plan.id).unwrap();
    let state = core.state(workspace).unwrap();
    assert_eq!(state.workspace.path, canonical.to_string_lossy());
    let serialized = serde_json::to_value(&state).unwrap();
    assert_eq!(
        serialized["workspace"]["path"],
        canonical.to_string_lossy().as_ref()
    );
    let names: Vec<&str> = state.mcp.iter().map(|server| server.name.as_str()).collect();
    assert_eq!(names, vec!["demo"]);
    assert_eq!(state.skills[0].source, "Claude · detected");
    assert!(!state.skills[0].managed);
}

#[test]
fn mcp_save_projects_to_agent_configs_with_stale_check_and_secret_restore() {
    let fixture = Fixture::new();
    let home = fixture.root.clone();
    let _env = McpEnv::new(&home);
    seed_agent_configs(&home);
    let mut core = fixture.core();
    let spec = json!({
        "type": "http",
        "url": "https://example.com/mcp",
        "headers": {"Authorization": "Bearer private-token"},
        "auth": {"type": "oauth", "clientId": "public-id", "clientSecret": "private-client"}
    });
    let plan = core
        .plan_mcp(
            "demo".into(),
            Some(spec),
            vec![mcp::Agent::Omp, mcp::Agent::Claude, mcp::Agent::Codex],
        )
        .unwrap();
    // 预览不泄露密钥；OMP JSON 保留无关键并写入 $schema。
    assert!(!plan.changes.iter().any(|change| change.after.contains("private-token")));
    let omp_change = plan.changes.iter().find(|c| c.path.ends_with("mcp.json")).unwrap();
    assert!(omp_change.after.contains("$schema"));
    let codex_change = plan.changes.iter().find(|c| c.path.ends_with("config.toml")).unwrap();
    assert!(codex_change.after.contains("model = \"test\""));
    core.apply(plan.id).unwrap();

    // Claude/Codex/OMP 文件都被投影；Codex 保留注释与其它表。
    let claude: Value = serde_json::from_slice(
        &fs::read(home.join("claude/.claude.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(claude["foo"], 1);
    assert_eq!(claude["mcpServers"]["demo"]["headers"]["Authorization"], "Bearer private-token");
    assert_eq!(claude["mcpServers"]["old"]["command"], "keep");
    let codex = fs::read_to_string(home.join("codex/config.toml")).unwrap();
    assert!(codex.contains("model = \"test\""));
    assert!(codex.contains("[mcp_servers.legacy]"));
    assert!(codex.contains("[mcp_servers.demo]"));
    assert!(codex.contains("url = \"https://example.com/mcp\""));
    let omp: Value = serde_json::from_slice(&fs::read(home.join("omp/agent/mcp.json")).unwrap())
        .unwrap();
    assert_eq!(omp["mcpServers"]["demo"]["url"], "https://example.com/mcp");
    let record = core.store.mcp_server("demo").unwrap().unwrap();
    assert!(record.omp && record.claude && record.codex);

    // 预览后文件被外部修改 → apply 拒绝写入。
    let stale = core
        .plan_mcp("demo".into(), Some(json!({"type":"stdio","command":"true"})), vec![mcp::Agent::Claude])
        .unwrap();
    fs::write(home.join("claude/.claude.json"), r#"{"mcpServers":{}}"#).unwrap();
    assert!(core.apply(stale.id).is_err());

    // 用脱敏后的 spec 重新保存 → 密钥从文件现值回填。
    let redacted = json!({
        "type": "http",
        "url": "https://example.com/mcp",
        "headers": {"Authorization": platform::HIDDEN_MCP_VALUE},
        "auth": {"type": "oauth", "clientId": "public-id", "clientSecret": platform::HIDDEN_MCP_VALUE}
    });
    let plan = core
        .plan_mcp("demo".into(), Some(redacted), vec![mcp::Agent::Claude, mcp::Agent::Codex])
        .unwrap();
    core.apply(plan.id).unwrap();
    let claude: Value = serde_json::from_slice(
        &fs::read(home.join("claude/.claude.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(claude["mcpServers"]["demo"]["headers"]["Authorization"], "Bearer private-token");
    assert_eq!(claude["mcpServers"]["demo"]["auth"]["clientSecret"], "private-client");
    let codex = fs::read_to_string(home.join("codex/config.toml")).unwrap();
    assert!(codex.contains("[mcp_servers.demo]"));
    assert!(!codex.contains("[mcp_servers.demo.headers]"));
    // spec 中不允许保留 enabled 开关键（开关归数据库列）。
    let stored = core.store.mcp_server("demo").unwrap().unwrap();
    assert!(stored.spec.get("enabled").is_none());

    // state 视图对密钥脱敏。
    let state = core.state(fixture.workspace()).unwrap();
    let view = state.mcp.iter().find(|s| s.name == "demo").unwrap();
    assert_eq!(view.config["headers"]["Authorization"], platform::HIDDEN_MCP_VALUE);
    assert_eq!(view.agents.len(), 2);

    // 删除：从所有已启用 Agent 的配置中移除，数据库记录消失。
    let plan = core.plan_mcp("demo".into(), None, vec![]).unwrap();
    core.apply(plan.id).unwrap();
    let claude: Value = serde_json::from_slice(
        &fs::read(home.join("claude/.claude.json")).unwrap(),
    )
    .unwrap();
    assert!(claude["mcpServers"].get("demo").is_none());
    let codex = fs::read_to_string(home.join("codex/config.toml")).unwrap();
    assert!(!codex.contains("[mcp_servers.demo]"));
    assert!(codex.contains("[mcp_servers.legacy]"));
    assert!(core.store.mcp_server("demo").unwrap().is_none());
}

/// 回归：脱敏编辑入库后，停用/再启用不得把 "[已隐藏]" 占位符写进
/// Agent 文件；数据库与文件必须始终保存真实凭据。
#[test]
fn mcp_redacted_save_keeps_real_secrets_in_db_and_files_after_toggle() {
    let fixture = Fixture::new();
    let home = fixture.root.clone();
    let _env = McpEnv::new(&home);
    seed_agent_configs(&home);
    let mut core = fixture.core();
    // 1) 首次保存（真实密钥）到 Claude + Codex。
    let plan = core
        .plan_mcp(
            "demo".into(),
            Some(json!({
                "type": "http",
                "url": "https://example.com/mcp",
                "headers": {"Authorization": "Bearer private-token"}
            })),
            vec![mcp::Agent::Claude, mcp::Agent::Codex],
        )
        .unwrap();
    core.apply(plan.id).unwrap();
    // 2) UI 回显脱敏 spec（占位符）后再保存：DB 必须仍存真实值。
    let redacted = json!({
        "type": "http",
        "url": "https://example.com/mcp",
        "headers": {"Authorization": platform::HIDDEN_MCP_VALUE}
    });
    let plan = core
        .plan_mcp("demo".into(), Some(redacted), vec![mcp::Agent::Claude, mcp::Agent::Codex])
        .unwrap();
    core.apply(plan.id).unwrap();
    let stored = core.store.mcp_server("demo").unwrap().unwrap();
    assert_eq!(
        stored.spec["headers"]["Authorization"],
        "Bearer private-token",
        "占位符不得进入数据库"
    );
    let claude: Value = serde_json::from_slice(
        &fs::read(home.join("claude/.claude.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        claude["mcpServers"]["demo"]["headers"]["Authorization"],
        "Bearer private-token"
    );
    // 3) 停用 Codex → 再启用：投影用的 DB spec 是真值，文件不得出现占位符。
    let plan = core.plan_mcp_toggle("demo".into(), mcp::Agent::Codex, false).unwrap();
    core.apply(plan.id).unwrap();
    let plan = core.plan_mcp_toggle("demo".into(), mcp::Agent::Codex, true).unwrap();
    core.apply(plan.id).unwrap();
    let codex = fs::read_to_string(home.join("codex/config.toml")).unwrap();
    assert!(codex.contains("http_headers"));
    assert!(codex.contains("\"Bearer private-token\""));
    assert!(!codex.contains(platform::HIDDEN_MCP_VALUE));
    let stored = core.store.mcp_server("demo").unwrap().unwrap();
    assert_eq!(stored.spec["headers"]["Authorization"], "Bearer private-token");
}

/// 回归：未勾选 OMP（记录仅存 DB）时，OMP 的坏 JSON 不得阻断 Codex 的
/// 保存——回填只读统一库，不扫描 Agent 文件。
#[test]
fn mcp_save_is_not_blocked_by_broken_unselected_agent_file() {
    let fixture = Fixture::new();
    let home = fixture.root.clone();
    let _env = McpEnv::new(&home);
    seed_agent_configs(&home);
    fs::write(home.join("omp/agent/mcp.json"), "{ broken json").unwrap();
    let mut core = fixture.core();
    let mut seeded = McpRecord::new(
        "demo".into(),
        json!({
            "type": "http",
            "url": "https://example.com/mcp",
            "headers": {"Authorization": "Bearer db-token"}
        }),
    );
    seeded.codex = true;
    core.store.save_mcp_server(&seeded).unwrap();

    let plan = core
        .plan_mcp(
            "demo".into(),
            Some(json!({
                "type": "http",
                "url": "https://example.com/mcp",
                "headers": {"Authorization": platform::HIDDEN_MCP_VALUE}
            })),
            vec![mcp::Agent::Codex],
        )
        .unwrap();
    core.apply(plan.id).unwrap();
    let stored = core.store.mcp_server("demo").unwrap().unwrap();
    assert_eq!(stored.spec["headers"]["Authorization"], "Bearer db-token");
    let codex = fs::read_to_string(home.join("codex/config.toml")).unwrap();
    assert!(codex.contains("\"Bearer db-token\""));
    assert!(!codex.contains(platform::HIDDEN_MCP_VALUE));
}

/// 回归：Codex 预览必须脱敏**整份** config.toml——model_providers 的
/// experimental_bearer_token / http_headers 等非 MCP 段落同样可能携带
/// 凭据；且预览只是展示层，不得改动磁盘原值。
#[test]
fn codex_preview_redacts_secrets_across_whole_document() {
    let raw = r#"
model = "gpt-test"

[model_providers.custom]
name = "Custom"
base_url = "https://relay.example.com/v1"
experimental_bearer_token = "sk-provider-secret"
http_headers = { Authorization = "Bearer hdr-secret" }

[mcp_servers.demo]
command = "true"
"#;
    let preview = mcp::codex::preview_redacted(raw.as_bytes());
    assert!(!preview.contains("sk-provider-secret"), "{preview}");
    assert!(!preview.contains("Bearer hdr-secret"), "{preview}");
    assert!(preview.contains(platform::HIDDEN_MCP_VALUE));
    // 非敏感结构保持可读，便于审阅变更。
    assert!(preview.contains("model = \"gpt-test\""));
    assert!(preview.contains("base_url = \"https://relay.example.com/v1\""));
    assert!(preview.contains("[mcp_servers.demo]"));
    // 预览是纯函数：原始字节未被改动（磁盘值不受影响）。
    assert!(raw.contains("sk-provider-secret"));
}

/// 回归：完全内联的 provider（顶层键值为内联表嵌套内联表）与数组内嵌
/// 的 header 表同样必须脱敏——容器键在 Value 入口与 Item 入口语义一致。
#[test]
fn codex_preview_redacts_inline_and_array_nested_secrets() {
    let raw = r#"
model_providers = { custom = { base_url = "https://x/v1", experimental_bearer_token = "inline-token-secret", http_headers = { Authorization = "Bearer inline-hdr" } } }

[[mcp_servers.array_demo]]
command = "true"
http_headers = { X-Api-Key = "array-header-secret" }
"#;
    let preview = mcp::codex::preview_redacted(raw.as_bytes());
    assert!(!preview.contains("inline-token-secret"), "{preview}");
    assert!(!preview.contains("Bearer inline-hdr"), "{preview}");
    assert!(!preview.contains("array-header-secret"), "{preview}");
    assert!(preview.contains(platform::HIDDEN_MCP_VALUE));
    assert!(preview.contains("base_url = \"https://x/v1\""));
    assert!(preview.contains("[[mcp_servers.array_demo]]") || preview.contains("array_demo"));
}

/// 回归：历史遗留/无法证明来源的库记录（managed=0，如旧版自动导入的
/// computer-use）按非 AMC 管理处理——不列出、不可切换、不可删除、
/// 不被保存接管，且用户 Agent 文件原样保留。
#[test]
fn unmanaged_records_are_invisible_and_protected() {
    let fixture = Fixture::new();
    let home = fixture.root.clone();
    let _env = McpEnv::new(&home);
    seed_agent_configs(&home);
    let mut core = fixture.core();
    // 模拟旧版自动导入留下的行：无 ownership 标记，但 Agent 文件里
    // 已有对应投影。
    let mut legacy = McpRecord::new(
        "computer-use".into(),
        json!({"type": "stdio", "command": "true"}),
    );
    legacy.managed = false;
    legacy.codex = true;
    core.store.save_mcp_server(&legacy).unwrap();
    let codex_path = home.join("codex/config.toml");
    fs::write(
        &codex_path,
        "[mcp_servers.computer-use]\ncommand = \"true\"\n\n[mcp_servers.legacy]\ncommand = \"legacy\"\n",
    )
    .unwrap();

    // 不列出。
    let state = core.state(fixture.workspace()).unwrap();
    assert!(
        state.mcp.iter().all(|server| server.name != "computer-use"),
        "未管理记录不得出现在列表中"
    );
    // 不可切换。
    let error = match core.plan_mcp_toggle("computer-use".into(), mcp::Agent::Codex, false) {
        Err(error) => error,
        Ok(_) => panic!("未管理记录不得切换"),
    };
    assert!(error.contains("非 AMC 管理"), "{error}");
    // 不可删除，且 Agent 文件原样保留。
    let error = match core.plan_mcp("computer-use".into(), None, vec![]) {
        Err(error) => error,
        Ok(_) => panic!("未管理记录不得删除"),
    };
    assert!(error.contains("非 AMC 管理"), "{error}");
    let codex = fs::read_to_string(&codex_path).unwrap();
    assert!(codex.contains("[mcp_servers.computer-use]"));
    assert!(core.store.mcp_server("computer-use").unwrap().is_some());
    // 保存同名 = 显式接管：记录转为 AMC 管理，文件投影以新 spec 为准。
    let plan = core
        .plan_mcp(
            "computer-use".into(),
            Some(json!({"type": "stdio", "command": "other"})),
            vec![mcp::Agent::Codex],
        )
        .unwrap();
    core.apply(plan.id).unwrap();
    let row = core.store.mcp_server("computer-use").unwrap().unwrap();
    assert!(row.managed);
    let state = core.state(fixture.workspace()).unwrap();
    let names: Vec<&str> = state.mcp.iter().map(|server| server.name.as_str()).collect();
    assert!(names.contains(&"computer-use"));
    // 接管后切换/删除恢复可用。
    let plan = core
        .plan_mcp_toggle("computer-use".into(), mcp::Agent::Codex, false)
        .unwrap();
    core.apply(plan.id).unwrap();
    let codex = fs::read_to_string(&codex_path).unwrap();
    assert!(!codex.contains("[mcp_servers.computer-use]"));
    // 未管理的其它行（legacy）依旧不动。
    assert!(codex.contains("[mcp_servers.legacy]"));
    assert!(!legacy.managed);
}

/// 回归：旧版 schema（无 managed 列）打开时自动迁移——既有行保留、
/// 标记为未管理，不被清库。
#[test]
fn legacy_schema_migrates_and_marks_rows_unmanaged() {
    let fixture = Fixture::new();
    let data = fixture.root.join("data");
    fs::create_dir_all(&data).unwrap();
    let legacy = rusqlite::Connection::open(data.join("amc.sqlite3")).unwrap();
    legacy
        .execute_batch(
            "CREATE TABLE mcp_servers (
                name TEXT PRIMARY KEY, spec TEXT NOT NULL,
                omp INTEGER NOT NULL DEFAULT 0,
                claude INTEGER NOT NULL DEFAULT 0,
                codex INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO mcp_servers(name,spec,codex) VALUES ('computer-use','{\"type\":\"stdio\",\"command\":\"true\"}',1);",
        )
        .unwrap();
    drop(legacy);

    let core = fixture.core();
    // 行保留且未管理：不列出，但仍可查到。
    let state = core.state(fixture.workspace()).unwrap();
    assert!(state.mcp.is_empty());
    let row = core.store.mcp_server("computer-use").unwrap().unwrap();
    assert!(!row.managed && row.codex);
}

/// 回归：从其它工具文档复制的 mcpServers 包装块应得到明确报错，
/// 而不是误导性的「stdio MCP 需要 command」。
#[test]
fn mcp_save_rejects_mcp_servers_envelope_with_clear_message() {
    let fixture = Fixture::new();
    let _env = McpEnv::new(&fixture.root);
    let mut core = fixture.core();
    let spec = json!({
        "mcpServers": {
            "zai-mcp-server": {
                "type": "stdio",
                "command": "npx",
                "args": ["-y", "@z_ai/mcp-server"],
                "env": {"Z_AI_API_KEY": "key", "Z_AI_MODE": "ZHIPU"}
            }
        }
    });
    let error = match core.plan_mcp("zai".into(), Some(spec), vec![mcp::Agent::Claude]) {
        Err(error) => error,
        Ok(_) => panic!("mcpServers 包装应当被拒绝"),
    };
    assert!(error.contains("mcpServers 包装"), "{error}");
    // 解包后的单个服务配置则应通过校验。
    let inner = json!({
        "type": "stdio",
        "command": "npx",
        "args": ["-y", "@z_ai/mcp-server"],
        "env": {"Z_AI_API_KEY": "key", "Z_AI_MODE": "ZHIPU"}
    });
    core.plan_mcp("zai-mcp-server".into(), Some(inner), vec![mcp::Agent::Claude])
        .unwrap();
}

/// 回归：Agent 文件中的密钥被手改漂移时，脱敏保存回填的是 DB 真值，
/// 文件漂移值不得反向进入库，随后投影会用库值覆盖文件。
#[test]
fn mcp_masked_save_restores_from_db_not_drifted_file() {
    let fixture = Fixture::new();
    let home = fixture.root.clone();
    let _env = McpEnv::new(&home);
    seed_agent_configs(&home);
    let mut core = fixture.core();
    let mut seeded = McpRecord::new(
        "demo".into(),
        json!({
            "type": "http",
            "url": "https://example.com/mcp",
            "headers": {"Authorization": "Bearer db-token"}
        }),
    );
    seeded.claude = true;
    core.store.save_mcp_server(&seeded).unwrap();
    // 文件里的值与 DB 漂移。
    fs::write(
        home.join("claude/.claude.json"),
        r#"{"mcpServers":{"demo":{"type":"http","url":"https://example.com/mcp","headers":{"Authorization":"Bearer file-token"}}}}"#,
    )
    .unwrap();

    let plan = core
        .plan_mcp(
            "demo".into(),
            Some(json!({
                "type": "http",
                "url": "https://example.com/mcp",
                "headers": {"Authorization": platform::HIDDEN_MCP_VALUE}
            })),
            vec![mcp::Agent::Claude, mcp::Agent::Codex],
        )
        .unwrap();
    core.apply(plan.id).unwrap();
    let stored = core.store.mcp_server("demo").unwrap().unwrap();
    assert_eq!(
        stored.spec["headers"]["Authorization"],
        "Bearer db-token",
        "文件漂移值不得覆盖库值"
    );
    let claude: Value = serde_json::from_slice(
        &fs::read(home.join("claude/.claude.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        claude["mcpServers"]["demo"]["headers"]["Authorization"],
        "Bearer db-token",
        "投影应以库值为准覆盖漂移文件"
    );
}

#[test]
fn mcp_toggle_only_touches_target_agent_and_skips_uninstalled() {
    let fixture = Fixture::new();
    let home = fixture.root.clone();
    let _env = McpEnv::new(&home);
    seed_agent_configs(&home);
    let mut core = fixture.core();
    let plan = core
        .plan_mcp(
            "demo".into(),
            Some(json!({"type":"stdio","command":"true"})),
            vec![mcp::Agent::Omp],
        )
        .unwrap();
    core.apply(plan.id).unwrap();
    // Codex 目录存在但未勾选；切换 Codex 开关只写 config.toml。
    let plan = core.plan_mcp_toggle("demo".into(), mcp::Agent::Codex, true).unwrap();
    core.apply(plan.id).unwrap();
    let codex = fs::read_to_string(home.join("codex/config.toml")).unwrap();
    assert!(codex.contains("[mcp_servers.demo]"));
    let record = core.store.mcp_server("demo").unwrap().unwrap();
    assert!(!record.claude && record.codex);
    // 停用 Codex：单条目被移除，legacy 保留。
    let plan = core.plan_mcp_toggle("demo".into(), mcp::Agent::Codex, false).unwrap();
    core.apply(plan.id).unwrap();
    let codex = fs::read_to_string(home.join("codex/config.toml")).unwrap();
    assert!(!codex.contains("[mcp_servers.demo]"));
    assert!(codex.contains("[mcp_servers.legacy]"));
    // OMP 目录不存在（未安装）→ 保存记录开关但跳过写入并给出警告。
    let plan = core
        .plan_mcp(
            "fresh".into(),
            Some(json!({"type":"stdio","command":"true"})),
            vec![mcp::Agent::Codex],
        )
        .unwrap();
    core.apply(plan.id).unwrap();
    std::env::remove_var("CODEX_HOME");
    let result = core.plan_mcp_toggle("fresh".into(), mcp::Agent::Codex, true);
    std::env::set_var("CODEX_HOME", home.join("codex"));
    let plan = result.unwrap();
    assert!(plan.changes.is_empty());
    assert!(plan.warnings.iter().any(|w| w.contains("Codex")));
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
