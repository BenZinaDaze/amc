//! ZCode 用户级 MCP 配置适配器。目标文件是 `~/.zcode/cli/config.json` 的
//! `mcp.servers`（嵌套键，区别于 OMP / Claude Code 的顶层 `mcpServers`；
//! 不写 `$schema`——ZCode 的 server 条目 schema 严格，未知键会让条目被
//! 整个丢弃）。该文件同时承载 hooks / plugins 状态，只做 `mcp` 单键的
//! 读-改-写。
//!
//! 不要改走 `~/.agents/mcp.json` 兼容回退：ZCode 只在 `.zcode` 侧一个
//! MCP server 都没有时才读它，用户在设置界面手加任意 server 都会让
//! 回退整体失效，AMC 同步的服务会静默消失。

use super::{json_remove_at, json_upsert_at, Location, McpWrite};
use crate::platform::{self, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) fn config_dir(home: &Path) -> PathBuf {
    home.join(".zcode").join("cli")
}

pub(crate) fn locate() -> Result<Location> {
    let home = platform::home()?;
    let dir = config_dir(&home);
    let file = dir.join("config.json");
    Ok(Location {
        present: dir.is_dir() || file.is_file(),
        file,
    })
}

pub(crate) fn upsert(location: &Location, name: &str, spec: &Value) -> Result<McpWrite> {
    json_upsert_at(&location.file, &["mcp", "servers"], name, spec, false)
}

pub(crate) fn remove(location: &Location, name: &str) -> Result<Option<McpWrite>> {
    json_remove_at(&location.file, &["mcp", "servers"], name)
}
