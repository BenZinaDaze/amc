//! AMC-owned usage archive. Every record a source scan produces lands here
//! exactly once — keyed by `(source, external_id)` — so statistics survive
//! the source agents cleaning up their own transcripts or stats databases.
//! Aggregation always reads this store, never the live sources; costs are
//! recomputed from the current pricing catalog at query time, so catalog
//! updates reprice archived usage.
use super::{resolve_provider, RawUsage, Totals, UsageRange, UsageRecord};
use crate::{platform, pricing::Pricing};
use rusqlite::params;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

/// `usage_records` 查询一行的原始列值。
type UsageRow = (String, String, String, i64, i64, i64, i64, i64, i64);

const FILE_NAME: &str = "usage.sqlite3";
const BUSY_TIMEOUT: Duration = Duration::from_millis(5_000);

pub(super) struct UsageStore {
    path: PathBuf,
}

impl UsageStore {
    pub(super) fn new(data_dir: &Path) -> platform::Result<Self> {
        std::fs::create_dir_all(data_dir).map_err(|e| format!("创建 AMC 数据目录失败: {e}"))?;
        Ok(Self {
            path: data_dir.join(FILE_NAME),
        })
    }

    fn open(&self) -> platform::Result<rusqlite::Connection> {
        let db = rusqlite::Connection::open(&self.path)
            .map_err(|e| format!("打开 AMC 用量数据库 {} 失败: {e}", self.path.display()))?;
        db.busy_timeout(BUSY_TIMEOUT)
            .map_err(|e| format!("设置 AMC 用量数据库并发超时失败: {e}"))?;
        // WAL keeps concurrent command handlers from failing on locks; the
        // pragma returns the resulting mode as a row, hence query_row.
        db.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
            .map_err(|e| format!("设置 AMC 用量数据库日志模式失败: {e}"))?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_records (
                source TEXT NOT NULL,
                external_id TEXT NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cache_read_tokens INTEGER NOT NULL,
                cache_write_tokens INTEGER NOT NULL,
                total_tokens INTEGER NOT NULL,
                PRIMARY KEY (source, external_id)
            );
            CREATE INDEX IF NOT EXISTS usage_records_timestamp
                ON usage_records (timestamp);",
        )
        .map_err(|e| format!("初始化 AMC 用量数据库失败: {e}"))?;
        Ok(db)
    }

    /// Inserts unseen records and returns how many were new. The primary
    /// key makes repeated scans of the same source idempotent: resumes,
    /// forks and re-broadcasts collapse into the record they duplicate.
    pub(super) fn ingest(&self, source: &str, records: &[UsageRecord]) -> platform::Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        let mut db = self.open()?;
        let transaction = db
            .transaction()
            .map_err(|e| format!("开启 AMC 用量事务失败: {e}"))?;
        let mut inserted = 0;
        {
            let mut stmt = transaction
                .prepare(
                    "INSERT OR IGNORE INTO usage_records \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                )
                .map_err(|e| format!("准备 AMC 用量写入失败: {e}"))?;
            for record in records {
                inserted += stmt
                    .execute(params![
                        source,
                        record.external_id,
                        record.provider,
                        record.model,
                        record.timestamp,
                        record.input_tokens,
                        record.output_tokens,
                        record.cache_read_tokens,
                        record.cache_write_tokens,
                        record.total_tokens,
                    ])
                    .map_err(|e| format!("写入 AMC 用量失败: {e}"))?;
            }
        }
        transaction
            .commit()
            .map_err(|e| format!("提交 AMC 用量写入失败: {e}"))?;
        Ok(inserted)
    }

    /// Aggregates archived raw records for one source (or every source when
    /// `source` is `None`) over the range: cutoff inclusive, end exclusive,
    /// trend bucketed by the range's bucket size.
    pub(super) fn query(
        &self,
        source: Option<&str>,
        range: UsageRange,
        prices: &Pricing,
    ) -> platform::Result<RawUsage> {
        let db = self.open()?;
        let mut totals = Totals::default();
        let mut models: BTreeMap<(String, String), Totals> = BTreeMap::new();
        let mut trend: BTreeMap<i64, (i64, i64)> = BTreeMap::new();
        let mut stmt = db
            .prepare(
                "SELECT source, provider, model, timestamp, input_tokens, output_tokens, \
                 cache_read_tokens, cache_write_tokens, total_tokens \
                 FROM usage_records \
                 WHERE (?1 IS NULL OR source = ?1) AND timestamp >= ?2 \
                   AND (?3 IS NULL OR timestamp < ?3)",
            )
            .map_err(|e| format!("读取 AMC 用量数据库失败: {e}"))?;
        let mut rows = stmt
            .query(params![source, range.cutoff, range.end_exclusive])
            .map_err(|e| format!("读取 AMC 用量失败: {e}"))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| format!("读取 AMC 用量失败: {e}"))?
        {
            let record: rusqlite::Result<UsageRow> = (|| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                })();
            let (row_source, stored_provider, model, timestamp, input, output, read, write, tokens) =
                record.map_err(|e| format!("AMC 用量记录无效: {e}"))?;
            let provider = if stored_provider.is_empty() {
                resolve_provider(&row_source, &model, prices)
            } else {
                stored_provider
            };
            let cost = prices.cost(&provider, &model, input, read, write, output);
            let model_totals = models.entry((provider, model)).or_default();
            for item in [&mut totals, model_totals] {
                item.requests += 1;
                item.total_tokens += tokens;
                item.input_tokens += input;
                item.output_tokens += output;
                item.cache_read_tokens += read;
                if let Some(cost) = cost {
                    item.cost += cost;
                } else {
                    item.unpriced_requests += 1;
                }
            }
            let bucket = timestamp.div_euclid(range.bucket_ms) * range.bucket_ms;
            let point = trend.entry(bucket).or_default();
            point.0 += 1;
            point.1 += tokens;
        }
        Ok(RawUsage {
            totals,
            models,
            trend,
            synced_at: 0,
        })
    }

    /// Newest archived timestamp for a source. Scans pass it back to the
    /// source as a lower bound so files that cannot hold anything newer are
    /// skipped without parsing.
    pub(super) fn high_water(&self, source: &str) -> platform::Result<i64> {
        let db = self.open()?;
        db.query_row(
            "SELECT MAX(timestamp) FROM usage_records WHERE source = ?1",
            [source],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(|e| format!("读取 AMC 用量水位失败: {e}"))
        .map(|value| value.unwrap_or(0))
    }

    /// Whether the archive holds any records for a source (or for any
    /// source when `source` is `None`).
    pub(super) fn has_records(&self, source: Option<&str>) -> platform::Result<bool> {
        let db = self.open()?;
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM usage_records WHERE ?1 IS NULL OR source = ?1)",
            params![source],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|e| format!("读取 AMC 用量数据库失败: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(external_id: &str, timestamp: i64, tokens: i64) -> UsageRecord {
        UsageRecord {
            external_id: external_id.to_owned(),
            provider: String::new(),
            model: "model".to_owned(),
            timestamp,
            input_tokens: 1,
            output_tokens: 2,
            cache_read_tokens: 3,
            cache_write_tokens: 0,
            total_tokens: tokens,
        }
    }

    #[test]
    fn repeated_ingests_are_idempotent_per_source_and_external_id() {
        let root = std::env::temp_dir().join(format!("amc-store-dedupe-{}", uuid::Uuid::new_v4()));
        let store = UsageStore::new(&root).unwrap();
        let records = [record("a", 1_700_000_000_000, 6), record("b", 1_700_000_001_000, 6)];
        assert_eq!(store.ingest("omp", &records).unwrap(), 2);
        // The same records again — a fresh scan of an unchanged source.
        assert_eq!(store.ingest("omp", &records).unwrap(), 0);
        // The same external id under another source is a distinct record.
        assert_eq!(store.ingest("claude-code", &records[..1]).unwrap(), 1);
        let stats = store
            .query(
                None,
                UsageRange::parse("all", 1_700_000_002_000).unwrap(),
                &crate::usage::prices(),
            )
            .unwrap()
            .finish();
        assert_eq!(stats.total_requests, 3);
        assert_eq!(stats.total_tokens, 18);
        assert_eq!(store.high_water("omp").unwrap(), 1_700_000_001_000);
        assert_eq!(store.high_water("codex").unwrap(), 0);
        assert!(store.has_records(Some("claude-code")).unwrap());
        assert!(!store.has_records(Some("codex")).unwrap());
        assert!(store.has_records(None).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn queries_filter_by_source_and_range_boundaries() {
        let root = std::env::temp_dir().join(format!("amc-store-range-{}", uuid::Uuid::new_v4()));
        let store = UsageStore::new(&root).unwrap();
        let start = 1_700_000_000_000;
        let end = start + 86_400_000;
        store
            .ingest(
                "omp",
                &[
                    record("before", start - 1, 6),
                    record("first", start, 6),
                    record("last", end - 1, 6),
                    record("after", end, 6),
                ],
            )
            .unwrap();
        let stats = store
            .query(
                Some("omp"),
                UsageRange::parse(&format!("custom:{start}:{end}"), end).unwrap(),
                &crate::usage::prices(),
            )
            .unwrap()
            .finish();
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.synced_at, 0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
