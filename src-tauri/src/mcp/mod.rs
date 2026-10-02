// MCP 管理（cc-switch 模型）：amc.sqlite3 的 mcp_servers 表是唯一事实
// 来源，OMP / Claude Code / Codex 的用户级配置文件只是投影。写入永远从
// 统一记录出发，对各 Agent 文件做单条目的读-改-写；Agent 侧文件只在
// 空库自动导入或用户显式导入时反向进入 DB，且同名单单补开关、不覆盖
// 已保存的 spec。
pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod omp;

use crate::{
    platform::{self, Result},
    store::{McpRecord, Store},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) const SCHEMA: &str = "https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/coding-agent/src/config/mcp-schema.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Omp,
    Claude,
    Codex,
}

pub const AGENTS: [Agent; 3] = [Agent::Omp, Agent::Claude, Agent::Codex];

impl Agent {
    pub fn title(self) -> &'static str {
        match self {
            Agent::Omp => "OMP",
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct McpView {
    pub name: String,
    pub config: Value,
    pub agents: Vec<Agent>,
}

pub(crate) struct McpWrite {
    pub path: PathBuf,
    pub before: Option<Vec<u8>>,
    pub after: Vec<u8>,
}

pub(crate) enum McpAction {
    Save { record: McpRecord },
    Delete { name: String },
    Toggle { name: String, agent: Agent, enabled: bool },
}

/// 各 Agent 用户级配置文件的定位结果。`present` 表示该 Agent 已初始化
/// （配置目录/文件存在）；未安装的 Agent 只跳过写入，不凭空创建文件。
pub(crate) struct Location {
    pub file: PathBuf,
    pub present: bool,
}

pub(crate) struct Targets {
    pub omp: Location,
    pub claude: Location,
    pub codex: Location,
}

impl Targets {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            omp: omp::locate()?,
            claude: claude::locate()?,
            codex: codex::locate()?,
        })
    }

    fn location(&self, agent: Agent) -> &Location {
        match agent {
            Agent::Omp => &self.omp,
            Agent::Claude => &self.claude,
            Agent::Codex => &self.codex,
        }
    }
}

fn upsert(agent: Agent, location: &Location, name: &str, spec: &Value) -> Result<McpWrite> {
    match agent {
        Agent::Omp => omp::upsert(location, name, spec),
        Agent::Claude => claude::upsert(location, name, spec),
        Agent::Codex => codex::upsert(location, name, spec),
    }
}

fn remove(agent: Agent, location: &Location, name: &str) -> Result<Option<McpWrite>> {
    match agent {
        Agent::Omp => omp::remove(location, name),
        Agent::Claude => claude::remove(location, name),
        Agent::Codex => codex::remove(location, name),
    }
}

/// 只取 AMC 管理的记录；历史遗留/无法证明来源的行按不存在处理。
fn managed_record(store: &Store, name: &str) -> Result<McpRecord> {
    store.mcp_server(name)?.filter(|record| record.managed).ok_or_else(|| {
        format!("MCP 服务 {name} 不存在或非 AMC 管理")
    })
}

/// 把一条统一记录投影到各 Agent 配置文件，返回待写入文件与提示。
/// 数据库动作（保存/删除/改开关）由调用方在文件写成功后执行。
pub(crate) fn project(
    store: &Store,
    targets: &Targets,
    action: &McpAction,
) -> Result<(Vec<McpWrite>, Vec<String>)> {
    let mut writes = Vec::new();
    let mut warnings = Vec::new();
    match action {
        McpAction::Save { record } => {
            // 同名未管理记录（历史遗留/旧版添加）允许被显式保存接管：
            // 这是用户迁移到 AMC 管理的唯一路径；切换/删除仍保持保护。
            let previous = store
                .mcp_server(&record.name)?
                .filter(|record| record.managed);
            for agent in AGENTS {
                let location = targets.location(agent);
                if record.enabled_for(agent) {
                    if !location.present {
                        warnings.push(format!(
                            "未检测到 {} 的配置目录，已跳过该 Agent 的写入；安装后可再次保存同步。",
                            agent.title()
                        ));
                        continue;
                    }
                    // spec 在 plan_mcp 入队前已完成密钥回填，这里直接投影；
                    // 数据库中的统一记录永远只保存真实值。
                    writes.push(upsert(agent, location, &record.name, &record.spec)?);
                } else if previous
                    .as_ref()
                    .is_some_and(|record| record.enabled_for(agent))
                    && location.present
                {
                    writes.extend(remove(agent, location, &record.name)?);
                }
            }
        }
        McpAction::Delete { name } => {
            let previous = managed_record(store, name)?;
            for agent in AGENTS {
                let location = targets.location(agent);
                if previous.enabled_for(agent) && location.present {
                    writes.extend(remove(agent, location, name)?);
                }
            }
        }
        McpAction::Toggle { name, agent, enabled } => {
            let record = managed_record(store, name)?;
            let location = targets.location(*agent);
            if *enabled {
                if !location.present {
                    warnings.push(format!(
                        "未检测到 {} 的配置目录，开关已记录但未写入文件；安装后可再次切换同步。",
                        agent.title()
                    ));
                } else {
                    writes.push(upsert(*agent, location, name, &record.spec)?);
                }
            } else if location.present {
                writes.extend(remove(*agent, location, name)?);
            }
        }
    }
    Ok((writes, warnings))
}

