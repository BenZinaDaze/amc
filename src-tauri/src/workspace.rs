use crate::platform::{self, Result, SkillView};
use serde::{Deserialize, Serialize};
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

}
