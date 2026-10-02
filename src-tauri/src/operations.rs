use crate::{
    mcp::{self, Agent, McpAction, McpWrite},
    platform::{self, Result},
    skills::{self, SkillTarget, SkillWrite},
    store::{McpRecord, Repository, SkillRecord, Store},
    workspace::{self, Workspace},
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

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
    /// 检测到已安装的 Agent（扫描目录中存在可执行文件，与 Agents 页
    /// 同源）；MCP / Skills 的 Agent 开关只对列表内的 Agent 显示。
    installed_agents: Vec<Agent>,
    mcp: Vec<mcp::McpView>,
    /// 分发管理的技能（skills 表 + 中央副本投影到用户级目录）。
    skills: Vec<skills::SkillView>,
    /// 工作区内检测到、但不归 AMC 管理的技能（只读展示）。
    detected: Vec<platform::SkillView>,
    repositories: Vec<Repository>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    path: String,
    before: String,
    after: String,
}

#[derive(Debug, Serialize)]
pub struct Plan {
    id: String,
    summary: String,
    changes: Vec<Change>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
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
    Mcp(Box<McpPending>),
    Skills(Box<SkillsPending>),
}

struct McpPending {
    writes: Vec<McpWrite>,
    action: McpAction,
}

/// 分发技能的待执行写入：目录级投影 + 配置文件投影 + 动作。
struct SkillsPending {
    writes: Vec<SkillWrite>,
    config_writes: Vec<skills::ConfigWrite>,
    action: skills::SkillAction,
}

/// `validated_skill_source` 的返回：(技能名, 文件快照, 内容哈希, 提交)。
type ValidatedSource = (String, Vec<(PathBuf, Vec<u8>)>, String, String);

impl Core {
    pub fn new(root: PathBuf) -> Result<Self> {
        let store = Store::new(root)?;
        // 一次性迁移：旧版工作区安装（installations 表）→ 分发模型，
        // 完成后旧表即被删除。
        skills::migrate_legacy(&store)?;
        Ok(Self {
            store,
            plans: HashMap::new(),
        })
    }

