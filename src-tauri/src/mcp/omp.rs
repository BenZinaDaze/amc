//! OMP 用户级 MCP 配置（`<agent 目录>/mcp.json`）适配器。
//! 路径解析复用 usage 适配器（PI_CONFIG_DIR / PI_CODING_AGENT_DIR /
//! profile），保证写入位置与 OMP 自身读取位置一致。

use super::{json_remove, json_upsert, Location, McpWrite};
use crate::platform::Result;
use serde_json::Value;

pub(crate) fn locate() -> Result<Location> {
    let location = crate::usage::omp_mcp_location()?;
    Ok(Location {
        present: location.config_root.is_dir(),
        file: location.mcp_json,
    })
}

pub(crate) fn upsert(location: &Location, name: &str, spec: &Value) -> Result<McpWrite> {
    json_upsert(&location.file, name, spec, true)
}

pub(crate) fn remove(location: &Location, name: &str) -> Result<Option<McpWrite>> {
    json_remove(&location.file, name)
}

