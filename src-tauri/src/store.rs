use crate::{
    mcp::Agent,
    platform::{self, Result},
    workspace,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub id: i64,
    pub url: String,
    pub reference: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    pub id: i64,
    pub repository_id: i64,
    pub skill_path: String,
    pub name: String,
    pub target_path: String,
    pub commit: String,
    pub modified: bool,
    pub update_available: bool,
    pub active: bool,
    pub rollback_available: bool,
}

/// 统一 MCP 服务器记录：`spec` 是中立的 JSON 配置，三个布尔是各 Agent
/// 的启用开关（唯一事实来源在 DB，Agent 配置文件只是投影）。
/// `managed` 是 ownership 标记：只有经 AMC 显式保存的记录为 true；
/// 历史遗留（如旧版自动导入）无法证明来源，按未管理处理——不列出、
/// 不可切换、不可删除，也不接管。
#[derive(Clone, Serialize, PartialEq)]
pub struct McpRecord {
    pub name: String,
    pub spec: Value,
    pub omp: bool,
    pub claude: bool,
    pub codex: bool,
    pub managed: bool,
}

impl McpRecord {
    /// AMC 显式保存入口创建的记录：天然拥有 ownership。
    pub fn new(name: String, spec: Value) -> Self {
        Self {
            name,
            spec,
            omp: false,
            claude: false,
            codex: false,
            managed: true,
        }
    }

    pub fn enabled_for(&self, agent: Agent) -> bool {
        match agent {
            Agent::Omp => self.omp,
            Agent::Claude => self.claude,
            Agent::Codex => self.codex,
        }
    }

    pub fn set_enabled(&mut self, agent: Agent, enabled: bool) {
        match agent {
            Agent::Omp => self.omp = enabled,
            Agent::Claude => self.claude = enabled,
            Agent::Codex => self.codex = enabled,
        }
    }
}

#[derive(Clone)]
pub struct InstallRecord {
    pub id: i64,
    pub repository_id: i64,
    pub skill_path: String,
    pub name: String,
    pub target_path: String,
    pub commit: String,
    pub hash: String,
    pub rollback_path: Option<String>,
    pub rollback_commit: Option<String>,
    pub rollback_hash: Option<String>,
    pub active: bool,
}

