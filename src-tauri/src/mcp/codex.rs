//! Codex 用户级 MCP 配置适配器（`$CODEX_HOME/config.toml`，默认
//! `~/.codex/config.toml`）。统一 spec 是 JSON，这里做 JSON↔TOML 转换：
//! 用 toml_edit 只改 `[mcp_servers.<name>]` 表，其余键、注释与格式全
//! 部保留。字段映射遵循 Codex 官方形态——stdio 写 command/args/env/cwd，
//! 远程写 url，headers 落到 `http_headers`，不写 `type` 键。

use super::{Location, McpWrite};
use crate::platform::{self, Result};
use serde_json::Value;
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use toml_edit::{Item, Table, TableLike};

/// Codex 的用户级配置目录：`CODEX_HOME`，默认 `~/.codex`。
pub(crate) fn config_dir(home: &Path) -> PathBuf {
    match non_empty(env::var_os("CODEX_HOME")) {
        Some(dir) => PathBuf::from(dir),
        None => home.join(".codex"),
    }
}

pub(crate) fn locate() -> Result<Location> {
    let dir = config_dir(&platform::home()?);
    Ok(Location {
        present: dir.is_dir(),
        file: dir.join("config.toml"),
    })
}

fn non_empty(value: Option<std::ffi::OsString>) -> Option<std::ffi::OsString> {
    value.filter(|v| !v.is_empty())
}

fn read(path: &Path) -> Result<(Option<Vec<u8>>, toml_edit::DocumentMut)> {
    let before = match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("读取 {} 失败: {e}", path.display())),
    };
    let document = match &before {
        // 解析失败必须报错而非用空文档顶替：写回空文档会把用户 config.toml
        // 里的其它段落（model/model_providers/注释等）整体清空。
        Some(bytes) => String::from_utf8_lossy(bytes)
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| format!("解析 {} 失败: {e}", path.display()))?,
        None => toml_edit::DocumentMut::new(),
    };
    Ok((before, document))
}

fn servers_table_mut(document: &mut toml_edit::DocumentMut) -> Result<&mut dyn TableLike> {
    if document
        .get("mcp_servers")
        .is_some_and(|item| !item.is_none())
        && document
            .get_mut("mcp_servers")
            .and_then(Item::as_table_like_mut)
            .is_none()
    {
        return Err("config.toml 的 mcp_servers 不是表，拒绝改写".into());
    }
    document
        .entry("mcp_servers")
        .or_insert_with(toml_edit::table);
    document
        .get_mut("mcp_servers")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| "config.toml 的 mcp_servers 不是表".into())
}

pub(crate) fn upsert(location: &Location, name: &str, spec: &Value) -> Result<McpWrite> {
    let (before, mut document) = read(&location.file)?;
    let table = json_to_toml_table(spec)?;
    servers_table_mut(&mut document)?.insert(name, Item::Table(table));
    Ok(McpWrite {
        path: location.file.clone(),
        before,
        after: document.to_string().into_bytes(),
    })
}

pub(crate) fn remove(location: &Location, name: &str) -> Result<Option<McpWrite>> {
    if !location.file.exists() {
        return Ok(None);
    }
    let (before, mut document) = read(&location.file)?;
    let Some(servers) = document
        .get_mut("mcp_servers")
        .and_then(Item::as_table_like_mut)
    else {
        return Ok(None);
    };
    servers.remove(name);
    if servers.is_empty() {
        document.as_table_mut().remove("mcp_servers");
    }
    Ok(Some(McpWrite {
        path: location.file.clone(),
        before,
        after: document.to_string().into_bytes(),
    }))
}

/// 统一 JSON spec → Codex TOML 服务表。核心字段强类型转换；其余字段
/// 通用透传（Codex 对未知键宽容）。
fn json_to_toml_table(spec: &Value) -> Result<Table> {
    let fields = spec.as_object().ok_or("MCP 配置必须是 JSON 对象")?;
    let remote = matches!(
        fields.get("type").and_then(Value::as_str),
        Some("http" | "sse")
    );
    let mut table = Table::new();
    if remote {
        let url = fields
            .get("url")
            .and_then(Value::as_str)
            .ok_or("远程 MCP 需要 URL")?;
        table["url"] = toml_edit::value(url);
        if let Some(headers) = fields.get("headers").and_then(Value::as_object) {
            let mut header_table = Table::new();
            for (key, value) in headers {
                if let Some(text) = value.as_str() {
                    header_table[key.as_str()] = toml_edit::value(text);
                }
            }
            if !header_table.is_empty() {
                table["http_headers"] = Item::Table(header_table);
            }
        }
    } else {
        let command = fields
            .get("command")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or("stdio MCP 需要 command")?;
        table["command"] = toml_edit::value(command);
        if let Some(args) = fields.get("args").and_then(Value::as_array) {
            let mut array = toml_edit::Array::new();
            for arg in args.iter().filter_map(Value::as_str) {
                array.push(arg);
            }
            if !array.is_empty() {
                table["args"] = toml_edit::value(array);
            }
        }
        if let Some(cwd) = fields.get("cwd").and_then(Value::as_str) {
            if !cwd.trim().is_empty() {
                table["cwd"] = toml_edit::value(cwd);
            }
        }
        if let Some(env) = fields.get("env").and_then(Value::as_object) {
            let mut env_table = Table::new();
            for (key, value) in env {
                if let Some(text) = value.as_str() {
                    env_table[key.as_str()] = toml_edit::value(text);
                }
            }
            if !env_table.is_empty() {
                table["env"] = Item::Table(env_table);
            }
        }
    }
    let core: &[&str] = if remote {
        &["type", "enabled", "url", "headers"]
    } else {
        &["type", "enabled", "command", "args", "env", "cwd"]
    };
    for (key, value) in fields {
        if core.contains(&key.as_str()) {
            continue;
        }
        if let Some(item) = json_value_to_toml(value) {
            table[key.as_str()] = item;
        }
    }
    Ok(table)
}

