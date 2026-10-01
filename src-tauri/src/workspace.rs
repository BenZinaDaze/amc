use crate::platform::{self, McpView, Result, SkillView};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    env, fs,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub path: String,
}

pub struct McpSource {
    pub path: PathBuf,
    pub label: &'static str,
    pub writable: bool,
}

pub fn root(workspace: &Workspace) -> Result<PathBuf> {
    let supplied = Path::new(&workspace.path);
    if !supplied.is_absolute()
        || supplied
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("工作区路径必须是无 .. 的绝对目录".into());
    }
    let root = fs::canonicalize(supplied).map_err(|e| format!("工作区目录无效: {e}"))?;
    if !root.is_dir() {
        return Err("工作区路径必须指向目录".into());
    }
    platform::no_links(&root, &root)?;
    Ok(root)
}

pub fn default_workspace() -> Result<Workspace> {
    Ok(from_root(&default_root()?))
}

pub fn default_root() -> Result<PathBuf> {
    let home = fs::canonicalize(home()?).map_err(|e| format!("用户目录无效: {e}"))?;
    if !home.is_dir() {
        return Err("用户目录必须是目录".into());
    }
    platform::no_links(&home, &home)?;
    Ok(home)
}

fn home() -> Result<PathBuf> {
    let value = if cfg!(windows) {
        env::var_os("USERPROFILE").or_else(|| env::var_os("HOME"))
    } else {
        env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"))
    };
    value
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "无法定位用户目录，无法扫描通用来源".into())
}

pub fn mcp_write_path(root: &Path) -> Result<PathBuf> {
    let home = fs::canonicalize(home()?).map_err(|e| format!("用户目录无效: {e}"))?;
    if root == home {
        Ok(root.join(".mcp.json"))
    } else {
        Ok(root.join("mcp.json"))
    }
}

fn platform_mcp_config(home: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return Some(home.join("Library/Application Support/Claude/claude_desktop_config.json"));
    }
    #[cfg(target_os = "windows")]
    {
        return env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|dir| dir.join("Claude/claude_desktop_config.json"));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Some(home.join(".config/Claude/claude_desktop_config.json"))
    }
}

pub fn mcp_sources(root: &Path) -> Result<Vec<McpSource>> {
    let home = fs::canonicalize(home()?).map_err(|e| format!("用户目录无效: {e}"))?;
    let user_scope = root == home;
    let mut sources = Vec::new();
    if user_scope {
        sources.push(McpSource {
            path: home.join(".mcp.json"),
            label: "通用用户 · .mcp.json",
            writable: true,
        });
        sources.push(McpSource {
            path: home.join("mcp.json"),
            label: "通用用户 · mcp.json",
            writable: false,
        });
    } else {
        sources.push(McpSource {
            path: root.join("mcp.json"),
            label: "通用项目 · mcp.json",
            writable: true,
        });
        sources.push(McpSource {
            path: root.join(".mcp.json"),
            label: "通用项目 · .mcp.json",
            writable: false,
        });
        sources.push(McpSource {
            path: home.join(".mcp.json"),
            label: "通用用户 · .mcp.json",
            writable: false,
        });
        sources.push(McpSource {
            path: home.join("mcp.json"),
            label: "通用用户 · mcp.json",
            writable: false,
        });
    }
    sources.extend([
        McpSource {
            path: root.join(".claude/mcp.json"),
            label: "Claude · detected",
            writable: false,
        },
        McpSource {
            path: root.join(".cursor/mcp.json"),
            label: "Cursor · detected",
            writable: false,
        },
        McpSource {
            path: home.join(".claude/mcp.json"),
            label: "Claude · user",
            writable: false,
        },
        McpSource {
            path: home.join(".cursor/mcp.json"),
            label: "Cursor · user",
            writable: false,
        },
        McpSource {
            path: root.join(".omp/mcp.json"),
            label: "OMP · compatibility",
            writable: false,
        },
        McpSource {
            path: home.join(".omp/agent/mcp.json"),
            label: "OMP · compatibility",
            writable: false,
        },
    ]);
    if let Some(path) = platform_mcp_config(&home) {
        sources.push(McpSource {
            path,
            label: "Claude Desktop · platform",
            writable: false,
        });
    }
    Ok(sources)
}

fn read_mcp(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(serde_json::json!({"mcpServers": {}}));
    }
    let bytes = fs::read(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("MCP JSON {} 无效: {e}", path.display()))?;
    if !document.is_object() || document.get("mcpServers").is_some_and(|v| !v.is_object()) {
        return Err(format!(
            "MCP JSON {} 的 mcpServers 必须是对象",
            path.display()
        ));
    }
    Ok(document)
}

pub fn scan_mcp(root: &Path) -> Result<Vec<McpView>> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut paths = HashSet::new();
    for source in mcp_sources(root)? {
        if !paths.insert(source.path.clone()) || !source.path.is_file() {
            continue;
        }
        platform::no_links(
            &source.path,
            source.path.parent().ok_or("MCP 来源路径无效")?,
        )?;
        let value = read_mcp(&source.path)?;
        let disabled: HashSet<&str> = value
            .get("disabledServers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let enabled: HashSet<&str> = value
            .get("enabledServers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if let Some(servers) = value.get("mcpServers").and_then(Value::as_object) {
            for (name, config) in servers {
                if seen.insert(name.clone()) {
                    result.push(McpView {
                        name: name.clone(),
                        config: platform::redacted_mcp(config),
                        source: format!("{} · {}", source.label, source.path.display()),
                        enabled: !disabled.contains(name.as_str())
                            && (enabled.contains(name.as_str())
                                || config.get("enabled") != Some(&Value::Bool(false))),
                        managed: source.writable,
                    });
                }
            }
        }
    }
    Ok(result)
}