/// 列表数据：只包含通过 AMC 保存的服务。
pub fn state(store: &Store) -> Result<Vec<McpView>> {
    Ok(store
        .mcp_servers()?
        .into_iter()
        .map(|record| McpView {
            agents: AGENTS
                .into_iter()
                .filter(|agent| record.enabled_for(*agent))
                .collect(),
            name: record.name,
            config: platform::redacted_mcp(&record.spec),
        })
        .collect())
}

/// 统一 spec 校验（与旧版一致：stdio 需 command，远程需 HTTP(S) URL）。
pub(crate) fn validate_spec(spec: &Value) -> Result<()> {
    let fields = spec.as_object().ok_or("MCP 配置必须是 JSON 对象")?;
    match fields.get("type").and_then(Value::as_str).unwrap_or("stdio") {
        "stdio" => {
            if fields
                .get("command")
                .and_then(Value::as_str)
                .is_none_or(|s| s.trim().is_empty())
            {
                return Err("stdio MCP 需要 command".into());
            }
            if fields.get("args").is_some_and(|v| {
                v.as_array()
                    .is_none_or(|a| a.iter().any(|v| !v.is_string()))
            }) {
                return Err("args 必须是字符串数组".into());
            }
            if fields.get("env").is_some_and(|v| !string_map(v)) {
                return Err("env 必须是字符串映射".into());
            }
        }
        "http" | "sse" => {
            let url = fields
                .get("url")
                .and_then(Value::as_str)
                .ok_or("远程 MCP 需要 URL")?;
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err("MCP URL 必须使用 HTTP(S)".into());
            }
            if fields.get("headers").is_some_and(|v| !string_map(v)) {
                return Err("headers 必须是字符串映射".into());
            }
        }
        _ => return Err("MCP type 仅支持 stdio、http、sse".into()),
    }
    Ok(())
}

/// 启用开关归数据库列管理，spec 里不允许携带。
pub(crate) fn strip_enabled(spec: &mut Value) {
    if let Some(fields) = spec.as_object_mut() {
        fields.remove("enabled");
    }
}

fn string_map(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|m| m.values().all(Value::is_string))
}

// —— JSON 形配置（OMP / Claude Code）共用读写 ——————————————————

pub(crate) fn json_upsert(
    path: &Path,
    name: &str,
    spec: &Value,
    add_schema: bool,
) -> Result<McpWrite> {
    let before = read_bytes(path)?;
    let mut document = parse_json(before.as_deref(), path)?;
    {
        let fields = document
            .as_object_mut()
            .ok_or_else(|| format!("JSON {} 的根必须是对象", path.display()))?;
        if add_schema && !fields.contains_key("$schema") {
            fields.insert("$schema".into(), Value::String(SCHEMA.into()));
        }
        let servers = fields
            .entry("mcpServers")
            .or_insert_with(|| Value::Object(Default::default()));
        if !servers.is_object() {
            return Err(format!(
                "JSON {} 的 mcpServers 必须是对象",
                path.display()
            ));
        }
        servers
            .as_object_mut()
            .ok_or_else(|| format!("JSON {} 的 mcpServers 必须是对象", path.display()))?
            .insert(name.to_string(), spec.clone());
    }
    let mut after = serde_json::to_vec_pretty(&document).map_err(|e| e.to_string())?;
    after.push(b'\n');
    Ok(McpWrite {
        path: path.to_path_buf(),
        before,
        after,
    })
}

pub(crate) fn json_remove(path: &Path, name: &str) -> Result<Option<McpWrite>> {
    let Some(before) = read_bytes(path)? else {
        return Ok(None);
    };
    let mut document = parse_json(Some(&before), path)?;
    let Some(servers) = document
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
    else {
        return Ok(None);
    };
    servers.remove(name);
    let mut after = serde_json::to_vec_pretty(&document).map_err(|e| e.to_string())?;
    after.push(b'\n');
    Ok(Some(McpWrite {
        path: path.to_path_buf(),
        before: Some(before),
        after,
    }))
}

fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取 {} 失败: {e}", path.display())),
    }
}

fn parse_json(bytes: Option<&[u8]>, path: &Path) -> Result<Value> {
    bytes.map_or_else(
        || Ok(Value::Object(Default::default())),
        |bytes| {
            serde_json::from_slice(bytes)
                .map_err(|e| format!("JSON {} 无效: {e}", path.display()))
        },
    )
}