    pub fn state(&self, workspace: Workspace) -> Result<State> {
        let root = workspace::root(&workspace)?;
        let (installed, version) = crate::usage::omp_status();
        let claude = crate::usage::claude_code_status();
        let codex = crate::usage::codex_status();
        let skill_views = skills::state(&self.store)?;
        let targets = skills::Targets::from_env()?;
        // 检测列表只排除启用中的投影；停用目标上的目录（可能是用户
        // 自己的副本）照常展示。OMP 与 Codex 共用 ~/.agents/skills。
        let managed_paths: Vec<PathBuf> = skill_views
            .iter()
            .flat_map(|view| {
                let mut dirs = Vec::new();
                if view.omp || view.codex {
                    dirs.push(targets.skill_dir(skills::SkillTarget::Omp, &view.name));
                }
                if view.claude {
                    dirs.push(targets.skill_dir(skills::SkillTarget::Claude, &view.name));
                }
                dirs
            })
            .collect();
        let detected = workspace::scan_skills(&root)?
            .into_iter()
            .filter(|skill| !managed_paths.contains(&PathBuf::from(&skill.path)))
            .collect();
        Ok(State {
            workspace: workspace::from_root(&root),
            agent: AgentInfo { installed, version },
            installed_agents: [
                (Agent::Omp, installed),
                (Agent::Claude, claude.installed),
                (Agent::Codex, codex.installed),
            ]
            .into_iter()
            .filter(|&(_, present)| present)
            .map(|(agent, _)| agent)
            .collect(),
            mcp: mcp::state(&self.store)?,
            skills: skill_views,
            detected,
            repositories: self.store.repositories()?,
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
        name: String,
        mut config: Option<Value>,
        agents: Vec<Agent>,
    ) -> Result<Plan> {
        if !platform::valid_mcp_name(&name) {
            return Err("MCP 名称无效（最多 100 字符，仅限字母数字、_-. :）".into());
        }
        let targets = mcp::Targets::from_env()?;
        let action = match config.as_mut() {
            Some(spec) => {
                if spec.get("mcpServers").is_some() {
                    return Err(
                        "配置应为单个 MCP 服务对象（{\"type\":...,\"command\":...}），不应包含 mcpServers 包装"
                            .into(),
                    );
                }
                mcp::strip_enabled(spec);
                mcp::validate_spec(spec)?;
                // 密钥回填必须在入队前作用于 record 本身：apply 会把
                // record 原样存入数据库，占位符一旦入库，之后的开关切换
                // 就会把 "[已隐藏]" 当真值投影进 Agent 文件。
                // 回填来源只用统一库的旧 spec——它是唯一事实来源：Agent
                // 文件可能损坏（坏 JSON 不应阻断其它 Agent 的保存）或被
                // 手改漂移（文件值不得反向覆盖库值）。
                let previous = self
                    .store
                    .mcp_server(&name)?
                    .filter(|record| record.managed);
                platform::restore_mcp_secrets(
                    spec,
                    previous.as_ref().map(|record| &record.spec),
                )?;
                let mut record = McpRecord::new(name.clone(), spec.clone());
                for agent in agents {
                    record.set_enabled(agent, true);
                }
                McpAction::Save { record }
            }
            None => McpAction::Delete { name },
        };
        self.enqueue_mcp(action, &targets)
    }

    pub fn plan_mcp_toggle(
        &mut self,
        name: String,
        agent: Agent,
        enabled: bool,
    ) -> Result<Plan> {
        let targets = mcp::Targets::from_env()?;
        self.enqueue_mcp(
            McpAction::Toggle {
                name,
                agent,
                enabled,
            },
            &targets,
        )
    }

    fn enqueue_mcp(&mut self, action: McpAction, targets: &mcp::Targets) -> Result<Plan> {
        let (writes, warnings) = mcp::project(&self.store, targets, &action)?;
        let summary = match &action {
            McpAction::Save { record } => {
                let existing = self.store.mcp_server(&record.name)?.is_some();
                let names: Vec<&str> = mcp::AGENTS
                    .into_iter()
                    .filter(|agent| record.enabled_for(*agent))
                    .map(Agent::title)
                    .collect();
                format!(
                    "{} MCP {} → {}",
                    if existing { "更新" } else { "新增" },
                    record.name,
                    if names.is_empty() {
                        "未选择 Agent（仅保存）".to_owned()
                    } else {
                        names.join("、")
                    }
                )
            }
            McpAction::Delete { name } => format!("移除 MCP {name}"),
            McpAction::Toggle {
                name,
                agent,
                enabled,
            } => format!(
                "在 {} 中{} {name}",
                agent.title(),
                if *enabled { "启用" } else { "停用" }
            ),
        };
        let changes = writes
            .iter()
            .map(|write| Change {
                path: write.path.display().to_string(),
                before: write
                    .before
                    .as_deref()
                    .map_or_else(String::new, |bytes| preview_mcp(&write.path, bytes)),
                after: preview_mcp(&write.path, &write.after),
            })
            .collect();
        Ok(self.enqueue(
            summary,
            changes,
            warnings,
            Pending::Mcp(Box::new(McpPending { writes, action })),
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
                "已更新仓库缓存至 {}；技能列表将显示可用更新",
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
            format!("已刷新 {} 个仓库", refreshed)
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
    /// 校验仓库内的技能来源，返回 (name, files, hash, commit)。
    fn validated_skill_source(
        &self,
        repository_id: i64,
        skill_path: &str,
    ) -> Result<ValidatedSource> {
        let repo_root = self.store.repo_dir(repository_id);
        let relative = Path::new(skill_path);
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
        Ok((name, files, hash, commit))
    }

    /// 分发安装/更新：刷新数据目录中的中央副本，并把启用中的目标重新
    /// 投影。新装默认同时启用两个用户级目标；同名记录若来自其他来源
    /// 则拒绝覆盖。
    pub fn plan_skill(&mut self, repository_id: i64, skill_path: String) -> Result<Plan> {
        let (name, files, hash, commit) = self.validated_skill_source(repository_id, &skill_path)?;
        let previous = self.store.skill(&name)?;
        if let Some(previous) = &previous {
            if previous.repository_id != repository_id || previous.skill_path != skill_path {
                return Err("同名技能已存在且来自其他来源，拒绝覆盖".into());
            }
        }
        let record = SkillRecord {
            id: previous.as_ref().map_or(0, |record| record.id),
            repository_id,
            skill_path: skill_path.clone(),
            name: name.clone(),
            commit,
            hash,
            omp: previous.as_ref().is_none_or(|record| record.omp),
            codex: previous.as_ref().is_none_or(|record| record.codex),
            claude: previous.as_ref().is_none_or(|record| record.claude),
        };
        let summary = if previous.is_some() {
            format!("更新技能 {name} 并同步已启用的投影目录")
        } else {
            format!("安装技能 {name} → 通用目录 (OMP/Codex) + Claude Code")
        };
        let source = self.store.repo_dir(repository_id).join(&skill_path);
        let action = skills::SkillAction::Save {
            record,
            files,
            source,
        };
        let targets = skills::Targets::from_env()?;
        let (writes, config_writes, warnings) = skills::project(&self.store, &targets, &action)?;
        let changes = writes
            .iter()
            .flat_map(dir_changes)
            .chain(config_writes.iter().map(config_change))
            .collect();
        Ok(self.enqueue(
            summary,
            changes,
            warnings,
            Pending::Skills(Box::new(SkillsPending {
                writes,
                config_writes,
                action,
            })),
        ))
    }

    pub fn plan_skill_toggle(
        &mut self,
        name: String,
        target: SkillTarget,
        enabled: bool,
    ) -> Result<Plan> {
        if !platform::valid_skill_name(&name) {
            return Err("技能名称无效".into());
        }
        let action = skills::SkillAction::Toggle {
            name: name.clone(),
            target,
            enabled,
        };
        let targets = skills::Targets::from_env()?;
        let (writes, config_writes, warnings) = skills::project(&self.store, &targets, &action)?;
        let summary = format!(
            "在 {} 中{} {name}",
            target.title(),
            if enabled { "启用" } else { "停用" }
        );
        let changes = writes
            .iter()
            .flat_map(dir_changes)
            .chain(config_writes.iter().map(config_change))
            .collect();
        Ok(self.enqueue(
            summary,
            changes,
            warnings,
            Pending::Skills(Box::new(SkillsPending {
                writes,
                config_writes,
                action,
            })),
        ))
    }

    pub fn plan_skill_remove(&mut self, name: String) -> Result<Plan> {
        if !platform::valid_skill_name(&name) {
            return Err("技能名称无效".into());
        }
        let action = skills::SkillAction::Remove { name: name.clone() };
        let targets = skills::Targets::from_env()?;
        let (writes, config_writes, warnings) = skills::project(&self.store, &targets, &action)?;
        let summary = format!("移除技能 {name}（删除所有投影目录与中央副本）");
        let changes = writes
            .iter()
            .flat_map(dir_changes)
            .chain(config_writes.iter().map(config_change))
            .collect();
        Ok(self.enqueue(
            summary,
            changes,
            warnings,
            Pending::Skills(Box::new(SkillsPending {
                writes,
                config_writes,
                action,
            })),
        ))
    }

    pub fn apply(&mut self, id: String) -> Result<Message> {
        let pending = self.plans.remove(&id).ok_or("方案已失效，请重新预览")?;
        match pending {
            Pending::Mcp(pending) => {
                let McpPending { writes, action } = *pending;
                apply_mcp(&self.store.root, &id, &writes)?;
                let message = match action {
                    McpAction::Save { record } => {
                        let name = record.name.clone();
                        self.store.save_mcp_server(&record)?;
                        format!("MCP {name} 已保存并写入所选 Agent 的用户级配置")
                    }
                    McpAction::Delete { name } => {
                        self.store.delete_mcp_server(&name)?;
                        format!("MCP {name} 已从数据库与各 Agent 配置中移除")
                    }
                    McpAction::Toggle {
                        name,
                        agent,
                        enabled,
                    } => {
                        let mut record = self
                            .store
                            .mcp_server(&name)?
                            .ok_or("方案已失效，请重新预览")?;
                        record.set_enabled(agent, enabled);
                        self.store.save_mcp_server(&record)?;
                        format!("已在 {} 中{} {name}", agent.title(), if enabled { "启用" } else { "停用" })
                    }
                };
                Ok(Message { message })
            }
            Pending::Skills(pending) => {
                let SkillsPending {
                    writes,
                    config_writes,
                    action,
                } = *pending;
                if let skills::SkillAction::Save {
                    record,
                    source,
                    ..
                } = &action
                {
                    if self.store.commit(record.repository_id)? != record.commit
                        || platform::tree_hash(&platform::source_snapshot(
                            source,
                            &self.store.repo_dir(record.repository_id),
                        )?) != record.hash
                    {
                        return Err("仓库提交或技能文件自预览后已变化".into());
                    }
                }
                let (backup_dir, undo_steps) =
                    apply_skill_writes(&self.store.root, &id, &writes)?;
                apply_config_writes(&id, &config_writes)?;
                let result = match &action {
                    skills::SkillAction::Save { record, .. } => self
                        .store
                        .save_skill(record)
                        .map(|_| format!("技能 {} 已保存并同步到启用的投影目录", record.name)),
                    skills::SkillAction::Toggle {
                        name,
                        target,
                        enabled,
                    } => {
                        let mut stored =
                            self.store.skill(name)?.ok_or("方案已失效，请重新预览")?;
                        match target {
                            SkillTarget::Omp => stored.omp = *enabled,
                            SkillTarget::Codex => stored.codex = *enabled,
                            SkillTarget::Claude => stored.claude = *enabled,
                        }
                        self.store.save_skill(&stored).map(|_| {
                            format!(
                                "已在 {} 中{} {name}",
                                target.title(),
                                if *enabled { "启用" } else { "停用" }
                            )
                        })
                    }
                    skills::SkillAction::Remove { name } => self
                        .store
                        .delete_skill(name)
                        .map(|_| format!("技能 {name} 已从投影目录与 AMC 移除")),
                };
                match result {
                    Ok(message) => {
                        let _ = fs::remove_dir_all(&backup_dir);
                        Ok(Message { message })
                    }
                    Err(error) => {
                        undo_config_writes(&config_writes);
                        if let Err(recovery) = undo_skill_writes(&backup_dir, &undo_steps) {
                            Err(format!("{error}；{recovery}"))
                        } else {
                            Err(error)
                        }
                    }
                }
            }
        }
    }

}

/// 预览一个目录级写入：对比 before/after 快照生成变更列表。
fn dir_changes(write: &SkillWrite) -> Vec<Change> {
    let before = write.before.clone().unwrap_or_default();
    let after = write.after.clone().unwrap_or_default();
    diff_files(&write.path, &before, &after)
}

/// 预览一个配置文件写入：before 为空表示文件将新建。Codex config.toml
/// 与 MCP 共用一份文件，内含 env/http_headers/bearer_token 等凭据，
/// 预览必须走整份脱敏（preview_redacted）。
fn config_change(write: &skills::ConfigWrite) -> Change {
    let render = |bytes: &[u8]| -> String {
        if write.path.extension().is_some_and(|e| e == "toml") {
            mcp::codex::preview_redacted(bytes)
        } else {
            preview(bytes)
        }
    };
    Change {
        path: write.path.display().to_string(),
        before: write
            .before
            .as_deref()
            .map_or_else(|| "（新建）".to_owned(), |bytes| render(bytes)),
        after: render(&write.after),
    }
}

/// 配置文件写入（OMP config.yml / Codex config.toml）：先全量校验
/// before 防漂移，再以临时文件 + rename 原子替换。回滚所需旧字节保留在
/// ConfigWrite.before 中（见 undo_config_writes），无需磁盘备份。
fn apply_config_writes(id: &str, writes: &[skills::ConfigWrite]) -> Result<()> {
    for write in writes {
        let actual = match fs::read(&write.path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("读取 {} 失败: {e}", write.path.display())),
        };
        if actual != write.before {
            return Err(format!(
                "{} 自预览后已变化，请重新预览",
                write.path.display()
            ));
        }
    }
    for (index, write) in writes.iter().enumerate() {
        let parent = write.path.parent().ok_or("配置文件路径无效")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let temp = parent.join(format!(".amc-cfg-{id}-{index}.tmp"));
        write_private(&temp, &write.after)?;
        fs::rename(&temp, &write.path).map_err(|error| {
            let _ = fs::remove_file(&temp);
            format!("写入 {} 失败: {error}", write.path.display())
        })?;
    }
    Ok(())
}

/// 回滚配置写入：还原旧字节；新建的配置文件直接删除。
fn undo_config_writes(writes: &[skills::ConfigWrite]) {
    for write in writes.iter().rev() {
        match &write.before {
            Some(bytes) => {
                let _ = fs::write(&write.path, bytes);
            }
            None => {
                let _ = fs::remove_file(&write.path);
            }
        }
    }
}

struct SkillUndo {
    had_before: bool,
    backup: PathBuf,
    target: PathBuf,
}

/// 回滚已执行的目录写入：被替换/删除的目录从备份还原，新出现的删除。
fn undo_skill_writes(backup_dir: &Path, done: &[SkillUndo]) -> Result<()> {
    let mut failures = Vec::new();
    for step in done.iter().rev() {
        if step.target.exists() {
            if let Err(error) = fs::remove_dir_all(&step.target) {
                failures.push(format!("无法移除 {}: {error}", step.target.display()));
                continue;
            }
        }
        if step.had_before {
            if let Err(error) = fs::rename(&step.backup, &step.target) {
                failures.push(format!(
                    "恢复失败，旧副本保留在 {}: {error}",
                    step.backup.display()
                ));
            }
        }
    }
    // 只有全部恢复成功才清理备份目录；任何失败都保留现场并报告路径。
    if failures.is_empty() {
        let _ = fs::remove_dir_all(backup_dir);
        Ok(())
    } else {
        Err(format!(
            "回滚未完全成功，备份保留在 {}：{}",
            backup_dir.display(),
            failures.join("；")
        ))
    }
}

/// 目录级分发写入：先全量校验 before 防漂移，再逐个换入；每个被替换或
/// 删除的目录先 rename 进备份目录，任一步失败立即回滚已执行步骤。
/// 返回备份目录与回滚账目，供调用方在数据库写入失败时回滚、成功后清理。
fn apply_skill_writes(
    data_dir: &Path,
    id: &str,
    writes: &[SkillWrite],
) -> Result<(PathBuf, Vec<SkillUndo>)> {
    for write in writes {
        let parent = write.path.parent().ok_or("技能目标路径无效")?;
        let exists = write.path.exists();
        match (&write.before, exists) {
            (None, true) => {
                return Err(format!("{} 自预览后已出现，请重新预览", write.path.display()))
            }
            (Some(_), false) => {
                return Err(format!("{} 自预览后已消失，请重新预览", write.path.display()))
            }
            (Some(before), true) => {
                platform::no_links(&write.path, parent)?;
                let actual = platform::snapshot(&write.path)?;
                if platform::tree_hash(&actual) != platform::tree_hash(before) {
                    return Err("技能目录自预览后已变化，请重新预览".into());
                }
            }
            (None, false) => {}
        }
    }
    let backup_dir = data_dir.join("backups/skills").join(id);
    fs::create_dir_all(&backup_dir).map_err(|e| e.to_string())?;
    let mut done: Vec<SkillUndo> = Vec::new();
    // 所有写入后错误都汇入同一个出口：先回滚 done（含当前失败步骤），
    // 恢复失败时保留备份并把路径并入错误信息。
    let failure = (|| -> Result<()> {
        for (index, write) in writes.iter().enumerate() {
            let backup = backup_dir.join(index.to_string());
            let had_before = write.path.exists();
            if had_before {
                fs::rename(&write.path, &backup)
                    .map_err(|error| format!("备份旧技能目录失败: {error}"))?;
            }
            let placed = (|| -> Result<()> {
                if let Some(after) = &write.after {
                    let parent = write.path.parent().ok_or("技能目标路径无效")?;
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("创建目录 {} 失败: {e}", parent.display()))?;
                    let stage = backup_dir.join(format!("stage-{index}"));
                    fs::create_dir(&stage).map_err(|e| e.to_string())?;
                    let source = write.source.as_deref().ok_or("技能来源缺失")?;
                    if let Err(error) = stage_files(&stage, after, source) {
                        let _ = fs::remove_dir_all(&stage);
                        return Err(error);
                    }
                    fs::rename(&stage, &write.path)
                        .map_err(|error| format!("写入技能目录失败: {error}"))?;
                }
                Ok(())
            })();
            done.push(SkillUndo {
                had_before,
                backup,
                target: write.path.clone(),
            });
            placed?;
        }
        Ok(())
    })();
    if let Err(error) = failure {
        return match undo_skill_writes(&backup_dir, &done) {
            Ok(()) => Err(error),
            Err(recovery) => Err(format!("{error}；{recovery}")),
        };
    }
    Ok((backup_dir, done))
}

fn apply_mcp(data_dir: &Path, id: &str, writes: &[McpWrite]) -> Result<()> {
    for write in writes {
        let parent = write.path.parent().ok_or("MCP 目标路径无效")?;
        platform::no_links(&write.path, parent)?;
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
    let mut staged = Vec::new();
    for (index, write) in writes.iter().enumerate() {
        let parent = write.path.parent().ok_or("MCP 目标路径无效")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let temp = parent.join(format!(".amc-{id}-{index}.tmp"));
        let stage_result = (|| -> Result<()> {
            if let Some(old) = &write.before {
                write_private(&backup_dir.join(format!("{id}-{index}.bak")), old)?;
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
        let parent = write.path.parent().ok_or("MCP 目标路径无效")?;
        let old_path = parent.join(format!(".amc-{id}-{index}.old"));
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
            let parent = write.path.parent().ok_or("MCP 目标路径无效")?;
            let restored = if write.before.is_some() {
                fs::remove_file(&write.path).and_then(|_| {
                    fs::rename(parent.join(format!(".amc-{id}-{index}.old")), &write.path)
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
            let parent = write.path.parent().ok_or("MCP 目标路径无效")?;
            let _ = fs::remove_file(parent.join(format!(".amc-{id}-{index}.old")));
        }
    }
    Ok(())
}

fn preview_mcp(path: &Path, data: &[u8]) -> String {
    if data.len() > 128 * 1024 {
        return "[MCP 配置超出预览大小；敏感字段未显示]".into();
    }
    if path.extension().is_some_and(|extension| extension == "toml") {
        return mcp::codex::preview_redacted(data);
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