pub struct SkillSource {
    pub path: PathBuf,
    pub label: &'static str,
    pub writable: bool,
}

pub fn skill_sources(root: &Path) -> Result<Vec<SkillSource>> {
    let home = home()?;
    Ok(vec![
        SkillSource {
            path: root.join(".agents/skills"),
            label: "Agents · generic",
            writable: true,
        },
        SkillSource {
            path: root.join(".claude/skills"),
            label: "Claude · detected",
            writable: false,
        },
        SkillSource {
            path: root.join(".codex/skills"),
            label: "Codex · detected",
            writable: false,
        },
        SkillSource {
            path: root.join(".omp/skills"),
            label: "OMP · compatibility",
            writable: false,
        },
        SkillSource {
            path: home.join(".agents/skills"),
            label: "Agents · user-compatible",
            writable: false,
        },
        SkillSource {
            path: home.join(".claude/skills"),
            label: "Claude · user-compatible",
            writable: false,
        },
        SkillSource {
            path: home.join(".codex/skills"),
            label: "Codex · user-compatible",
            writable: false,
        },
        SkillSource {
            path: home.join(".omp/agent/skills"),
            label: "OMP · compatibility",
            writable: false,
        },
    ])
}

pub fn scan_skills(root: &Path) -> Result<Vec<SkillView>> {
    let mut result = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut seen_names = HashSet::new();
    for source in skill_sources(root)? {
        if !seen_paths.insert(source.path.clone()) || !source.path.is_dir() {
            continue;
        }
        platform::no_links(
            &source.path,
            source.path.parent().ok_or("技能来源路径无效")?,
        )?;
        let mut entries = fs::read_dir(&source.path)
            .map_err(|e| format!("读取 {} 失败: {e}", source.path.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let entry_type = entry.file_type().map_err(|e| e.to_string())?;
            if !entry_type.is_dir() {
                continue;
            }
            let directory = entry.path();
            let skill_file = directory.join("SKILL.md");
            if !skill_file.is_file()
                || fs::symlink_metadata(&skill_file)
                    .map_err(|e| e.to_string())?
                    .file_type()
                    .is_symlink()
            {
                continue;
            }
            let Ok((name, description)) = platform::skill_metadata(&skill_file) else {
                continue;
            };
            if directory.file_name().and_then(|value| value.to_str()) != Some(name.as_str()) {
                continue;
            }
            let shadowed = !seen_names.insert(name.clone());
            result.push(SkillView {
                name,
                path: directory.to_string_lossy().into_owned(),
                source: source.label.into(),
                managed: source.writable,
                shadowed,
                description,
            });
        }
    }
    Ok(result)
}

pub fn installation_root(root: &Path) -> PathBuf {
    root.join(".agents/skills")
}

pub fn from_root(root: &Path) -> Workspace {
    Workspace {
        path: root.to_string_lossy().into_owned(),
    }
}

pub fn is_installation_target(root: &Path, path: &Path) -> bool {
    let Ok(canonical_root) = fs::canonicalize(root) else {
        return false;
    };
    let expected = installation_root(&canonical_root);
    let Some(parent) = path.parent() else {
        return false;
    };
    if !path.is_absolute()
        || !platform::valid_skill_name(
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(""),
        )
    {
        return false;
    }
    let mut existing = parent;
    while !existing.exists() {
        let Some(next) = existing.parent() else {
            return false;
        };
        existing = next;
    }
    let Ok(canonical_existing) = fs::canonicalize(existing) else {
        return false;
    };
    if platform::no_links(&canonical_existing, &canonical_root).is_err() {
        return false;
    }
    match fs::canonicalize(parent) {
        Ok(canonical) if canonical == expected => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && parent == expected => {}
        Ok(_) | Err(_) => return false,
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => false,
        Ok(_) => fs::canonicalize(path)
            .is_ok_and(|canonical| canonical.parent() == Some(expected.as_path())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn installation_target_requires_direct_safe_child() {
        let root = std::env::temp_dir().join(format!("amc-workspace-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join(".agents/skills")).unwrap();
        let direct = root.join(".agents/skills/example");
        let nested = root.join(".agents/skills/nested/example");
        assert!(is_installation_target(&root, &direct));
        assert!(!is_installation_target(&root, &nested));
        #[cfg(unix)]
        {
            let outside = root.join("outside");
            fs::create_dir_all(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, &direct).unwrap();
            assert!(!is_installation_target(&root, &direct));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_workspace_is_canonical_and_uses_scope_specific_mcp_paths() {
        let home = fs::canonicalize(super::home().unwrap()).unwrap();
        let workspace = default_workspace().unwrap();
        assert_eq!(Path::new(&workspace.path), home.as_path());
        assert_eq!(mcp_write_path(&home).unwrap(), home.join(".mcp.json"));
        assert_eq!(installation_root(&home), home.join(".agents/skills"));

        let project = home.join(format!(".amc-test-project-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&project).unwrap();
        assert_eq!(mcp_write_path(&project).unwrap(), project.join("mcp.json"));
        assert_ne!(mcp_write_path(&project).unwrap(), project.join(".mcp.json"));
        fs::remove_dir_all(project).unwrap();
    }
}