pub struct Store {
    pub db: Connection,
    pub root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root).map_err(|e| format!("创建 AMC 数据目录失败: {e}"))?;
        let db = Connection::open(root.join("amc.sqlite3"))
            .map_err(|e| format!("打开 AMC 数据库失败: {e}"))?;
        db.execute_batch(
            "PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS repositories (
                id INTEGER PRIMARY KEY, url TEXT NOT NULL, git_ref TEXT NOT NULL,
                UNIQUE(url, git_ref)
            );
            CREATE TABLE IF NOT EXISTS installations (
                id INTEGER PRIMARY KEY, repository_id INTEGER NOT NULL REFERENCES repositories(id),
                skill_path TEXT NOT NULL, name TEXT NOT NULL, target_path TEXT NOT NULL UNIQUE,
                git_commit TEXT NOT NULL, content_hash TEXT NOT NULL,
                rollback_path TEXT, rollback_commit TEXT, rollback_hash TEXT,
                active INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE IF NOT EXISTS mcp_servers (
                name TEXT PRIMARY KEY,
                spec TEXT NOT NULL,
                omp INTEGER NOT NULL DEFAULT 0,
                claude INTEGER NOT NULL DEFAULT 0,
                codex INTEGER NOT NULL DEFAULT 0,
                managed INTEGER NOT NULL DEFAULT 0
            );
            DROP TABLE IF EXISTS local_origins;",
        )
        .map_err(|e| format!("初始化 AMC 数据库失败: {e}"))?;
        // 旧库迁移：历史遗留行（含旧版自动导入）默认 managed=0，
        // 视为非 AMC 管理——不列出、不可改删，也不动用户 Agent 文件。
        let has_managed: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('mcp_servers') WHERE name='managed'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| format!("检查 mcp_servers 表结构失败: {e}"))?;
        if has_managed == 0 {
            db.execute(
                "ALTER TABLE mcp_servers ADD COLUMN managed INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|e| format!("迁移 mcp_servers 表失败: {e}"))?;
        }
        Ok(Self { db, root })
    }

    pub fn repositories(&self) -> Result<Vec<Repository>> {
        let mut stmt = self
            .db
            .prepare("SELECT id,url,git_ref FROM repositories ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Repository {
                    id: r.get(0)?,
                    url: r.get(1)?,
                    reference: r.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())
    }

    pub fn mcp_servers(&self) -> Result<Vec<McpRecord>> {
        let mut stmt = self
            .db
            .prepare("SELECT name,spec,omp,claude,codex,managed FROM mcp_servers WHERE managed=1 ORDER BY name")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], Self::row_mcp).map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())
    }

    fn row_mcp(row: &rusqlite::Row<'_>) -> rusqlite::Result<McpRecord> {
        let spec: String = row.get(1)?;
        Ok(McpRecord {
            name: row.get(0)?,
            spec: serde_json::from_str(&spec)
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
            omp: row.get::<_, i64>(2)? != 0,
            claude: row.get::<_, i64>(3)? != 0,
            codex: row.get::<_, i64>(4)? != 0,
            managed: row.get::<_, i64>(5)? != 0,
        })
    }

    pub fn mcp_server(&self, name: &str) -> Result<Option<McpRecord>> {
        self.db
            .query_row(
                "SELECT name,spec,omp,claude,codex,managed FROM mcp_servers WHERE name=?1",
                [name],
                Self::row_mcp,
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    pub fn save_mcp_server(&self, record: &McpRecord) -> Result<()> {
        let spec = serde_json::to_string(&record.spec).map_err(|e| e.to_string())?;
        self.db
            .execute(
                "INSERT OR REPLACE INTO mcp_servers(name,spec,omp,claude,codex,managed) VALUES (?1,?2,?3,?4,?5,?6)",
                params![record.name, spec, record.omp, record.claude, record.codex, record.managed],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 只允许删除 AMC 管理的记录；未管理行对删除不可见。
    pub fn delete_mcp_server(&self, name: &str) -> Result<bool> {
        let removed = self
            .db
            .execute("DELETE FROM mcp_servers WHERE name=?1 AND managed=1", [name])
            .map_err(|e| e.to_string())?;
        Ok(removed > 0)
    }

    pub fn repository(&self, id: i64) -> Result<Repository> {
        self.db
            .query_row(
                "SELECT id,url,git_ref FROM repositories WHERE id=?1",
                [id],
                |r| {
                    Ok(Repository {
                        id: r.get(0)?,
                        url: r.get(1)?,
                        reference: r.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "仓库不存在".into())
    }

    pub fn records(&self) -> Result<Vec<InstallRecord>> {
        let mut stmt = self.db.prepare("SELECT id,repository_id,skill_path,name,target_path,git_commit,content_hash,rollback_path,rollback_commit,rollback_hash,active FROM installations ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], Self::row_record)
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())
    }

    fn row_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<InstallRecord> {
        Ok(InstallRecord {
            id: row.get(0)?,
            repository_id: row.get(1)?,
            skill_path: row.get(2)?,
            name: row.get(3)?,
            target_path: row.get(4)?,
            commit: row.get(5)?,
            hash: row.get(6)?,
            rollback_path: row.get(7)?,
            rollback_commit: row.get(8)?,
            rollback_hash: row.get(9)?,
            active: row.get(10)?,
        })
    }

    pub fn record(&self, id: i64) -> Result<InstallRecord> {
        self.db.query_row("SELECT id,repository_id,skill_path,name,target_path,git_commit,content_hash,rollback_path,rollback_commit,rollback_hash,active FROM installations WHERE id=?1", [id], Self::row_record)
            .optional().map_err(|e| e.to_string())?.ok_or_else(|| "安装记录不存在".into())
    }

    pub fn by_path(&self, path: &Path) -> Result<Option<InstallRecord>> {
        self.db.query_row("SELECT id,repository_id,skill_path,name,target_path,git_commit,content_hash,rollback_path,rollback_commit,rollback_hash,active FROM installations WHERE target_path=?1", [path.to_string_lossy().as_ref()], Self::row_record)
            .optional().map_err(|e| e.to_string())
    }

    pub fn repo_dir(&self, id: i64) -> PathBuf {
        self.root.join("repositories").join(id.to_string())
    }

    pub fn add_repository(&mut self, url: &str, reference: &str) -> Result<Repository> {
        let url = validated_url(url)?;
        let reference = validated_ref(reference)?;
        if let Some(existing) = self
            .repositories()?
            .into_iter()
            .find(|repo| repo.url == url && repo.reference == reference)
        {
            return Ok(existing);
        }
        self.db
            .execute(
                "INSERT INTO repositories(url,git_ref) VALUES (?1,?2)",
                params![url, reference],
            )
            .map_err(|e| e.to_string())?;
        let id = self.db.last_insert_rowid();
        let repo = Repository { id, url, reference };
        if let Err(error) = self.clone_repository(&repo) {
            self.db
                .execute("DELETE FROM repositories WHERE id=?1", [id])
                .map_err(|e| format!("{error}; 清理记录失败: {e}"))?;
            return Err(error);
        }
        Ok(repo)
    }

    pub fn remove_repository(&mut self, id: i64) -> Result<String> {
        self.repository(id)?;
        let records: Vec<_> = self
            .records()?
            .into_iter()
            .filter(|record| record.repository_id == id)
            .collect();
        if records.iter().any(|record| record.active) {
            return Err("该仓库仍有已安装的技能；请先移除这些安装再删除仓库".into());
        }
        let mut cleanup = Vec::new();
        for record in &records {
            let Some(backup_path) = &record.rollback_path else {
                continue;
            };
            let backup = PathBuf::from(backup_path);
            let target = Path::new(&record.target_path);
            let skills_root = target.parent().ok_or("历史安装路径无效")?;
            let workspace_root = skills_root
                .parent()
                .and_then(Path::parent)
                .ok_or("历史安装路径无效")?;
            if !workspace::is_installation_target(workspace_root, target) {
                return Err("历史安装路径不在工作区 .agents/skills 内".into());
            }
            let canonical_workspace =
                fs::canonicalize(workspace_root).map_err(|_| "历史工作区路径无效")?;
            let root = canonical_workspace.join(".agents");
            let expected_parent =
                fs::canonicalize(root.join(".amc-backups")).map_err(|_| "历史备份目录无效")?;
            let backup_parent = fs::canonicalize(backup.parent().ok_or("历史备份路径无效")?)
                .map_err(|_| "历史备份目录无效")?;
            let owned_backup = backup_parent == expected_parent
                && backup
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok());
            let canonical_backup = fs::canonicalize(&backup).map_err(|_| "历史备份路径无效")?;
            platform::no_links(&canonical_backup, &root)?;
            if !owned_backup {
                return Err(format!(
                    "历史备份不在 AMC 管理目录内，拒绝删除: {}",
                    backup.display()
                ));
            }
            cleanup.push(backup);
        }
        let cache = self.repo_dir(id);
        platform::no_links(&cache, &self.root)?;
        cleanup.push(cache);
        let tx = self
            .db
            .transaction()
            .map_err(|e| format!("开始仓库删除事务失败: {e}"))?;
        tx.execute(
            "DELETE FROM installations WHERE repository_id=?1 AND active=0",
            [id],
        )
        .map_err(|e| format!("清理安装历史失败: {e}"))?;
        tx.execute("DELETE FROM repositories WHERE id=?1", [id])
            .map_err(|e| format!("删除仓库记录失败: {e}"))?;
        tx.commit().map_err(|e| format!("提交仓库删除失败: {e}"))?;
        let mut leftovers = Vec::new();
        for path in cleanup {
            if let Err(error) = fs::remove_dir_all(&path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    leftovers.push(format!("{} ({error})", path.display()));
                }
            }
        }
        if leftovers.is_empty() {
            Ok("仓库及关联的已移除技能历史、缓存已删除".into())
        } else {
            Ok(format!(
                "仓库记录已删除，但目录清理失败；请手动检查: {}",
                leftovers.join("；")
            ))
        }
    }

    fn clone_repository(&self, repo: &Repository) -> Result<()> {
        let cache = self.root.join("repositories");
        fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
        let temp = cache.join(format!(".clone-{}", uuid::Uuid::new_v4()));
        let mut command = Command::new("git");
        command.args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.ext.allow=never",
            "clone",
            "--no-checkout",
            "--depth",
            "1",
            "--single-branch",
        ]);
        if !repo.reference.is_empty() {
            command.args(["--branch", &repo.reference]);
        }
        command.arg("--").arg(&repo.url).arg(&temp);
        let result = run_git(&mut command);
        if let Err(error) = result {
            let _ = fs::remove_dir_all(&temp);
            return Err(error);
        }
        let commit = match git_commit(&temp) {
            Ok(commit) => commit,
            Err(error) => {
                let _ = fs::remove_dir_all(&temp);
                return Err(error);
            }
        };
        if let Err(error) = checkout(&temp, &commit) {
            let _ = fs::remove_dir_all(&temp);
            return Err(error);
        }
        if let Err(error) = fs::rename(&temp, self.repo_dir(repo.id)) {
            let _ = fs::remove_dir_all(&temp);
            return Err(format!("缓存 Git 仓库失败: {error}"));
        }
        Ok(())
    }

    pub fn update_repository(&self, repo: &Repository) -> Result<String> {
        let directory = self.repo_dir(repo.id);
        platform::no_links(&directory, &self.root)?;
        platform::no_links(&directory.join(".git"), &self.root)?;
        if !directory.is_dir() {
            return Err("Git 缓存缺失，请重新添加仓库".into());
        }
        let mut fetch = git(&directory);
        fetch.args(["fetch", "--depth", "1", "origin"]);
        fetch.arg(if repo.reference.is_empty() {
            "HEAD"
        } else {
            &repo.reference
        });
        run_git(&mut fetch)?;
        let mut rev = git(&directory);
        rev.args(["rev-parse", "--verify", "FETCH_HEAD^{commit}"]);
        let commit = run_git(&mut rev)?.trim().to_owned();
        checkout(&directory, &commit)?;
        Ok(commit)
    }

    pub fn commit(&self, id: i64) -> Result<String> {
        let directory = self.repo_dir(id);
        platform::no_links(&directory.join(".git"), &self.root)?;
        git_commit(&directory)
    }

    pub fn installations(&self) -> Result<Vec<Installation>> {
        let mut result = Vec::new();
        for record in self.records()? {
            let target = Path::new(&record.target_path);
            let modified = record.active
                && platform::snapshot(target)
                    .map(|files| platform::tree_hash(&files) != record.hash)
                    .unwrap_or(true);
            let source = self.repo_dir(record.repository_id).join(&record.skill_path);
            let update_available = record.active
                && platform::source_snapshot(&source, &self.repo_dir(record.repository_id))
                    .map(|files| !files.is_empty() && platform::tree_hash(&files) != record.hash)
                    .unwrap_or(false);
            let target_free = matches!(fs::symlink_metadata(target), Err(e) if e.kind() == std::io::ErrorKind::NotFound);
            let rollback_available = (if record.active {
                !modified
            } else {
                target_free
            }) && record
                .rollback_path
                .as_deref()
                .is_some_and(|path| Path::new(path).is_dir())
                && record.rollback_commit.is_some()
                && record.rollback_hash.is_some();
            result.push(Installation {
                id: record.id,
                repository_id: record.repository_id,
                skill_path: record.skill_path,
                name: record.name,
                target_path: record.target_path,
                commit: record.commit,
                modified,
                update_available,
                active: record.active,
                rollback_available,
            });
        }
        Ok(result)
    }
}

fn validated_url(url: &str) -> Result<String> {
    let url = url.trim();
    if url.is_empty() || url.contains('\0') || url.contains('\n') || url.contains('\r') {
        return Err("无效 Git 地址".into());
    }
    if url.starts_with("https://")
        || url.starts_with("ssh://")
        || (url.starts_with("git@") && url.contains(':'))
    {
        return Ok(url.into());
    }
    let path = Path::new(url);
    if path.is_absolute() && path.is_dir() {
        return fs::canonicalize(path)
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| e.to_string());
    }
    Err("仅支持 HTTPS、SSH 或本地绝对路径的 Git 仓库".into())
}

fn validated_ref(reference: &str) -> Result<String> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Ok(String::new());
    }
    if reference.len() > 200
        || reference.starts_with('-')
        || reference.starts_with('/')
        || reference.ends_with('/')
        || reference.contains("..")
        || reference.contains("//")
        || reference.contains("@{")
        || !reference
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'/'))
    {
        return Err("无效 Git 分支或标签".into());
    }
    Ok(reference.into())
}

