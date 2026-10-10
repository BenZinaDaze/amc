//! 设置页数据：各 Agent 用户级配置文件/目录的实际位置。
//! 路径全部复用 MCP / Skills 适配器的定位逻辑（含 CODEX_HOME、
//! CLAUDE_CONFIG_DIR 与 OMP profile 解析），保证与写入位置一致。

use crate::mcp::{self, Agent};
use crate::platform::{self, Result};
use crate::skills::SkillTarget;
use crate::usage::omp_mcp_location;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PathKind {
    File,
    Dir,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConfigPathEntry {
    pub purpose: String,
    pub path: String,
    pub kind: PathKind,
    pub exists: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigPaths {
    pub agent: Agent,
    pub entries: Vec<ConfigPathEntry>,
}

fn entry(purpose: &str, path: PathBuf, kind: PathKind) -> ConfigPathEntry {
    let exists = match kind {
        PathKind::File => path.is_file(),
        PathKind::Dir => path.is_dir(),
    };
    ConfigPathEntry {
        purpose: purpose.to_owned(),
        path: path.display().to_string(),
        kind,
        exists,
    }
}

/// 汇总各 Agent 的用户级配置位置，按 OMP / Claude Code / Codex 排列。
pub fn agent_config_paths() -> Result<Vec<AgentConfigPaths>> {
    let home = platform::home()?;
    let targets = mcp::Targets::from_env()?;
    let omp = omp_mcp_location()?;
    Ok(vec![
        AgentConfigPaths {
            agent: Agent::Omp,
            entries: vec![
                entry("配置目录", omp.config_root.clone(), PathKind::Dir),
                entry("Skills 目录", SkillTarget::Omp.dir(&home), PathKind::Dir),
                entry("配置文件", omp.config_root.join("config.yml"), PathKind::File),
            ],
        },
        AgentConfigPaths {
            agent: Agent::Claude,
            entries: vec![
                entry("配置目录", mcp::claude::config_dir(&home), PathKind::Dir),
                entry("Skills 目录", SkillTarget::Claude.dir(&home), PathKind::Dir),
                entry("配置文件", targets.claude.file, PathKind::File),
            ],
        },
        AgentConfigPaths {
            agent: Agent::Codex,
            entries: vec![
                entry("配置目录", mcp::codex::config_dir(&home), PathKind::Dir),
                entry("Skills 目录", SkillTarget::Codex.dir(&home), PathKind::Dir),
                entry("配置文件", targets.codex.file, PathKind::File),
            ],
        },
    ])
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
