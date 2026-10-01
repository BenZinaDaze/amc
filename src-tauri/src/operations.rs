use crate::{
    platform::{self, Result},
    store::{InstallRecord, Installation, Repository, Store},
    workspace::{self, Workspace},
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

const SCHEMA: &str = "https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/coding-agent/src/config/mcp-schema.json";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    installed: bool,
    version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    workspace: Workspace,
    agent: AgentInfo,
    mcp: Vec<platform::McpView>,
    skills: Vec<platform::SkillView>,
    repositories: Vec<Repository>,
    installations: Vec<Installation>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    path: String,
    before: String,
    after: String,
}

#[derive(Serialize)]
pub struct Plan {
    id: String,
    summary: String,
    changes: Vec<Change>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
pub struct Message {
    message: String,
}

#[derive(Serialize)]
pub struct RepositorySkill {
    path: String,
    name: String,
    description: String,
}

pub struct Core {
    pub store: Store,
    plans: HashMap<String, Pending>,
}

enum Pending {
    Mcp {
        root: PathBuf,
        writes: Vec<McpWrite>,
    },
    Skill {
        root: PathBuf,
        target: PathBuf,
        old_hash: Option<String>,
        record: Option<InstallRecord>,
        action: SkillAction,
    },
}

struct McpWrite {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
}

enum SkillAction {
    Install {
        repository_id: i64,
        skill_path: String,
        name: String,
        commit: String,
        files: Vec<(PathBuf, Vec<u8>)>,
        hash: String,
    },
    Remove,
    Rollback {
        backup: PathBuf,
    },
}

impl Core {
    pub fn new(root: PathBuf) -> Result<Self> {
        Ok(Self {
            store: Store::new(root)?,
            plans: HashMap::new(),
        })
    }

    pub fn state(&self, workspace: Workspace) -> Result<State> {
        let root = workspace::root(&workspace)?;
        let (installed, version) = crate::usage::omp_status();
        let installations = self
            .store
            .installations()?
            .into_iter()
            .filter(|entry| workspace::is_installation_target(&root, Path::new(&entry.target_path)))
            .collect();
        Ok(State {
            workspace: workspace::from_root(&root),
            agent: AgentInfo { installed, version },
            mcp: workspace::scan_mcp(&root)?,
            skills: workspace::scan_skills(&root)?,
            repositories: self.store.repositories()?,
            installations,
        })
    }

    fn enqueue(
        &mut self,
        summary: String,
        changes: Vec<Change>,
        warnings: Vec<String>,
        pending: Pending,
    ) -> Plan {
        let id = Uuid::new_v4().to_string();
        self.plans.insert(id.clone(), pending);
        Plan {
            id,
            summary,
            changes,
            warnings,
        }
    }

