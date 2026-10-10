use super::*;
use std::{env, ffi::OsString, fs, path::Path};

/// 环境变量守护：持全局锁并在 drop 时恢复原值，避免并行测试互相污染。
struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    fn new(names: &[&'static str]) -> Self {
        Self {
            _lock: crate::test_support::env_lock(),
            saved: names
                .iter()
                .map(|name| (*name, env::var_os(name)))
                .collect(),
        }
    }

    fn clear(&self, names: &[&str]) {
        for name in names {
            env::remove_var(name);
        }
    }

    fn set(&self, name: &str, value: &Path) {
        env::set_var(name, value);
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in &self.saved {
            match value {
                Some(value) => env::set_var(name, value),
                None => env::remove_var(name),
            }
        }
    }
}

/// 影响路径解析的全部环境变量。
const PATH_ENV: &[&str] = &[
    "CODEX_HOME",
    "CLAUDE_CONFIG_DIR",
    "PI_CONFIG_DIR",
    "PI_CODING_AGENT_DIR",
    "PI_PROFILE",
    "XDG_DATA_HOME",
];

fn entry<'a>(group: &'a AgentConfigPaths, purpose: &str) -> &'a ConfigPathEntry {
    group
        .entries
        .iter()
        .find(|entry| entry.purpose == purpose)
        .unwrap_or_else(|| panic!("缺少条目: {purpose}"))
}

#[test]
fn default_locations_follow_home() {
    let guard = EnvGuard::new(PATH_ENV);
    guard.clear(PATH_ENV);

    let groups = agent_config_paths().unwrap();
    assert_eq!(
        groups.iter().map(|group| group.agent).collect::<Vec<_>>(),
        vec![Agent::Omp, Agent::Claude, Agent::Codex]
    );

    let home = crate::platform::home().unwrap();
    let claude = groups
        .iter()
        .find(|group| group.agent == Agent::Claude)
        .unwrap();
    assert_eq!(claude.entries.len(), 3);
    assert_eq!(
        entry(claude, "配置文件").path,
        home.join(".claude.json").display().to_string()
    );
    assert_eq!(
        entry(claude, "配置目录").path,
        home.join(".claude").display().to_string()
    );
    assert_eq!(
        entry(claude, "Skills 目录").path,
        home.join(".claude/skills").display().to_string()
    );

    let codex = groups
        .iter()
        .find(|group| group.agent == Agent::Codex)
        .unwrap();
    assert_eq!(
        entry(codex, "配置文件").path,
        home.join(".codex/config.toml").display().to_string()
    );
    assert_eq!(
        entry(codex, "Skills 目录").path,
        home.join(".agents/skills").display().to_string()
    );
}

#[test]
fn env_overrides_redirect_locations() {
    let guard = EnvGuard::new(PATH_ENV);
    guard.clear(PATH_ENV);
    let sandbox = env::temp_dir().join(format!("amc-settings-env-{}", std::process::id()));
    let codex_home = sandbox.join("codex");
    let claude_dir = sandbox.join("claude");
    guard.set("CODEX_HOME", &codex_home);
    guard.set("CLAUDE_CONFIG_DIR", &claude_dir);

    let groups = agent_config_paths().unwrap();
    let codex = groups
        .iter()
        .find(|group| group.agent == Agent::Codex)
        .unwrap();
    assert_eq!(
        entry(codex, "配置文件").path,
        codex_home.join("config.toml").display().to_string()
    );
    assert_eq!(
        entry(codex, "配置目录").path,
        codex_home.display().to_string()
    );

    let claude = groups
        .iter()
        .find(|group| group.agent == Agent::Claude)
        .unwrap();
    assert_eq!(
        entry(claude, "配置文件").path,
        claude_dir.join(".claude.json").display().to_string()
    );
    assert_eq!(
        entry(claude, "配置目录").path,
        claude_dir.display().to_string()
    );

    fs::remove_dir_all(&sandbox).ok();
}

#[test]
fn missing_agent_reports_uncreated() {
    let mut names = vec!["HOME"];
    names.extend_from_slice(PATH_ENV);
    let guard = EnvGuard::new(&names);
    guard.clear(&names);
    // 空沙箱 HOME = 三个 Agent 都未安装
    let sandbox = env::temp_dir().join(format!("amc-settings-empty-{}", std::process::id()));
    fs::create_dir_all(&sandbox).unwrap();
    env::set_var("HOME", &sandbox);

    let groups = agent_config_paths().unwrap();
    assert_eq!(groups.len(), 3);
    for group in &groups {
        assert_eq!(group.entries.len(), 3);
        for entry in &group.entries {
            assert!(!entry.exists, "未安装的 Agent 不应虚报存在: {}", entry.path);
        }
    }

    fs::remove_dir_all(&sandbox).ok();
}

#[test]
fn exists_flags_match_filesystem() {
    let guard = EnvGuard::new(PATH_ENV);
    guard.clear(PATH_ENV);
    let sandbox = env::temp_dir().join(format!("amc-settings-exists-{}", std::process::id()));
    let codex_home = sandbox.join("codex");
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(codex_home.join("config.toml"), b"[mcp_servers]\n").unwrap();
    guard.set("CODEX_HOME", &codex_home);

    let groups = agent_config_paths().unwrap();
    let codex = groups
        .iter()
        .find(|group| group.agent == Agent::Codex)
        .unwrap();
    let config = entry(codex, "配置文件");
    assert!(config.exists);
    assert_eq!(config.kind, PathKind::File);
    let dir = entry(codex, "配置目录");
    assert!(dir.exists);
    assert_eq!(dir.kind, PathKind::Dir);

    // 未创建的路径不得虚报存在：与文件系统实况对比
    let home = crate::platform::home().unwrap();
    assert_eq!(
        entry(codex, "Skills 目录").exists,
        home.join(".agents/skills").exists()
    );

    fs::remove_dir_all(&sandbox).ok();
}
