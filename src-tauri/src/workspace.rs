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
}

pub fn skill_sources(root: &Path) -> Result<Vec<SkillSource>> {
    let home = home()?;
    Ok(vec![
        SkillSource {
            path: root.join(".agents/skills"),
            label: "Agents · generic",
        },
        SkillSource {
            path: root.join(".claude/skills"),
            label: "Claude · detected",
        },
        SkillSource {
            path: root.join(".codex/skills"),
            label: "Codex · OMP opt-in",
        },
        SkillSource {
            path: root.join(".omp/skills"),
            label: "OMP · compatibility",
        },
        SkillSource {
            path: home.join(".agents/skills"),
            label: "通用 · OMP/Codex 用户级",
        },
        SkillSource {
            path: home.join(".claude/skills"),
            label: "Claude · user",
        },
        SkillSource {
            path: home.join(".codex/skills"),
            label: "Codex · OMP opt-in",
        },
        SkillSource {
            path: home.join(".omp/agent/skills"),
            label: "OMP · compatibility",
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
                shadowed,
                description,
            });
        }
    }
    Ok(result)
}

pub fn from_root(root: &Path) -> Workspace {
    Workspace {
        path: root.to_string_lossy().into_owned(),
    }
}