fn json_value_to_toml(value: &Value) -> Option<Item> {
    match value {
        Value::String(s) => Some(toml_edit::value(s.clone())),
        Value::Number(n) if n.is_u64() || n.is_i64() => n.as_i64().map(toml_edit::value),
        Value::Number(n) => n.as_f64().map(toml_edit::value),
        Value::Bool(b) => Some(toml_edit::value(*b)),
        Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                match item {
                    Value::String(s) => array.push(s.clone()),
                    Value::Number(n) => {
                        let number = n
                            .as_i64()
                            .map(toml_edit::Value::from)
                            .or_else(|| n.as_f64().map(toml_edit::Value::from));
                        array.push(number?);
                    }
                    Value::Bool(b) => array.push(*b),
                    _ => return None,
                }
            }
            Some(toml_edit::value(array))
        }
        Value::Object(fields) => {
            let mut table = Table::new();
            for (key, child) in fields {
                if let Some(item) = json_value_to_toml(child) {
                    table[key.as_str()] = item;
                }
            }
            Some(Item::Table(table))
        }
        Value::Null => None,
    }
}

/// Codex 写入的 TOML 预览：对**整份** config.toml 递归脱敏后再序列化。
/// 用户配置里其它段落（model_providers 的 experimental_bearer_token /
/// http_headers 等）同样可能携带凭据，不能只处理 mcp_servers。
pub(crate) fn preview_redacted(bytes: &[u8]) -> String {
    let Ok(document) = String::from_utf8_lossy(bytes).parse::<toml_edit::DocumentMut>() else {
        return "[Codex TOML 不可预览；敏感字段未显示]".into();
    };
    let mut document = document;
    redact_toml_table(document.as_table_mut());
    document.to_string()
}

fn redact_toml_table(table: &mut Table) {
    for (key, item) in table.iter_mut() {
        redact_toml_entry(key.get(), item);
    }
}

/// Item 入口（标准表条目）。
fn redact_toml_entry(key: &str, item: &mut Item) {
    let name = key.to_ascii_lowercase();
    // 敏感键：无论值是标量、内联表还是数组，整体替换。
    if platform::is_sensitive_mcp_key(&name) {
        *item = Item::Value(toml_edit::Value::from(platform::HIDDEN_MCP_VALUE));
        return;
    }
    // 凭据容器键：只替换其中的字符串值，保留键名结构。
    if name == "env" || name == "headers" || name == "http_headers" {
        if let Item::Table(table) = item {
            for (_, child) in table.iter_mut() {
                if let Some(value) = child.as_value_mut() {
                    *value = toml_edit::Value::from(platform::HIDDEN_MCP_VALUE);
                }
            }
            return;
        }
        if let Item::Value(value) = item {
            redact_container_value(value);
            return;
        }
        // ArrayOfTables 容器：逐表递归。
        if let Item::ArrayOfTables(tables) = item {
            for table in tables.iter_mut() {
                redact_toml_table(table);
            }
        }
        return;
    }
    match item {
        Item::Table(table) => redact_toml_table(table),
        Item::Value(value) => redact_toml_value(value),
        Item::ArrayOfTables(tables) => {
            for table in tables.iter_mut() {
                redact_toml_table(table);
            }
        }
        Item::None => {}
    }
}

/// Value 入口（内联表/数组条目）：与 Item 入口共用同一套键语义。
fn redact_toml_value(value: &mut toml_edit::Value) {
    match value {
        toml_edit::Value::InlineTable(inline) => {
            for (key, child) in inline.iter_mut() {
                let name = key.get().to_ascii_lowercase();
                if platform::is_sensitive_mcp_key(&name) {
                    *child = toml_edit::Value::from(platform::HIDDEN_MCP_VALUE);
                } else if name == "env" || name == "headers" || name == "http_headers" {
                    redact_container_value(child);
                } else {
                    redact_toml_value(child);
                }
            }
        }
        toml_edit::Value::Array(items) => {
            for item in items.iter_mut() {
                redact_toml_value(item);
            }
        }
        _ => {}
    }
}

/// 凭据容器（Value 形态）：其中所有字符串值替换为占位符，
/// 嵌套内联表继续递归。
fn redact_container_value(value: &mut toml_edit::Value) {
    match value {
        toml_edit::Value::InlineTable(inline) => {
            for (_, child) in inline.iter_mut() {
                if matches!(child, toml_edit::Value::String(_)) {
                    *child = toml_edit::Value::from(platform::HIDDEN_MCP_VALUE);
                } else {
                    redact_toml_value(child);
                }
            }
        }
        toml_edit::Value::Array(items) => {
            for item in items.iter_mut() {
                if matches!(item, toml_edit::Value::String(_)) {
                    *item = toml_edit::Value::from(platform::HIDDEN_MCP_VALUE);
                } else {
                    redact_toml_value(item);
                }
            }
        }
        _ => {}
    }
}