fn git(directory: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(directory).args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "protocol.ext.allow=never",
    ]);
    command
}

fn git_commit(directory: &Path) -> Result<String> {
    let mut command = git(directory);
    command.args(["rev-parse", "--verify", "HEAD^{commit}"]);
    Ok(run_git(&mut command)?.trim().to_owned())
}

fn checkout(directory: &Path, commit: &str) -> Result<()> {
    let mut command = git(directory);
    command.args(["checkout", "--force", "--detach", commit]);
    run_git(&mut command)?;
    Ok(())
}

fn run_git(command: &mut Command) -> Result<String> {
    let output = command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new",
        )
        .output()
        .map_err(|e| format!("启动 Git 失败: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Git 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn removing_repository_requires_no_active_installations_and_purges_removed_history() {
        let fixture = Fixture(
            std::env::temp_dir().join(format!("amc-repo-removal-{}", uuid::Uuid::new_v4())),
        );
        let mut store = Store::new(fixture.0.join("state")).unwrap();
        store
            .db
            .execute(
                "INSERT INTO repositories(url,git_ref) VALUES (?1,'')",
                ["https://example.org/skills.git"],
            )
            .unwrap();
        let repo_id = store.db.last_insert_rowid();
        let cache = store.repo_dir(repo_id);
        fs::create_dir_all(&cache).unwrap();
        let target = fixture.0.join("project/.agents/skills/sample");
        fs::create_dir_all(&target).unwrap();
        let backup = fixture
            .0
            .join("project/.agents/.amc-backups")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&backup).unwrap();
        store.db.execute(
            "INSERT INTO installations(repository_id,skill_path,name,target_path,git_commit,content_hash,rollback_path,rollback_commit,rollback_hash,active)
             VALUES (?1,'skills/sample','sample',?2,'commit','hash',?3,'previous','previous_hash',1)",
            params![repo_id, target.to_str().unwrap(), backup.to_str().unwrap()],
        ).unwrap();
        let installation_id = store.db.last_insert_rowid();

        assert!(store.remove_repository(repo_id).is_err());
        assert!(store.repository(repo_id).is_ok());
        assert!(cache.is_dir() && backup.is_dir() && target.is_dir());

        store
            .db
            .execute(
                "UPDATE installations SET active=0 WHERE id=?1",
                [installation_id],
            )
            .unwrap();
        fs::remove_dir_all(&target).unwrap();
        store.remove_repository(repo_id).unwrap();
        assert!(store.repository(repo_id).is_err());
        assert!(store.record(installation_id).is_err());
        assert!(!cache.exists() && !backup.exists());
    }
}
