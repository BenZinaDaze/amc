// ZCode usage: the desktop app archives one row per model request in its
// local sqlite database `~/.zcode/cli/db/db.sqlite` (table `model_usage`);
// no CLI sync step exists. The table is ZCode-internal and may change across
// versions, so a failing scan only degrades to an error message — already
// archived statistics survive in the AMC store.
#[cfg(test)]
use crate::usage::prices;
use crate::usage::{AgentUsageAdapter, SourceScan, UsageRecord};
use crate::platform;
use rusqlite::OpenFlags;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub(crate) struct ZcodeUsageAdapter {
    db: PathBuf,
}

impl ZcodeUsageAdapter {
    pub(crate) fn from_environment() -> platform::Result<Self> {
        let home = platform::home()?;
        Self::from_base(home.join(".zcode"))
    }

    fn from_base(base: PathBuf) -> platform::Result<Self> {
        let db = base.join("cli").join("db").join("db.sqlite");
        if !db.is_file() {
            return Err(format!(
                "找不到 ZCode 数据库 {}；请确认已安装并使用过 ZCode",
                db.display()
            ));
        }
        Ok(Self { db })
    }
}

/// 打开 ZCode 的 WAL 数据库：应用运行中时 -wal/-shm 由其维护，只读连接
/// 可并发读取；只读打开失败（如崩溃后遗留 -wal 而无 -shm）时回退读写
/// 打开，让 SQLite 自行完成 WAL 恢复。AMC 绝不写入业务数据。
fn open_read(path: &Path) -> platform::Result<rusqlite::Connection> {
    if let Ok(db) = rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
    {
        return Ok(db);
    }
    let db = rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| format!("打开 ZCode 数据库 {} 失败: {e}", path.display()))?;
    db.busy_timeout(std::time::Duration::from_millis(5_000))
        .map_err(|e| format!("设置 ZCode 数据库并发超时失败: {e}"))?;
    Ok(db)
}

impl AgentUsageAdapter for ZcodeUsageAdapter {
    fn scan(&self, since: i64) -> platform::Result<SourceScan> {
        let db = open_read(&self.db)?;
        let mut synced_at: i64 = db
            .query_row(
                "SELECT MAX(started_at) FROM model_usage WHERE status = 'completed'",
                [],
                |row| row.get(0),
            )
            .map_err(schema_error)?;
        synced_at = synced_at.max(0);
        let mut stmt = db
            .prepare(
                "SELECT id, model_id, started_at, input_tokens, output_tokens, \
                 cache_read_input_tokens, cache_creation_input_tokens \
                 FROM model_usage \
                 WHERE status = 'completed' AND started_at >= ?1",
            )
            .map_err(schema_error)?;
        let rows = stmt
            .query_map([since], |row| {
                // ZCode 记录模型注册名的大小写（GLM-5.3），计价表按小写键。
                let model: String = row.get(1)?;
                let stored_input: i64 = row.get(3)?;
                let output: i64 = row.get(4)?;
                let read: i64 = row.get(5)?;
                let write: i64 = row.get(6)?;
                // model_usage.input_tokens 存的是 prompt 侧合计（含缓存读
                // 与写，实测 rollout 原始 API usage 与库行对账：净输入 1215
                // + cache_read 270592 → 存 271807；ZCode 读侧 inputSide-
                // TokensFromStoredUsage 的默认分支同样按含缓存处理）。AMC
                // 的 input_tokens 语义是净输入，这里换算回去。
                let input = (stored_input - read - write).max(0);
                Ok(UsageRecord {
                    external_id: row.get(0)?,
                    model: model.to_lowercase(),
                    timestamp: row.get(2)?,
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: read,
                    cache_write_tokens: write,
                    total_tokens: input + output + read + write,
                })
            })
            .map_err(|e| format!("读取 ZCode 用量失败: {e}"))?;
        let mut records = Vec::new();
        for record in rows {
            records.push(record.map_err(|e| format!("读取 ZCode 用量失败: {e}"))?);
        }
        Ok(SourceScan { records, synced_at })
    }
}