    pub fn plan_mcp(
        &mut self,
        workspace: Workspace,
        name: String,
        mut config: Option<Value>,
    ) -> Result<Plan> {
        if !platform::valid_mcp_name(&name) {
            return Err("MCP 名称无效（最多 100 字符，仅限字母数字、_-. :）".into());
        }
        if let Some(value) = &config {
            validate_mcp(value)?;
        }
        let is_update = config.is_some();
        let root = workspace::root(&workspace)?;
        let primary = workspace::mcp_write_path(&root)?;
        let legacy = if primary == root.join(".mcp.json") {
            root.join("mcp.json")
        } else {
            root.join(".mcp.json")
        };
        platform::no_links(&primary, &root)?;
        platform::no_links(&legacy, &root)?;
        let primary_doc = read_mcp_document(&primary)?;
        let legacy_doc = read_mcp_document(&legacy)?;
        let in_primary = primary_doc
            .get("mcpServers")
            .and_then(|v| v.get(&name))
            .is_some();
        let in_legacy = legacy_doc
            .get("mcpServers")
            .and_then(|v| v.get(&name))
            .is_some();
        if config.is_none() && !in_primary {
            return Err(if in_legacy {
                "该 MCP 位于只读 .mcp.json 来源，不能移除".into()
            } else {
                "此工作区中不存在该 MCP".into()
            });
        }
        let before = match fs::read(&primary) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("读取 {} 失败: {e}", primary.display())),
        };
        let mut doc = primary_doc;
        let map = doc.as_object_mut().ok_or("MCP JSON 根必须是对象")?;
        if is_update && !map.contains_key("$schema") {
            map.insert("$schema".into(), Value::String(SCHEMA.into()));
        }
        let servers = map
            .entry("mcpServers")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or("mcpServers 必须是对象")?;
        if let Some(mut value) = config.take() {
            let fallback = legacy_doc.get("mcpServers").and_then(|v| v.get(&name));
            platform::restore_mcp_secrets(&mut value, servers.get(&name).or(fallback))?;
            servers.insert(name.clone(), value);
        } else {
            servers.remove(&name);
        }
        let mut after = serde_json::to_vec_pretty(&doc).map_err(|e| e.to_string())?;
        after.push(b'\n');
        let changes = vec![Change {
            path: primary.display().to_string(),
            before: before.as_deref().map_or_else(String::new, preview_mcp),
            after: preview_mcp(&after),
        }];
        let writes = vec![McpWrite {
            path: primary,
            before,
            after,
        }];
        Ok(self.enqueue(
            format!(
                "{} MCP {name}",
                if !is_update {
                    "移除"
                } else if in_primary || in_legacy {
                    "更新"
                } else {
                    "新增"
                }
            ),
            changes,
            if is_update && !in_primary && in_legacy {
                vec!["该 MCP 来自只读来源；更新将在工作区 mcp.json 创建覆盖项。".into()]
            } else {
                vec![]
            },
            Pending::Mcp { root, writes },
        ))
    }

    pub fn add_repository(&mut self, url: String, reference: String) -> Result<Repository> {
        self.store.add_repository(&url, &reference)
    }

    pub fn remove_repository(&mut self, repository_id: i64) -> Result<Message> {
        Ok(Message {
            message: self.store.remove_repository(repository_id)?,
        })
    }

    pub fn repository_skills(&self, repository_id: i64) -> Result<Vec<RepositorySkill>> {
        self.store.repository(repository_id)?;
        let root = self.store.repo_dir(repository_id);
        let mut skills = Vec::new();
        fn scan(
            base: &Path,
            dir: &Path,
            depth: usize,
            result: &mut Vec<RepositorySkill>,
        ) -> Result<()> {
            if depth > 32 {
                return Err("仓库目录嵌套过深，无法安全扫描".into());
            }
            let file = dir.join("SKILL.md");
            if file.exists() {
                if fs::symlink_metadata(&file)
                    .map_err(|e| e.to_string())?
                    .file_type()
                    .is_symlink()
                {
                    if dir != base {
                        return Ok(());
                    }
                } else if let Ok((name, description)) = platform::skill_metadata(&file) {
                    if dir != base
                        && dir.file_name().and_then(|value| value.to_str()) != Some(name.as_str())
                    {
                        return Ok(());
                    }
                    let path = dir
                        .strip_prefix(base)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .into_owned();
                    result.push(RepositorySkill {
                        path: if path.is_empty() { ".".into() } else { path },
                        name,
                        description,
                    });
                }
                if dir != base {
                    return Ok(());
                }
            }
            let mut entries = fs::read_dir(dir)
                .map_err(|e| e.to_string())?
                .map(|entry| entry.map_err(|e| e.to_string()))
                .collect::<Result<Vec<_>>>()?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                if entry.file_name() == ".git" {
                    continue;
                }
                if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                    scan(base, &entry.path(), depth + 1, result)?;
                }
            }
            Ok(())
        }
        scan(&root, &root, 0, &mut skills)?;
        Ok(skills)
    }

    pub fn check_updates(&mut self, repository_id: i64) -> Result<Message> {
        let repo = self.store.repository(repository_id)?;
        let commit = self.store.update_repository(&repo)?;
        Ok(Message {
            message: format!(
                "已更新仓库缓存至 {}；安装记录将显示可用更新",
                &commit[..commit.len().min(12)]
            ),
        })
    }

    pub fn check_all_updates(&mut self) -> Result<Message> {
        let repositories = self.store.repositories()?;
        if repositories.is_empty() {
            return Ok(Message {
                message: "没有已添加的仓库".into(),
            });
        }
        let mut refreshed = 0;
        let mut failures = Vec::new();
        for repository in &repositories {
            match self.store.update_repository(repository) {
                Ok(_) => refreshed += 1,
                Err(error) => {
                    failures.push(format!("#{} {}：{}", repository.id, repository.url, error))
                }
            }
        }
        let message = if failures.is_empty() {
            format!("已刷新 {} 个仓库；安装记录已更新", refreshed)
        } else {
            format!(
                "已刷新 {}/{} 个仓库；失败：{}",
                refreshed,
                repositories.len(),
                failures.join("；")
            )
        };
        Ok(Message { message })
    }
    pub fn plan_skill(
        &mut self,
        workspace: Workspace,
        repository_id: i64,
        skill_path: String,
    ) -> Result<Plan> {
        let workspace_root = workspace::root(&workspace)?;
        let root = workspace::installation_root(&workspace_root)
            .parent()
            .ok_or("工作区安装目录无效")?
            .to_path_buf();
        self.plan_skill_at(root, repository_id, skill_path, None)
    }

    pub fn plan_sync_record(&mut self, record: InstallRecord) -> Result<Plan> {
        if !record.active {
            return Err("已移除的技能无法同步，请先回滚".into());
        }
        let path = PathBuf::from(&record.target_path);
        let skills_root = path.parent().ok_or("安装路径无效")?.to_path_buf();
        let workspace_root = skills_root
            .parent()
            .and_then(Path::parent)
            .ok_or("安装路径无效")?
            .to_path_buf();
        if !workspace::is_installation_target(&workspace_root, &path) {
            return Err("安装目标不在工作区 .agents/skills 内".into());
        }
        let root = skills_root.parent().ok_or("安装路径无效")?.to_path_buf();
        self.plan_skill_at(
            root,
            record.repository_id,
            record.skill_path.clone(),
            Some(path),
        )
    }

    fn plan_skill_at(
        &mut self,
        root: PathBuf,
        repository_id: i64,
        skill_path: String,
        expected_target: Option<PathBuf>,
    ) -> Result<Plan> {
        let repo_root = self.store.repo_dir(repository_id);
        let relative = Path::new(&skill_path);
        if skill_path != "."
            && (relative.as_os_str().is_empty()
                || !relative
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
                || relative.components().any(|part| part.as_os_str() == ".git"))
        {
            return Err("无效技能路径".into());
        }
        let source = repo_root.join(relative);
        platform::no_links(&source, &repo_root)?;
        if !source.is_dir() {
            return Err("仓库中不存在该技能目录".into());
        }
        let (name, _) = platform::skill_metadata(&source.join("SKILL.md"))?;
        if skill_path != "."
            && source.file_name().and_then(|value| value.to_str()) != Some(name.as_str())
        {
            return Err("技能 name 必须与目录名一致".into());
        }
        let files = platform::source_snapshot(&source, &repo_root)?;
        let hash = platform::tree_hash(&files);
        let commit = self.store.commit(repository_id)?;
        let target = root.join("skills").join(&name);
        let workspace_root = root.parent().ok_or("工作区安装目录无效")?;
        if !workspace::is_installation_target(workspace_root, &target) {
            return Err("安装目标不在工作区 .agents/skills 内".into());
        }
        if expected_target.is_some_and(|expected| expected != target) {
            return Err("技能名称已更改；不能原地同步旧安装".into());
        }
        platform::no_links(&target, &root)?;
        let existing = self.store.by_path(&target)?;
        let old = existing.as_ref().filter(|r| r.active);
        if target.exists() && old.is_none() {
            return Err("目标已有非 AMC 管理的技能，拒绝覆盖".into());
        }
        if let Some(record) = old {
            if record.repository_id != repository_id || record.skill_path != skill_path {
                return Err("目标技能来自另一个仓库，拒绝覆盖".into());
            }
        }
        let old_hash = checked_target(&target, old)?;
        let old_files = platform::snapshot(&target)?;
        let changes = diff_files(&target, &old_files, &files);
        Ok(self.enqueue(
            format!("{}技能 {name}", if old.is_some() { "更新" } else { "安装" }),
            changes,
            vec!["只复制技能文件；不会运行仓库脚本。".into()],
            Pending::Skill {
                root,
                target,
                old_hash,
                record: existing,
                action: SkillAction::Install {
                    repository_id,
                    skill_path,
                    name,
                    commit,
                    files,
                    hash,
                },
            },
        ))
    }

    pub fn plan_remove_skill(&mut self, installation_id: i64) -> Result<Plan> {
        let record = self.store.record(installation_id)?;
        if !record.active {
            return Err("技能已移除".into());
        }
        let (root, target, old_hash, files) = self.record_target(&record)?;
        let changes = diff_files(&target, &files, &[]);
        Ok(self.enqueue(
            format!("移除技能 {}", record.name),
            changes,
            vec!["只移除 AMC 管理且未被手动修改的安装。".into()],
            Pending::Skill {
                root,
                target,
                old_hash,
                record: Some(record),
                action: SkillAction::Remove,
            },
        ))
    }

    pub fn rollback_skill(&mut self, installation_id: i64) -> Result<Plan> {
        let record = self.store.record(installation_id)?;
        let backup = record
            .rollback_path
            .as_ref()
            .map(PathBuf::from)
            .ok_or("该安装没有可回滚的版本")?;
        if record.rollback_commit.is_none() || record.rollback_hash.is_none() {
            return Err("回滚备份缺少提交编号或文件校验".into());
        }
        let target = PathBuf::from(&record.target_path);
        let skills_root = target.parent().ok_or("安装路径无效")?.to_path_buf();
        let workspace_root = skills_root
            .parent()
            .and_then(Path::parent)
            .ok_or("安装路径无效")?
            .to_path_buf();
        let root = skills_root.parent().ok_or("安装路径无效")?.to_path_buf();
        if !workspace::is_installation_target(&workspace_root, &target) {
            return Err("安装目标不在工作区 .agents/skills 内".into());
        }
        platform::no_links(&target, &root)?;
        platform::no_links(&backup, &root)?;
        if !backup.is_dir() {
            return Err("回滚备份不存在".into());
        }
        let old_hash = checked_target(&target, if record.active { Some(&record) } else { None })?;
        if !record.active && target.exists() {
            return Err("目标路径已被占用，无法回滚".into());
        }
        let backup_files = platform::snapshot(&backup)?;
        if record.rollback_hash.as_deref() != Some(platform::tree_hash(&backup_files).as_str()) {
            return Err("回滚备份已变化".into());
        }
        let files = platform::snapshot(&target)?;
        let changes = diff_files(&target, &files, &backup_files);
        Ok(self.enqueue(
            format!("回滚技能 {}", record.name),
            changes,
            vec![],
            Pending::Skill {
                root,
                target,
                old_hash,
                record: Some(record),
                action: SkillAction::Rollback { backup },
            },
        ))
    }

    fn record_target(
        &self,
        record: &InstallRecord,
    ) -> Result<(PathBuf, PathBuf, Option<String>, Vec<(PathBuf, Vec<u8>)>)> {
        let target = PathBuf::from(&record.target_path);
        let skills_root = target.parent().ok_or("安装路径无效")?.to_path_buf();
        let workspace_root = skills_root
            .parent()
            .and_then(Path::parent)
            .ok_or("安装路径无效")?
            .to_path_buf();
        let root = skills_root.parent().ok_or("安装路径无效")?.to_path_buf();
        if !workspace::is_installation_target(&workspace_root, &target) {
            return Err("安装目标不在工作区 .agents/skills 内".into());
        }
        platform::no_links(&target, &root)?;
        let hash = checked_target(&target, Some(record))?;
        let files = platform::snapshot(&target)?;
        Ok((root, target, hash, files))
    }

    pub fn apply(&mut self, id: String) -> Result<Message> {
        let pending = self.plans.remove(&id).ok_or("方案已失效，请重新预览")?;
        match pending {
            Pending::Mcp { root, writes } => apply_mcp(&self.store.root, &root, &id, &writes),
            Pending::Skill {
                root,
                target,
                old_hash,
                record,
                action,
            } => {
                let skills_root = target.parent().ok_or("安装路径无效")?;
                let workspace_root = skills_root
                    .parent()
                    .and_then(Path::parent)
                    .ok_or("安装路径无效")?;
                if !workspace::is_installation_target(workspace_root, &target) {
                    return Err("安装目标不在工作区 .agents/skills 内".into());
                }
                platform::no_links(&target, &root)?;
                if let Some(previous) = &record {
                    let current = self.store.record(previous.id)?;
                    if current.commit != previous.commit
                        || current.hash != previous.hash
                        || current.active != previous.active
                        || current.rollback_path != previous.rollback_path
                        || current.rollback_hash != previous.rollback_hash
                        || current.rollback_commit != previous.rollback_commit
                    {
                        return Err("安装记录已改变，请重新预览".into());
                    }
                } else if self.store.by_path(&target)?.is_some() {
                    return Err("安装目标已被占用".into());
                }
                let current = if target.exists() {
                    Some(platform::tree_hash(&platform::snapshot(&target)?))
                } else {
                    None
                };
                if current != old_hash {
                    return Err("技能文件自预览后已变化，请重新预览".into());
                }
                self.apply_skill(&id, root, target, record, action)
            }
        }
    }

    fn apply_skill(
        &mut self,
        id: &str,
        root: PathBuf,
        target: PathBuf,
        record: Option<InstallRecord>,
        action: SkillAction,
    ) -> Result<Message> {
        let parent = target.parent().ok_or("技能安装目录无效")?;
        let backup_root = root.join(".amc-backups");
        let new_backup = backup_root.join(id);
        platform::no_links(&new_backup, &root)?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        fs::create_dir_all(&backup_root).map_err(|e| e.to_string())?;
        let stage = backup_root.join(format!(".stage-{id}"));
        let (new_files, source, message) = match &action {
            SkillAction::Install {
                repository_id,
                skill_path,
                files,
                hash,
                commit,
                ..
            } => {
                let source = self.store.repo_dir(*repository_id).join(skill_path);
                if self.store.commit(*repository_id)? != *commit
                    || platform::tree_hash(&platform::source_snapshot(
                        &source,
                        &self.store.repo_dir(*repository_id),
                    )?) != *hash
                {
                    return Err("仓库提交或技能文件自预览后已变化".into());
                }
                (Some(files), Some(source), "技能已安装/更新")
            }
            SkillAction::Remove => (None, None, "技能已移除；可使用安装编号回滚"),
            SkillAction::Rollback { backup } => {
                let old = record.as_ref().ok_or("安装记录不存在")?;
                if old.rollback_commit.is_none() || old.rollback_hash.is_none() {
                    return Err("回滚备份缺少提交编号或文件校验".into());
                }
                if old.rollback_hash.as_ref()
                    != Some(&platform::tree_hash(&platform::snapshot(backup)?))
                {
                    return Err("回滚备份已变化".into());
                }
                (None, None, "技能已回滚")
            }
        };
        if let Some(files) = new_files {
            fs::create_dir(&stage).map_err(|e| e.to_string())?;
            if let Err(e) = stage_files(&stage, files, source.as_deref().ok_or("技能来源缺失")?)
            {
                let _ = fs::remove_dir_all(&stage);
                return Err(e);
            }
        }
        let had_target = target.exists();
        if had_target {
            fs::rename(&target, &new_backup).map_err(|e| {
                let _ = fs::remove_dir_all(&stage);
                e.to_string()
            })?;
        }
        let incoming = match &action {
            SkillAction::Rollback { backup } => Some(backup.as_path()),
            SkillAction::Install { .. } => Some(stage.as_path()),
            SkillAction::Remove => None,
        };
        if let Some(incoming) = incoming {
            if let Err(e) = fs::rename(incoming, &target) {
                if had_target {
                    let _ = fs::rename(&new_backup, &target);
                }
                let _ = fs::remove_dir_all(&stage);
                return Err(format!("替换技能失败: {e}"));
            }
        }
        let db_result = self.save_install(
            &target,
            record.as_ref(),
            &action,
            if had_target { Some(&new_backup) } else { None },
        );
        if let Err(error) = db_result {
            let undo = match &action {
                SkillAction::Rollback { backup } if target.exists() => fs::rename(&target, backup),
                _ if target.exists() => fs::remove_dir_all(&target),
                _ => Ok(()),
            };
            if let Err(recovery) = undo {
                return Err(format!(
                    "{error}; 无法恢复技能文件 ({recovery})，请检查 {} 和 {}",
                    target.display(),
                    new_backup.display()
                ));
            }
            if had_target {
                fs::rename(&new_backup, &target).map_err(|recovery| {
                    format!(
                        "{error}; 无法恢复原始技能 ({recovery})，备份位于 {}",
                        new_backup.display()
                    )
                })?;
            }
            return Err(error);
        }
        if let Some(previous) = record.and_then(|r| r.rollback_path) {
            let previous = PathBuf::from(previous);
            if previous != target
                && previous != new_backup
                && previous.starts_with(&backup_root)
                && previous.exists()
            {
                let _ = fs::remove_dir_all(previous);
            }
        }
        Ok(Message {
            message: message.into(),
        })
    }

    fn save_install(
        &mut self,
        target: &Path,
        previous: Option<&InstallRecord>,
        action: &SkillAction,
        backup: Option<&Path>,
    ) -> Result<()> {
        let path = target.to_str().ok_or("技能安装路径必须是 UTF-8")?;
        match action {
            SkillAction::Install {
                repository_id,
                skill_path,
                name,
                commit,
                hash,
                ..
            } => {
                if let Some(old) = previous {
                    self.store.db.execute("UPDATE installations SET repository_id=?1,skill_path=?2,name=?3,git_commit=?4,content_hash=?5,rollback_path=?6,rollback_commit=?7,rollback_hash=?8,active=1 WHERE id=?9",
                        rusqlite::params![repository_id,skill_path,name,commit,hash,backup.map(|p| p.to_string_lossy().into_owned()),
                            backup.map(|_| old.commit.as_str()),backup.map(|_| old.hash.as_str()),old.id]).map_err(|e| e.to_string())?;
                } else {
                    self.store.db.execute("INSERT INTO installations(repository_id,skill_path,name,target_path,git_commit,content_hash) VALUES (?1,?2,?3,?4,?5,?6)",
                        rusqlite::params![repository_id,skill_path,name,path,commit,hash]).map_err(|e| e.to_string())?;
                }
            }
            SkillAction::Remove => {
                let old = previous.ok_or("安装记录不存在")?;
                self.store.db.execute("UPDATE installations SET rollback_path=?1,rollback_commit=?2,rollback_hash=?3,active=0 WHERE id=?4",
                    rusqlite::params![backup.map(|p| p.to_string_lossy().into_owned()),old.commit,old.hash,old.id]).map_err(|e| e.to_string())?;
            }
            SkillAction::Rollback { .. } => {
                let old = previous.ok_or("安装记录不存在")?;
                let new_commit = old.rollback_commit.as_ref().ok_or("备份缺少提交编号")?;
                let new_hash = old.rollback_hash.as_ref().ok_or("备份缺少文件校验")?;
                self.store.db.execute("UPDATE installations SET git_commit=?1,content_hash=?2,rollback_path=?3,rollback_commit=?4,rollback_hash=?5,active=1 WHERE id=?6",
                    rusqlite::params![new_commit,new_hash,backup.map(|p| p.to_string_lossy().into_owned()),
                        backup.map(|_| old.commit.as_str()), backup.map(|_| old.hash.as_str()), old.id]).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}

fn apply_mcp(data_dir: &Path, root: &Path, id: &str, writes: &[McpWrite]) -> Result<Message> {
    for write in writes {
        platform::no_links(&write.path, root)?;
        let actual = match fs::read(&write.path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("读取 {} 失败: {e}", write.path.display())),
        };
        if actual.as_ref() != write.before.as_ref() {
            return Err("MCP 文件自预览后已变化，请重新预览".into());
        }
    }
    let backup_dir = data_dir.join("backups/mcp");
    fs::create_dir_all(&backup_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let mut staged = Vec::new();
    for (index, write) in writes.iter().enumerate() {
        let temp = root.join(format!(".amc-{id}-{index}.tmp"));
        let stage_result = (|| -> Result<()> {
            if let Some(old) = &write.before {
                write_private(&backup_dir.join(format!("{id}-{index}.json")), old)?;
            }
            write_private(&temp, &write.after)?;
            if let Ok(metadata) = fs::metadata(&write.path) {
                fs::set_permissions(&temp, metadata.permissions()).map_err(|e| e.to_string())?;
            }
            Ok(())
        })();
        if let Err(error) = stage_result {
            let _ = fs::remove_file(&temp);
            for path in staged {
                let _ = fs::remove_file(path);
            }
            return Err(error);
        }
        staged.push(temp);
    }
    let mut completed = 0;
    let mut failure = None;
    for (index, write) in writes.iter().enumerate() {
        let old_path = root.join(format!(".amc-{id}-{index}.old"));
        if write.before.is_some() {
            if let Err(error) = fs::rename(&write.path, &old_path) {
                failure = Some(format!("备份旧 MCP 配置失败: {error}"));
                break;
            }
        }
        if let Err(error) = fs::rename(&staged[index], &write.path) {
            if write.before.is_some() {
                if let Err(recovery) = fs::rename(&old_path, &write.path) {
                    failure = Some(format!(
                        "写入失败: {error}; 恢复失败: {recovery}，旧文件在 {}",
                        old_path.display()
                    ));
                    break;
                }
            }
            failure = Some(format!("写入 MCP 配置失败: {error}"));
            break;
        }
        completed += 1;
    }
    if let Some(error) = failure {
        let mut recovery_error = None;
        for index in (0..completed).rev() {
            let write = &writes[index];
            let restored = if write.before.is_some() {
                fs::remove_file(&write.path).and_then(|_| {
                    fs::rename(root.join(format!(".amc-{id}-{index}.old")), &write.path)
                })
            } else {
                fs::remove_file(&write.path)
            };
            if let Err(e) = restored {
                recovery_error = Some(e.to_string());
            }
        }
        for path in staged {
            let _ = fs::remove_file(path);
        }
        if let Some(recovery) = recovery_error {
            return Err(format!(
                "{error}; 恢复失败: {recovery}，原文件备份在 {}",
                backup_dir.display()
            ));
        }
        return Err(error);
    }
    for (index, write) in writes.iter().enumerate() {
        if write.before.is_some() {
            let _ = fs::remove_file(root.join(format!(".amc-{id}-{index}.old")));
        }
    }
    Ok(Message {
        message: "MCP 配置已安全写入；请在 OMP 中重新加载 MCP。".into(),
    })
}

fn checked_target(target: &Path, record: Option<&InstallRecord>) -> Result<Option<String>> {
    if let Some(record) = record {
        if !target.is_dir() {
            return Err("已管理的技能目录已丢失，请手动恢复后重试".into());
        }
        let hash = platform::tree_hash(&platform::snapshot(target)?);
        if hash != record.hash {
            return Err("技能已被手动修改，拒绝覆盖；请先备份或恢复原文件".into());
        }
        Ok(Some(hash))
    } else if target.exists() {
        Err("目标已有非 AMC 管理的技能，拒绝覆盖".into())
    } else {
        Ok(None)
    }
}

fn validate_mcp(config: &Value) -> Result<()> {
    let fields = config.as_object().ok_or("MCP 配置必须是 JSON 对象")?;
    match fields
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("stdio")
    {
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

fn read_mcp_document(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({"mcpServers": {}}));
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
fn string_map(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|m| m.values().all(Value::is_string))
}
fn preview_mcp(data: &[u8]) -> String {
    if data.len() > 128 * 1024 {
        return "[MCP JSON 超出预览大小；敏感字段未显示]".into();
    }
    let Ok(mut doc) = serde_json::from_slice::<Value>(data) else {
        return "[MCP JSON 不可预览；敏感字段未显示]".into();
    };
    platform::redact_mcp_in_place(&mut doc);
    serde_json::to_string_pretty(&doc)
        .unwrap_or_else(|_| "[MCP JSON 不可预览；敏感字段未显示]".into())
}

fn preview(data: &[u8]) -> String {
    if data.len() <= 128 * 1024 {
        if let Ok(text) = std::str::from_utf8(data) {
            return text.to_owned();
        }
    }
    format!("[{} 字节; SHA-256 {}]", data.len(), platform::digest(data))
}
fn diff_files(
    target: &Path,
    before: &[(PathBuf, Vec<u8>)],
    after: &[(PathBuf, Vec<u8>)],
) -> Vec<Change> {
    let left: HashMap<&Path, &[u8]> = before
        .iter()
        .map(|(p, b)| (p.as_path(), b.as_slice()))
        .collect();
    let right: HashMap<&Path, &[u8]> = after
        .iter()
        .map(|(p, b)| (p.as_path(), b.as_slice()))
        .collect();
    let names: HashSet<&Path> = left.keys().chain(right.keys()).copied().collect();
    let mut names: Vec<_> = names.into_iter().collect();
    names.sort();
    names
        .into_iter()
        .filter(|name| left.get(name) != right.get(name))
        .map(|name| Change {
            path: target.join(name).display().to_string(),
            before: left.get(name).map_or_else(String::new, |v| preview(v)),
            after: right.get(name).map_or_else(String::new, |v| preview(v)),
        })
        .collect()
}
fn stage_files(directory: &Path, files: &[(PathBuf, Vec<u8>)], source: &Path) -> Result<()> {
    for (path, bytes) in files {
        let target = directory.join(path);
        fs::create_dir_all(target.parent().ok_or("非法文件路径")?).map_err(|e| e.to_string())?;
        write_private(&target, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let original_mode = fs::metadata(source.join(path))
                .map_err(|e| e.to_string())?
                .permissions()
                .mode();
            if original_mode & 0o111 != 0 {
                fs::set_permissions(&target, fs::Permissions::from_mode(0o700))
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options
        .open(path)
        .map_err(|e| format!("创建 {} 失败: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

#[cfg(test)]
#[path = "operations_tests.rs"]
mod tests;
