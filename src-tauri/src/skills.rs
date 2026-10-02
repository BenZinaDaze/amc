// Skills 分发管理（cc-switch 模型）：amc.sqlite3 的 skills 表 + 数据目录
// skills/<name>/ 中央副本是唯一事实来源，各 Agent 的用户级技能目录只是投影。
// 物理目录只有两个：~/.agents/skills（OMP 的 agents 来源与 Codex 的 USER 级
// 共用，双方默认加载）与 ~/.claude/skills（Claude Code 个人级）。
// OMP 与 Codex 的独立开关不靠目录，而靠各自配置文件里的禁用项：
// OMP → config.yml 的 skills.ignoredSkills（按技能名 glob）；
// Codex → config.toml 的 [[skills.config]]（按 SKILL.md 绝对路径
// enabled=false）。两个开关都关时共享目录才被移除。
// 归属规则：只有「当前启用中的投影」（flag ON 且哈希与记录一致）可被
// 覆盖/删除；任何非 AMC 投影的同名目录（含内容相同的用户副本）一律拒绝。

use crate::{
    platform::{self, Result},
    store::{SkillRecord, Store},
};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillTarget {
    /// `~/.agents/skills` 中 OMP 的视角；停用 = OMP config.yml 忽略该名称。
    Omp,
    /// `~/.agents/skills` 中 Codex 的视角；停用 = config.toml
    /// [[skills.config]] 按路径 enabled=false。
    Codex,
    /// `~/.claude/skills`：Claude Code 个人级技能目录（目录即开关）。
    Claude,
}

pub const SKILL_TARGETS: [SkillTarget; 3] = [SkillTarget::Omp, SkillTarget::Codex, SkillTarget::Claude];

impl SkillTarget {
    pub fn title(self) -> &'static str {
        match self {
            SkillTarget::Omp => "OMP",
            SkillTarget::Codex => "Codex",
            SkillTarget::Claude => "Claude Code",
        }
    }

    /// 该目标的技能目录；OMP 与 Codex 共用 ~/.agents/skills。
    pub fn dir(self, home: &std::path::Path) -> PathBuf {
        match self {
            SkillTarget::Omp | SkillTarget::Codex => home.join(".agents/skills"),
            SkillTarget::Claude => home.join(".claude/skills"),
        }
    }

    fn flag(self, record: &SkillRecord) -> bool {
        match self {
            SkillTarget::Omp => record.omp,
            SkillTarget::Codex => record.codex,
            SkillTarget::Claude => record.claude,
        }
    }
}

/// 各 Agent 用户级技能目录的定位结果。
pub(crate) struct Targets {
    home: PathBuf,
}

impl Targets {
    pub fn from_env() -> Result<Self> {
        let home = platform::home()?;
        if !home.is_dir() {
            return Err("用户目录无效，无法定位技能投影目录".into());
        }
        Ok(Self { home })
    }

    pub fn skill_dir(&self, target: SkillTarget, name: &str) -> PathBuf {
        target.dir(&self.home).join(name)
    }
}