fn schema_error(error: rusqlite::Error) -> String {
    format!("读取 ZCode 用量表失败（数据库结构可能随版本变化）: {error}")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZcodeStatus {
    pub installed: bool,
    pub version: String,
}

/// ZCode 是桌面应用，没有 PATH 上的可执行文件：安装状态看数据目录
/// `~/.zcode/cli` 是否存在，版本取数据库里最近一次会话记录的客户端
/// 版本（读不到时为空串）。
pub fn zcode_status() -> ZcodeStatus {
    let Some(home) = platform::home().ok() else {
        return ZcodeStatus {
            installed: false,
            version: String::new(),
        };
    };
    let dir = home.join(".zcode").join("cli");
    if !dir.is_dir() {
        return ZcodeStatus {
            installed: false,
            version: String::new(),
        };
    }
    let version = dir
        .join("db")
        .join("db.sqlite")
        .is_file()
        .then(|| read_version(&dir))
        .flatten()
        .unwrap_or_default();
    ZcodeStatus {
        installed: true,
        version,
    }
}

fn read_version(dir: &Path) -> Option<String> {
    let db = open_read(&dir.join("db").join("db.sqlite")).ok()?;
    let version: Option<String> = db
        .query_row(
            "SELECT version FROM session ORDER BY time_created DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok();
    version.filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::fs;

    /// 建一个只含适配器所读列的 ZCode 数据库；started_at 为 epoch 毫秒。
    /// 行字段见 `UsageRow`；`stored_input` 是库里 input 列的原值（prompt
    /// 合计，含缓存读与写）。
    struct UsageRow<'a> {
        id: &'a str,
        model: &'a str,
        started_at: i64,
        stored_input: i64,
        output: i64,
        cache_read: i64,
        cache_write: i64,
        status: &'a str,
    }

    fn zcode_db(path: &Path, rows: &[UsageRow<'_>]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = rusqlite::Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY, version TEXT NOT NULL,
                time_created INTEGER NOT NULL
            );
            CREATE TABLE model_usage (
                id TEXT PRIMARY KEY, model_id TEXT NOT NULL,
                started_at INTEGER NOT NULL, input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cache_read_input_tokens INTEGER NOT NULL,
                cache_creation_input_tokens INTEGER NOT NULL,
                status TEXT NOT NULL
            );",
        )
        .unwrap();
        db.execute(
            "INSERT INTO session(id, version, time_created) VALUES ('s1', '0.17.0', ?1)",
            [rows.last().map(|row| row.started_at).unwrap_or(0)],
        )
        .unwrap();
        for row in rows {
            db.execute(
                "INSERT INTO model_usage VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    row.id,
                    row.model,
                    row.started_at,
                    row.stored_input,
                    row.output,
                    row.cache_read,
                    row.cache_write,
                    row.status
                ],
            )
            .unwrap();
        }
    }

    #[test]
    fn zcode_scan_converts_stored_prompt_total_to_net_input() {
        let root = std::env::temp_dir().join(format!("amc-zcode-{}", uuid::Uuid::new_v4()));
        let start = 1_760_000_000_000_i64;
        // 库里 input 列是 prompt 合计（净输入 + 缓存读 + 缓存写）；
        // 行字段：(id, model, started_at, input[合计], output, cache_read, cache_write, status)。
        let row = |id, model, started_at, stored_input, output, cache_read, cache_write, status| UsageRow {
            id,
            model,
            started_at,
            stored_input,
            output,
            cache_read,
            cache_write,
            status,
        };
        zcode_db(
            &root.join("cli/db/db.sqlite"),
            &[
                // 常规请求：净 1000 + 读 4000 + 写 300 → 存 5300。
                row("u1", "GLM-5.3", start, 5300, 200, 4000, 300, "completed"),
                // 冷缓存首请求：无缓存，input 即净输入。
                row("u2", "GLM-5.3", start + 1000, 10, 20, 0, 0, "completed"),
                row("u3", "glm-5.3-flash", start + 2000, 4, 2, 3, 0, "completed"),
                // 错误与取消的请求没有用量，不进归档。
                row("u4", "GLM-5.3", start + 3000, 999, 999, 0, 0, "error"),
                // 零用量行：由归档写入护栏丢弃。
                row("u5", "GLM-5.3", start + 4000, 0, 0, 0, 0, "completed"),
                // 防御：脏数据 input < cache_read 时钳到 0，不产生负数。
                row("u6", "GLM-5.3", start + 5000, 100, 7, 500, 0, "completed"),
            ],
        );
        let adapter = ZcodeUsageAdapter::from_base(root.clone()).unwrap();
        let mut store = crate::usage::store::UsageStore::new(&root).unwrap();
        let scan = adapter.scan(0).unwrap();
        assert_eq!(scan.synced_at, start + 5000);
        store.ingest("zcode", &scan.records).unwrap();
        // 重扫不变：主键 id 幂等去重。
        let rescan = adapter.scan(0).unwrap();
        assert_eq!(store.ingest("zcode", &rescan.records).unwrap(), 0);
        // 水位过滤：只取水位之后的行。
        let incremental = adapter.scan(start + 4500).unwrap();
        let ids: Vec<&str> = incremental
            .records
            .iter()
            .map(|record| record.external_id.as_str())
            .collect();
        assert_eq!(ids, vec!["u6"]);
        let stats = store
            .query(
                Some("zcode"),
                crate::usage::UsageRange::parse(
                    &format!("custom:{start}:{}", start + 86_400_000),
                    start + 1,
                )
                .unwrap(),
                &prices(),
            )
            .unwrap()
            .finish();
        // u1/u2/u3/u6 入档（u5 零用量被护栏丢弃）；input 是换算后的净输入
        // （u6 脏数据被钳到 0）。
        assert_eq!(stats.total_requests, 4);
        assert_eq!(stats.input_tokens, 1000 + 10 + 1);
        assert_eq!(stats.output_tokens, 200 + 20 + 2 + 7);
        assert_eq!(stats.cache_read_tokens, 4000 + 3 + 500);
        assert_eq!(stats.total_tokens, (1000 + 10 + 1) + (200 + 20 + 2 + 7) + (4000 + 3 + 500) + 300);
        // 缓存命中率 = 读 /（净输入 + 读）：4503 / 5514。
        assert!((stats.cache_rate - 4503.0 / 5514.0).abs() < 1e-12);
        // 大小写归一到计价表键后全部可计价。
        assert_eq!(stats.unpriced_requests, 0);
        assert!(stats.total_cost.is_some());
        let models: Vec<String> = stats.by_model.iter().map(|m| m.model.clone()).collect();
        assert_eq!(models, vec!["glm-5.3".to_owned(), "glm-5.3-flash".to_owned()]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn zcode_adapter_reports_missing_database_and_reads_status_version() {
        let root = std::env::temp_dir().join(format!("amc-zcode-missing-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let error = ZcodeUsageAdapter::from_base(root.clone()).err().unwrap();
        assert!(error.contains("找不到 ZCode 数据库"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }
}
