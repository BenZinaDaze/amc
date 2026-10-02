//! Claude Code 用户级 MCP 配置适配器。目标文件是 `~/.claude.json`
//! （user scope 的 mcpServers 顶层键）；设置 `CLAUDE_CONFIG_DIR` 时，
//! Claude Code 把状态文件放在 `<配置目录>/.claude.json`。该文件由
//! Claude Code 自己拥有大量其它状态，只做 mcpServers 单键读-改-写。

use super::{json_remove, json_upsert, Location, McpWrite};
use crate::platform::{self, Result};
use serde_json::Value;
use std::{env, path::PathBuf};

pub(crate) fn locate() -> Result<Location> {
    let home = platform::home()?;
    match non_empty(env::var_os("CLAUDE_CONFIG_DIR")) {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            let file = dir.join(".claude.json");
            Ok(Location {
                present: dir.is_dir() || file.is_file(),
                file,
            })
        }
        None => {
            let file = home.join(".claude.json");
            Ok(Location {
                present: file.is_file() || home.join(".claude").is_dir(),
                file,
            })
        }
    }
}

fn non_empty(value: Option<std::ffi::OsString>) -> Option<std::ffi::OsString> {
    value.filter(|v| !v.is_empty())
}

pub(crate) fn upsert(location: &Location, name: &str, spec: &Value) -> Result<McpWrite> {
    json_upsert(&location.file, name, spec, false)
}

pub(crate) fn remove(location: &Location, name: &str) -> Result<Option<McpWrite>> {
    json_remove(&location.file, name)
}