/// 中央副本目录：数据目录 skills/<name>/。
pub fn central_dir(store: &Store, name: &str) -> PathBuf {
    store.root.join("skills").join(name)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SkillView {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub repository_id: i64,
    pub skill_path: String,
    pub commit: String,
    pub update_available: bool,
    pub omp: bool,
    pub codex: bool,
    pub claude: bool,
}

/// 目录级投影写入：`after` 为 None 表示删除目录；`before` 用于预览
/// 与 apply 时防漂移校验（None = 目录必须不存在）；`source` 是派生可执行
/// 位的参照目录（仅复制时需要）。
pub(crate) struct SkillWrite {
    pub path: PathBuf,
    pub before: Option<FileSnapshot>,
    pub after: Option<FileSnapshot>,
    pub source: Option<PathBuf>,
}

/// 配置文件级投影写入（OMP config.yml / Codex config.toml）：
/// `before` 用于防漂移与回滚（None = 文件必须不存在）。
pub(crate) struct ConfigWrite {
    pub path: PathBuf,
    pub before: Option<Vec<u8>>,
    pub after: Vec<u8>,
}

pub(crate) enum SkillAction {
    /// 新增或更新：刷新中央副本，并把启用中的目标重新投影。
    Save {
        record: SkillRecord,
        files: Vec<(PathBuf, Vec<u8>)>,
        source: PathBuf,
    },
    Toggle {
        name: String,
        target: SkillTarget,
        enabled: bool,
    },
    /// 移除：删除所有投影与中央副本，随后删记录。
    Remove { name: String },
}

/// 目录快照：相对路径 → 文件内容。
type FileSnapshot = Vec<(PathBuf, Vec<u8>)>;

fn dir_state(dir: &std::path::Path) -> Result<Option<FileSnapshot>> {
    if !dir.exists() {
        return Ok(None);
    }
    let parent = dir.parent().ok_or("技能目录路径无效")?;
    platform::no_links(dir, parent)?;
    Ok(Some(platform::snapshot(dir)?))
}

/// 中央副本与记录的一致性：存在时必须与记录哈希一致，记录在而副本丢失
/// 视为损坏。返回当前中央副本文件（供复制投影使用）。
fn checked_central(store: &Store, record: &SkillRecord) -> Result<Option<FileSnapshot>> {
    let central = central_dir(store, &record.name);
    let Some(files) = dir_state(&central)? else {
        return Err("AMC 中央副本丢失，请重新安装该技能".into());
    };
    if platform::tree_hash(&files) != record.hash {
        return Err("AMC 中央副本与记录不一致，请重新安装该技能".into());
    }
    Ok(Some(files))
}

/// 目标目录归属校验。`owned` 表示该目标是 AMC 当前启用中的投影：仅当
/// 内容哈希与受管记录一致时才允许覆盖/删除。`owned=false` 时任何已存在
/// 目录都视为用户数据，一律冲突拒绝——内容相等不能证明归属。返回目录
/// 当前内容（None = 不存在）。
fn checked_projection(
    dir: &std::path::Path,
    owned: bool,
    accepted: &[&str],
) -> Result<Option<FileSnapshot>> {
    let Some(files) = dir_state(dir)? else {
        return Ok(None);
    };
    if !owned {
        return Err("目标已存在同名技能目录，且不属于 AMC 当前启用的投影，拒绝覆盖；请先手动移除".into());
    }
    let hash = platform::tree_hash(&files);
    if accepted.contains(&hash.as_str()) {
        return Ok(Some(files));
    }
    Err("技能目录已被手动修改，拒绝覆盖；请先备份或恢复原文件".into())
}

/// 把一条动作投影成待写入的目录与配置文件。数据库动作由调用方在文件
/// 写成功后执行。
pub(crate) fn project(
    store: &Store,
    targets: &Targets,
    action: &SkillAction,
) -> Result<(Vec<SkillWrite>, Vec<ConfigWrite>, Vec<String>)> {
    match action {
        SkillAction::Save {
            record,
            files,
            source,
        } => {
            let mut dirs = Vec::new();
            let mut configs = Vec::new();
            // 中央副本：新装时必须不存在；更新时必须与旧记录一致。
            let central = central_dir(store, &record.name);
            let central_before = dir_state(&central)?;
            let previous = store.skill(&record.name)?;
            match (&central_before, &previous) {
                (Some(before), Some(previous)) => {
                    if platform::tree_hash(before) != previous.hash {
                        return Err("AMC 中央副本与记录不一致，请重新安装该技能".into());
                    }
                }
                (Some(_), None) => {
                    return Err("数据目录已有无记录的同名技能副本，拒绝覆盖".into());
                }
                (None, Some(_)) => {
                    return Err("AMC 中央副本丢失，请重新安装该技能".into());
                }
                (None, None) => {}
            }
            dirs.push(SkillWrite {
                path: central.clone(),
                before: central_before,
                after: Some(files.clone()),
                source: Some(source.clone()),
            });
            // 投影：只写启用中的目标；内容已一致时跳过。旧投影（旧哈希）
            // 视为本主所有，直接替换为新内容；新装时目标若已存在（即便
            // 内容相同）一律冲突，避免静默接管用户目录。
            let new_hash = platform::tree_hash(files);
            for target in SKILL_TARGETS {
                if !target.flag(record) {
                    continue;
                }
                let owned = previous.is_some();
                let previous_hash = previous
                    .as_ref()
                    .map(|record| record.hash.as_str())
                    .unwrap_or_default();
                let dir = targets.skill_dir(target, &record.name);
                let before = checked_projection(&dir, owned, &[previous_hash, new_hash.as_str()])?;
                let Some(before) = before else {
                    dirs.push(SkillWrite {
                        path: dir,
                        before: None,
                        after: Some(files.clone()),
                        source: Some(central.clone()),
                    });
                    continue;
                };
                if platform::tree_hash(&before) == new_hash {
                    continue;
                }
                dirs.push(SkillWrite {
                    path: dir,
                    before: Some(before),
                    after: Some(files.clone()),
                    source: Some(central.clone()),
                });
            }
            // 启用中的目标不得残留禁用配置（防御：上次开关中断的残页）。
            if record.omp {
                configs.extend(omp_ignore_write(targets, &record.name, false)?);
            }
            if record.codex {
                let dir = targets.skill_dir(SkillTarget::Codex, &record.name);
                configs.extend(codex_disable_write(&dir, false)?);
            }
            Ok((dirs, configs, Vec::new()))
        }
        SkillAction::Toggle {
            name,
            target,
            enabled,
        } => {
            let record = store
                .skill(name)?
                .ok_or("技能记录不存在，请刷新后重试")?;
            if target.flag(&record) == *enabled {
                return Err(if *enabled {
                    "该目标本就已启用"
                } else {
                    "该目标本就未启用"
                }
                .into());
            }
            let central_files = checked_central(store, &record)?;
            let mut dirs = Vec::new();
            let mut configs = Vec::new();
            let mut warnings = Vec::new();
            match target {
                SkillTarget::Claude => {
                    let dir = targets.skill_dir(SkillTarget::Claude, name);
                    // owned 取当前开关（合法 toggle 必为 !enabled）：启用时
                    // 目录已存在即冲突；停用时目录是本主投影，校验后删除。
                    let before = checked_projection(&dir, !*enabled, &[record.hash.as_str()])?;
                    if *enabled {
                        // 目录不存在（owned=false 时存在会已报冲突）→ 复制。
                        let files = central_files.ok_or("AMC 中央副本丢失，请重新安装该技能")?;
                        dirs.push(SkillWrite {
                            path: dir,
                            before,
                            after: Some(files),
                            source: Some(central_dir(store, name)),
                        });
                    } else {
                        let Some(before) = before else {
                            // 目录已不存在：只补数据库开关。
                            return Ok((dirs, configs, Vec::new()));
                        };
                        dirs.push(SkillWrite {
                            path: dir,
                            before: Some(before),
                            after: None,
                            source: None,
                        });
                    }
                }
                SkillTarget::Omp | SkillTarget::Codex => {
                    // 共享目录：另一个伙伴启用时目录保留且归对方视角所有，
                    // 只写本 Agent 的禁用配置；两者都关时目录移除（并清
                    // 残留配置）。
                    let other = if *target == SkillTarget::Omp {
                        SkillTarget::Codex
                    } else {
                        SkillTarget::Omp
                    };
                    let other_on = other.flag(&record);
                    let dir = targets.skill_dir(*target, name);
                    let accepted = [record.hash.as_str()];
                    match (*enabled, other_on) {
                        (true, _) => {
                            // 启用：目录缺失则复制；已存在时校验归属
                            // （对方启用 → 对方视角所有，内容一致即可）。
                            let before =
                                checked_projection(&dir, other_on, &accepted)?;
                            if before.is_none() {
                                let files = central_files
                                    .ok_or("AMC 中央副本丢失，请重新安装该技能")?;
                                dirs.push(SkillWrite {
                                    path: dir.clone(),
                                    before: None,
                                    after: Some(files),
                                    source: Some(central_dir(store, name)),
                                });
                            }
                            // 启用 → 清掉本 Agent 的禁用项。
                            configs.extend(match *target {
                                SkillTarget::Omp => omp_ignore_write(targets, name, false)?,
                                SkillTarget::Codex => codex_disable_write(&dir, false)?,
                                _ => Vec::new(),
                            });
                            if !other_on {
                                // 双关 → 单开：共享目录恢复后对方也会加载，
                                // 必须同时写对方的禁用项维持隔离。
                                configs.extend(match other {
                                    SkillTarget::Omp => omp_ignore_write(targets, name, true)?,
                                    SkillTarget::Codex => codex_disable_write(&dir, true)?,
                                    _ => Vec::new(),
                                });
                                warnings.push(format!(
                                    "共享目录已恢复；{} 仍停用，已在其配置中保留禁用项",
                                    other.title()
                                ));
                            }
                        }
                        (false, true) => {
                            // 停用但对方仍启用：目录不动，仅写禁用项。
                            configs.extend(match *target {
                                SkillTarget::Omp => omp_ignore_write(targets, name, true)?,
                                SkillTarget::Codex => codex_disable_write(&dir, true)?,
                                _ => Vec::new(),
                            });
                            warnings.push(format!(
                                "共享目录保留（{} 仍启用）；{} 改由其配置文件禁用",
                                other.title(),
                                target.title()
                            ));
                        }
                        (false, false) => {
                            // 两个都关：移除目录（若存在且归属本主）。
                            let Some(before) = checked_projection(&dir, true, &accepted)? else {
                                configs.extend(self::cleanup_configs(targets, name));
                                return Ok((dirs, configs, Vec::new()));
                            };
                            dirs.push(SkillWrite {
                                path: dir.clone(),
                                before: Some(before),
                                after: None,
                                source: None,
                            });
                            configs.extend(cleanup_configs(targets, name));
                        }
                    }
                }
            }
            Ok((dirs, configs, warnings))
        }
        SkillAction::Remove { name } => {
            let record = store
                .skill(name)?
                .ok_or("技能记录不存在，请刷新后重试")?;
            let central_files = checked_central(store, &record)?;
            let mut dirs = Vec::new();
            // 只清理启用中的投影；停用目标上的目录即便内容相同也不是 AMC 的，
            // 可能是用户放回的副本，绝不触碰。
            for target in SKILL_TARGETS {
                if !target.flag(&record) {
                    continue;
                }
                let dir = targets.skill_dir(target, name);
                let Some(before) = checked_projection(&dir, true, &[record.hash.as_str()])? else {
                    continue;
                };
                dirs.push(SkillWrite {
                    path: dir,
                    before: Some(before),
                    after: None,
                    source: None,
                });
            }
            dirs.push(SkillWrite {
                path: central_dir(store, name),
                before: central_files,
                after: None,
                source: None,
            });
            let configs = cleanup_configs(targets, name);
            Ok((dirs, configs, Vec::new()))
        }
    }
}

/// 移除/双关后清理两份配置里的残留禁用项。
fn cleanup_configs(targets: &Targets, name: &str) -> Vec<ConfigWrite> {
    let mut configs = omp_ignore_write(targets, name, false).unwrap_or_default();
    let dir = targets.skill_dir(SkillTarget::Codex, name);
    configs.extend(codex_disable_write(&dir, false).unwrap_or_default());
    configs
}

/// OMP config.yml 的 skills.ignoredSkills：`ignore=true` 追加技能名，
/// `ignore=false` 移除该名称的全部条目；无变化返回 None。
fn omp_ignore_write(
    targets: &Targets,
    name: &str,
    ignore: bool,
) -> Result<Vec<ConfigWrite>> {
    let path = targets.omp_config_path()?;
    let before = fs::read(&path).ok();
    let mut doc: serde_yaml::Value = match &before {
        Some(bytes) => {
            serde_yaml::from_slice(bytes).map_err(|e| format!("解析 {} 失败: {e}", path.display()))?
        }
        None => serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
    };
    if !doc.is_mapping() {
        return Err(format!("{} 顶层必须是映射", path.display()));
    }
    let mapping = doc.as_mapping_mut().ok_or("YAML 顶层不是映射")?;
    let skills = mapping
        .entry(serde_yaml::Value::String("skills".into()))
        .or_insert(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    if !skills.is_mapping() {
        return Err(format!("{} 中 skills 不是映射", path.display()));
    }
    let skills = skills.as_mapping_mut().ok_or("YAML skills 不是映射")?;
    let ignored = skills
        .entry(serde_yaml::Value::String("ignoredSkills".into()))
        .or_insert(serde_yaml::Value::Sequence(Vec::new()));
    if !ignored.is_sequence() {
        return Err(format!("{} 中 skills.ignoredSkills 不是列表", path.display()));
    }
    let list = ignored.as_sequence_mut().ok_or("ignoredSkills 不是列表")?;
    let name_value = serde_yaml::Value::String(name.into());
    let mut changed = false;
    if ignore {
        let present = list.iter().any(|item| *item == name_value);
        if !present {
            list.push(name_value);
            changed = true;
        }
    } else {
        let before_len = list.len();
        list.retain(|item| *item != name_value);
        changed = list.len() != before_len;
    }
    if !changed {
        return Ok(Vec::new());
    }
    let after = serde_yaml::to_string(&doc).map_err(|e| format!("序列化 {} 失败: {e}", path.display()))?;
    Ok(vec![ConfigWrite {
        path,
        before,
        after: after.into_bytes(),
    }])
}

/// Codex config.toml 的 [[skills.config]]：`disable=true` 追加
/// { path = <SKILL.md 绝对路径>, enabled = false }，`disable=false` 移除
/// 该路径的全部条目；无变化返回 None。
fn codex_disable_write(skill_dir: &std::path::Path, disable: bool) -> Result<Vec<ConfigWrite>> {
    let path = codex_config_path()?;
    let before = fs::read(&path).ok();
    let mut doc: toml_edit::DocumentMut = match &before {
        Some(bytes) => std::str::from_utf8(bytes)
            .map_err(|e| format!("解析 {} 失败: 非 UTF-8 ({e})", path.display()))?
            .parse()
            .map_err(|e| format!("解析 {} 失败: {e}", path.display()))?,
        None => toml_edit::DocumentMut::new(),
    };
    let skill_md = skill_dir.join("SKILL.md");
    let path_str = skill_md
        .to_str()
        .ok_or("Codex 禁用路径必须是 UTF-8")?
        .to_owned();
    let skills_item = doc
        .as_table_mut()
        .entry("skills")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(skills_table) = skills_item.as_table_like_mut() else {
        return Err(format!("{} 中 [skills] 不是表", path.display()));
    };
    let mut changed = false;
    match disable {
        true => {
            let already = skills_table
                .get("config")
                .and_then(|item| item.as_array_of_tables())
                .is_some_and(|entries| {
                    entries
                        .iter()
                        .any(|entry| entry.get("path").and_then(|p| p.as_str()) == Some(path_str.as_str()))
                });
            if !already {
                let entry = {
                    let mut table = toml_edit::Table::new();
                    table.insert("path", toml_edit::value(path_str.as_str()));
                    table.insert("enabled", toml_edit::value(false));
                    table
                };
                match skills_table.entry("config") {
                    toml_edit::Entry::Vacant(vacant) => {
                        let mut array = toml_edit::ArrayOfTables::new();
                        array.push(entry);
                        vacant.insert(toml_edit::Item::ArrayOfTables(array));
                    }
                    toml_edit::Entry::Occupied(mut occupied) => {
                        if let Some(array) = occupied.get_mut().as_array_of_tables_mut() {
                            array.push(entry);
                        } else {
                            return Err(format!("{} 中 skills.config 不是 [[skills.config]] 数组", path.display()));
                        }
                    }
                }
                changed = true;
            }
        }
        false => {
            if let Some(array) = skills_table.get_mut("config").and_then(|i| i.as_array_of_tables_mut()) {
                let before_len = array.len();
                let kept: Vec<_> = array
                    .iter()
                    .filter(|entry| entry.get("path").and_then(|p| p.as_str()) != Some(path_str.as_str()))
                    .cloned()
                    .collect();
                array.clear();
                for entry in kept {
                    array.push(entry);
                }
                changed = array.len() != before_len;
            }
        }
    }
    if !changed {
        return Ok(Vec::new());
    }
    Ok(vec![ConfigWrite {
        path,
        before,
        after: doc.to_string().into_bytes(),
    }])
}

/// OMP 生效的设置文件：agent 目录（与 OMP MCP 配置同一套目录规则）下的
/// config.yml；config.yaml / settings.json 只是 OMP 自己的迁移源。
fn codex_config_path() -> Result<PathBuf> {
    let home = platform::home()?;
    Ok(std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
        .join("config.toml"))
}

impl Targets {
    fn omp_config_path(&self) -> Result<PathBuf> {
        let location = crate::usage::omp_mcp_location()?;
        Ok(location.config_root.join("config.yml"))
    }
}

/// 列表数据：中央副本提供描述，仓库缓存提供更新标记。
pub fn state(store: &Store) -> Result<Vec<SkillView>> {
    let mut views = Vec::new();
    for record in store.skills()? {
        let description = platform::skill_metadata(&central_dir(store, &record.name).join("SKILL.md"))
            .map(|(_, description)| description)
            .unwrap_or_default();
        let source = store.repo_dir(record.repository_id).join(&record.skill_path);
        let update_available = platform::source_snapshot(&source, &store.repo_dir(record.repository_id))
            .map(|files| !files.is_empty() && platform::tree_hash(&files) != record.hash)
            .unwrap_or(false);
        views.push(SkillView {
            id: record.id,
            description,
            name: record.name,
            repository_id: record.repository_id,
            skill_path: record.skill_path,
            commit: record.commit,
            update_available,
            omp: record.omp,
            codex: record.codex,
            claude: record.claude,
        });
    }
    Ok(views)
}

/// 一次性迁移：旧版工作区安装（installations 表）→ 分发模型。对每条
/// 活跃记录：目标内容哈希与记录一致时，先建立中央副本与 Claude 投影
/// （临时目录 + rename 原子落位），全部写成功后才提交 skills 记录
/// （omp=codex=true——`~/.agents/skills` 旧文件就地成为共享投影；
/// claude 仅在目标原本不存在时为 true，已存在的目录哪怕是内容相同的
/// 用户副本都不接管）。单条失败不阻断启动：该记录保留在旧表中，下次
/// 启动重试；全部处理成功才删除旧表。
pub fn migrate_legacy(store: &Store) -> Result<()> {
    let records = store.legacy_records()?;
    if records.is_empty() {
        store.drop_installations()?;
        return Ok(());
    }
    let targets = Targets::from_env()?;
    let mut failures = 0_usize;
    for record in &records {
        if store.skill(&record.name)?.is_some() {
            continue;
        }
        let target = std::path::Path::new(&record.target_path);
        let Ok(files) = platform::snapshot(target) else {
            failures += 1;
            continue;
        };
        if platform::tree_hash(&files) != record.hash {
            // 用户改过：不接管，旧文件保留为用户数据。
            continue;
        }
        let central = central_dir(store, &record.name);
        if !central.exists() && write_tree_atomic(&central, &files, target).is_err() {
            failures += 1;
            continue;
        }
        let claude_dir = targets.skill_dir(SkillTarget::Claude, &record.name);
        let claude = if claude_dir.exists() {
            false
        } else if write_tree_atomic(&claude_dir, &files, target).is_err() {
            failures += 1;
            continue;
        } else {
            true
        };
        if store
            .save_skill(&SkillRecord {
                id: 0,
                repository_id: record.repository_id,
                skill_path: record.skill_path.clone(),
                name: record.name.clone(),
                commit: record.commit.clone(),
                hash: record.hash.clone(),
                omp: true,
                codex: true,
                claude,
            })
            .is_err()
        {
            failures += 1;
        }
    }
    if failures == 0 {
        store.drop_installations()?;
    }
    Ok(())
}

/// 把快照写成一个目录树：先写进同卷临时目录再 rename 到目标，任何失败
/// 都不留下半份目录（可执行位参照源目录，规则与 stage_files 一致）。
fn write_tree_atomic(dest: &std::path::Path, files: &FileSnapshot, source: &std::path::Path) -> Result<()> {
    let parent = dest.parent().ok_or("技能目录路径无效")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = parent.join(format!(".amc-migrate-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        write_tree(&temp, files, source)?;
        fs::rename(&temp, dest).map_err(|e| format!("落位 {} 失败: {e}", dest.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temp);
    }
    result
}

/// 把快照写成一个目录树（可执行位参照源目录，规则与 stage_files 一致）。
fn write_tree(dest: &std::path::Path, files: &FileSnapshot, source: &std::path::Path) -> Result<()> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for (relative, bytes) in files {
        let path = dest.join(relative);
        fs::create_dir_all(path.parent().ok_or("非法技能路径")?).map_err(|e| e.to_string())?;
        fs::write(&path, bytes).map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for (relative, _) in files {
            if let Ok(metadata) = fs::metadata(source.join(relative)) {
                if metadata.permissions().mode() & 0o111 != 0 {
                    let _ = fs::set_permissions(
                        dest.join(relative),
                        fs::Permissions::from_mode(0o700),
                    );
                }
            }
        }
    }
    Ok(())
}
