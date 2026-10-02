// Agent usage collection: shared aggregation types, the adapter contract,
// the registry and the AMC-owned archive. One source file lives beside it
// (`omp.rs`, `claude.rs`, `codex.rs`); a new agent registers in
// [`AGENT_IDS`] and implements [`AgentUsageAdapter`], nothing else.
//
// Adapters only scan: they turn the source's raw records into
// [`store::UsageRecord`]s, which are ingested into the archive
// ([`store::UsageStore`], `<data dir>/usage.sqlite3`) keyed by
// `(source, external_id)`. Every read aggregates the archive over the
// requested range and prices it with the current catalog, so deleting the
// sources never takes already-seen statistics with it.
use crate::{platform, pricing::Pricing};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicI64,
    time::{SystemTime, UNIX_EPOCH},
};

/// One raw usage record as emitted by a source scan. `input_tokens` counts
/// uncached prompt tokens; cache reads and writes are separate buckets.
/// `provider` carries the source-owned value (OMP records one); empty means
/// the provider is resolved from the pricing catalog when queried, so a
/// later catalog entry can still price and attribute the record.
pub(super) struct UsageRecord {
    pub external_id: String,
    pub provider: String,
    pub model: String,
    pub timestamp: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
}

/// What one source scan produced: raw records destined for the archive plus
/// the source's own notion of when it was last written.
pub(super) struct SourceScan {
    pub records: Vec<UsageRecord>,
    pub synced_at: i64,
}

pub(super) trait AgentUsageAdapter {
    /// Optional CLI refresh before scanning; OMP runs `omp stats --summary`
    /// so its stats database reflects the newest sessions.
    fn sync_source(&self) -> platform::Result<()> {
        Ok(())
    }

    /// Scans records written at or after `since` (0 scans everything).
    /// Overlap between scans is harmless: the archive deduplicates by the
    /// source's stable record identity.
    fn scan(&self, since: i64) -> platform::Result<SourceScan>;
}

mod agents;
mod store;

pub(crate) use agents::claude::{claude_code_status, ClaudeCodeStatus};
pub(crate) use agents::codex::{codex_status, CodexStatus};
pub(crate) use agents::omp::status as omp_status;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageStats {
    total_requests: i64,
    total_tokens: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_rate: f64,
    total_cost: Option<f64>,
    unpriced_requests: i64,
    by_model: Vec<ModelUsage>,
    trend: Vec<TrendPoint>,
    synced_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelUsage {
    provider: String,
    model: String,
    requests: i64,
    total_tokens: i64,
    cache_rate: f64,
    cost: Option<f64>,
    unpriced_requests: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TrendPoint {
    bucket: i64,
    requests: i64,
    total_tokens: i64,
}

#[derive(Default)]
struct Totals {
    requests: i64,
    total_tokens: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cost: f64,
    unpriced_requests: i64,
}

impl Totals {
    fn cache_rate(&self) -> f64 {
        let prompt = self.input_tokens + self.cache_read_tokens;
        if prompt <= 0 {
            0.0
        } else {
            (self.cache_read_tokens as f64 / prompt as f64).clamp(0.0, 1.0)
        }
    }
}

/// Aggregatable usage without derived fields; [`RawUsage::finish`] renders it.
struct RawUsage {
    totals: Totals,
    models: BTreeMap<(String, String), Totals>,
    trend: BTreeMap<i64, (i64, i64)>,
    synced_at: i64,
}

impl RawUsage {
    fn finish(self) -> UsageStats {
        UsageStats {
            total_requests: self.totals.requests,
            total_tokens: self.totals.total_tokens,
            input_tokens: self.totals.input_tokens,
            output_tokens: self.totals.output_tokens,
            cache_read_tokens: self.totals.cache_read_tokens,
            cache_rate: self.totals.cache_rate(),
            total_cost: (self.totals.requests > self.totals.unpriced_requests)
                .then_some(self.totals.cost),
            unpriced_requests: self.totals.unpriced_requests,
            by_model: self
                .models
                .into_iter()
                .map(|((provider, model), item)| ModelUsage {
                    provider,
                    model,
                    requests: item.requests,
                    total_tokens: item.total_tokens,
                    cache_rate: item.cache_rate(),
                    cost: (item.requests > item.unpriced_requests).then_some(item.cost),
                    unpriced_requests: item.unpriced_requests,
                })
                .collect(),
            trend: self
                .trend
                .into_iter()
                .map(|(bucket, (requests, total_tokens))| TrendPoint {
                    bucket,
                    requests,
                    total_tokens,
                })
                .collect(),
            synced_at: self.synced_at,
        }
    }
}

/// Resolves the provider for records whose source names only a model (the
/// Claude Code and Codex transcripts). Resolving at query time keeps later
/// catalog entries able to price and re-attribute archived records; the
/// fallback is the CLI vendor, as before.
fn resolve_provider(source: &str, model: &str, prices: &Pricing) -> String {
    let fallback = match source {
        "claude-code" => "anthropic",
        "codex" => "openai",
        _ => source,
    };
    prices
        .primary_provider(model)
        .unwrap_or(fallback)
        .to_owned()
}

#[derive(Clone, Copy)]
struct UsageRange {
    cutoff: i64,
    end_exclusive: Option<i64>,
    bucket_ms: i64,
}

impl UsageRange {
    fn parse(value: &str, now: i64) -> platform::Result<Self> {
        if let Some(bounds) = value.strip_prefix("custom:") {
            let (start, end) = bounds
                .split_once(':')
                .ok_or_else(|| format!("无效的自定义用量时间范围: {value}"))?;
            let parse_bound = |bound: &str| {
                if bound.is_empty() || !bound.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(format!("无效的自定义用量时间范围: {value}"));
                }
                bound
                    .parse::<i64>()
                    .map_err(|_| format!("无效的自定义用量时间范围: {value}"))
            };
            let start = parse_bound(start)?;
            let end = parse_bound(end)?;
            if start >= end {
                return Err(format!(
                    "自定义用量时间范围结束时间必须晚于开始时间: {value}"
                ));
            }
            let duration = end - start;
            let bucket_ms = if duration <= 48 * 3_600_000 {
                3_600_000
            } else if duration <= 180 * 86_400_000 {
                86_400_000
            } else {
                7 * 86_400_000
            };
            return Ok(Self {
                cutoff: start,
                end_exclusive: Some(end),
                bucket_ms,
            });
        }
        let (window_ms, bucket_ms): (Option<i64>, i64) = match value {
            "1h" => (Some(3_600_000), 5 * 60_000),
            "24h" => (Some(24 * 3_600_000), 3_600_000),
            "7d" => (Some(7 * 86_400_000), 86_400_000),
            "14d" => (Some(14 * 86_400_000), 86_400_000),
            "30d" => (Some(30 * 86_400_000), 86_400_000),
            "90d" => (Some(90 * 86_400_000), 86_400_000),
            "all" => (None, 7 * 86_400_000),
            _ => return Err(format!("不支持的用量时间范围: {value}")),
        };
        Ok(Self {
            cutoff: window_ms.map_or(i64::MIN, |window| now.saturating_sub(window)),
            end_exclusive: None,
            bucket_ms,
        })
    }
}

fn now_millis() -> platform::Result<i64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("获取当前时间失败: {e}"))?
        .as_millis();
    i64::try_from(millis).map_err(|_| "当前时间超出支持范围".into())
}

fn rfc3339_millis(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.timestamp_millis())
}

fn modified_millis(path: &Path) -> Option<i64> {
    let time = fs::metadata(path).ok()?.modified().ok()?;
    i64::try_from(time.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}

/// Session transcripts nest arbitrarily (Claude Code keeps official subagent
/// transcripts two levels below the project dir), so the JSONL sources need
/// a recursive walk collecting every `*.jsonl` file.
fn collect_jsonl(source: &str, directory: &Path, files: &mut Vec<PathBuf>) -> platform::Result<()> {
    for entry in fs::read_dir(directory)
        .map_err(|e| format!("读取 {source} 数据目录 {} 失败: {e}", directory.display()))?
    {
        let path = entry
            .map_err(|e| format!("读取 {source} 数据目录 {} 失败: {e}", directory.display()))?
            .path();
        if path.is_dir() {
            collect_jsonl(source, &path, files)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            files.push(path);
        }
    }
    Ok(())
}

static LAST_SUCCESSFUL_SYNC: AtomicI64 = AtomicI64::new(0);

/// Every agent AMC collects usage from; a new source registers here and
/// implements [`AgentUsageAdapter`], nothing else.
const AGENT_IDS: &[&str] = &["omp", "claude-code", "codex"];
fn adapter_for(agent_id: &str) -> platform::Result<Box<dyn AgentUsageAdapter>> {
    use agents::{claude::ClaudeCodeUsageAdapter, codex::CodexUsageAdapter, omp::OmpUsageAdapter};
    match agent_id {
        "omp" => {
            if agents::omp::executable().is_none() {
                return Err("找不到 OMP 可执行文件；请安装 omp 后重试".into());
            }
            Ok(Box::new(OmpUsageAdapter::from_environment()?))
        }
        "claude-code" => Ok(Box::new(ClaudeCodeUsageAdapter::from_environment()?)),
        "codex" => Ok(Box::new(CodexUsageAdapter::from_environment()?)),
        _ => Err(format!("不支持的 Agent 用量来源: {agent_id}")),
    }
}

/// Read-path construction: unlike [`adapter_for`] it does not require the
/// OMP executable, because reading may be served from the archive alone
/// once the CLI (or its data) is gone.
fn adapter_for_read(agent_id: &str) -> platform::Result<Box<dyn AgentUsageAdapter>> {
    use agents::{claude::ClaudeCodeUsageAdapter, codex::CodexUsageAdapter, omp::OmpUsageAdapter};
    match agent_id {
        "omp" => Ok(Box::new(OmpUsageAdapter::from_environment()?)),
        "claude-code" => Ok(Box::new(ClaudeCodeUsageAdapter::from_environment()?)),
        "codex" => Ok(Box::new(CodexUsageAdapter::from_environment()?)),
        _ => Err(format!("不支持的 Agent 用量来源: {agent_id}")),
    }
}

/// Ingests one source's newest records into the archive; returns the scan's
/// own last-write time so callers can report `synced_at`.
fn ingest_source(
    store: &store::UsageStore,
    agent_id: &str,
    adapter: &dyn AgentUsageAdapter,
) -> platform::Result<i64> {
    let scan = adapter.scan(store.high_water(agent_id)?)?;
    store.ingest(agent_id, &scan.records)?;
    Ok(scan.synced_at)
}

/// A source that cannot be scanned only fails the request when the archive
/// holds nothing for it; otherwise the archived statistics survive the
/// cleanup that broke the source.
fn require_archive(
    store: &store::UsageStore,
    agent_id: &str,
    error: String,
) -> platform::Result<()> {
    if store.has_records(Some(agent_id))? {
        Ok(())
    } else {
        Err(error)
    }
}

/// OMP 生效的用户级 MCP 配置位置（含 profile 与 PI_CONFIG_DIR /
/// PI_CODING_AGENT_DIR 解析），供 MCP 管理模块复用同一套目录规则。
pub struct OmpMcpLocation {
    pub mcp_json: PathBuf,
    pub config_root: PathBuf,
}

pub fn omp_mcp_location() -> platform::Result<OmpMcpLocation> {
    let adapter = agents::omp::OmpUsageAdapter::from_environment()?;
    Ok(OmpMcpLocation {
        mcp_json: adapter.mcp_json_path().to_path_buf(),
        config_root: adapter.config_root().to_path_buf(),
    })
}

/// Syncs one agent: refresh its source (OMP CLI), ingest everything new,
/// then aggregate the archive over the range.
pub fn sync_agent_usage(
    agent_id: &str,
    range: &str,
    data_dir: &Path,
    price_file: &Path,
) -> platform::Result<UsageStats> {
    let range = UsageRange::parse(range, now_millis()?)?;
    let prices = Pricing::load(price_file)?;
    let store = store::UsageStore::new(data_dir)?;
    let adapter = adapter_for(agent_id)?;
    adapter.sync_source()?;
    let synced_at = ingest_source(&store, agent_id, adapter.as_ref())?;
    let now = now_millis()?;
    LAST_SUCCESSFUL_SYNC.store(now, std::sync::atomic::Ordering::Relaxed);
    let mut raw = store.query(Some(agent_id), range, &prices)?;
    raw.synced_at = now.max(synced_at);
    Ok(raw.finish())
}

/// Reads one agent's usage: ingest what is new when the source still
/// exists, then aggregate the archive — which keeps answering after the
/// agent cleaned up its files.
pub fn get_agent_usage(
    agent_id: &str,
    range: &str,
    data_dir: &Path,
    price_file: &Path,
) -> platform::Result<UsageStats> {
    let range = UsageRange::parse(range, now_millis()?)?;
    let prices = Pricing::load(price_file)?;
    let store = store::UsageStore::new(data_dir)?;
    let mut synced_at = match adapter_for_read(agent_id) {
        Ok(adapter) => match ingest_source(&store, agent_id, adapter.as_ref()) {
            Ok(scan_synced_at) => scan_synced_at,
            Err(error) => {
                require_archive(&store, agent_id, error)?;
                0
            }
        },
        Err(error) => {
            require_archive(&store, agent_id, error)?;
            0
        }
    };
    let mut raw = store.query(Some(agent_id), range, &prices)?;
    synced_at = synced_at.max(LAST_SUCCESSFUL_SYNC.load(std::sync::atomic::Ordering::Relaxed));
    raw.synced_at = synced_at;
    Ok(raw.finish())
}

/// Syncs every agent; sources that fail (e.g. not installed) are skipped as
/// long as the archive can still answer, and only a fully empty result is
/// reported as an error.
pub fn sync_agents_usage(
    range: &str,
    data_dir: &Path,
    price_file: &Path,
) -> platform::Result<UsageStats> {
    let range = UsageRange::parse(range, now_millis()?)?;
    let prices = Pricing::load(price_file)?;
    let store = store::UsageStore::new(data_dir)?;
    let mut first_error: Option<String> = None;
    let mut synced_any = false;
    for agent_id in AGENT_IDS {
        let result = adapter_for(agent_id)
            .and_then(|adapter| {
                adapter.sync_source()?;
                ingest_source(&store, agent_id, adapter.as_ref())
            })
            .map(|_| ());
        match result {
            Ok(()) => synced_any = true,
            Err(error) => {
                let _ = first_error.get_or_insert(error);
            }
        }
    }
    if !synced_any && !store.has_records(None)? {
        return Err(first_error.unwrap_or_else(|| "没有可统计的 Agent 用量来源".to_owned()));
    }
    let now = now_millis()?;
    if synced_any {
        LAST_SUCCESSFUL_SYNC.store(now, std::sync::atomic::Ordering::Relaxed);
    }
    let mut raw = store.query(None, range, &prices)?;
    raw.synced_at = if synced_any {
        now
    } else {
        LAST_SUCCESSFUL_SYNC.load(std::sync::atomic::Ordering::Relaxed)
    };
    Ok(raw.finish())
}

/// Reads every agent's usage; a source that cannot be scanned only fails
/// the request when the archive holds nothing for it.
pub fn get_agents_usage(
    range: &str,
    data_dir: &Path,
    price_file: &Path,
) -> platform::Result<UsageStats> {
    let range = UsageRange::parse(range, now_millis()?)?;
    let prices = Pricing::load(price_file)?;
    let store = store::UsageStore::new(data_dir)?;
    let mut synced_at = 0;
    for agent_id in AGENT_IDS {
        match adapter_for_read(agent_id)
            .and_then(|adapter| ingest_source(&store, agent_id, adapter.as_ref()))
        {
            Ok(scan_synced_at) => synced_at = synced_at.max(scan_synced_at),
            Err(error) => require_archive(&store, agent_id, error)?,
        }
    }
    let mut raw = store.query(None, range, &prices)?;
    raw.synced_at = synced_at.max(LAST_SUCCESSFUL_SYNC.load(std::sync::atomic::Ordering::Relaxed));
    Ok(raw.finish())
}

#[cfg(test)]
pub(crate) fn prices() -> Pricing {
    Pricing::load(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../model-pricing.json"
    )))
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn ranges_are_milliseconds_and_all_includes_legacy_epoch_entries() {
        let now = 1_700_000_000_000;
        assert_eq!(
            UsageRange::parse("1h", now).unwrap().cutoff,
            now - 3_600_000
        );
        assert_eq!(
            UsageRange::parse("14d", now).unwrap().cutoff,
            now - 14 * 86_400_000
        );
        assert_eq!(UsageRange::parse("all", now).unwrap().cutoff, i64::MIN);
        assert!(UsageRange::parse("yesterday", now).is_err());
        for (duration, expected_bucket) in [
            (48 * 3_600_000, 3_600_000),
            (48 * 3_600_000 + 1, 86_400_000),
            (180 * 86_400_000, 86_400_000),
            (180 * 86_400_000 + 1, 7 * 86_400_000),
        ] {
            let range = UsageRange::parse(&format!("custom:0:{duration}"), now).unwrap();
            assert_eq!(range.bucket_ms, expected_bucket);
            assert_eq!(range.end_exclusive, Some(duration));
        }
    }

    #[test]
    fn custom_range_rejects_missing_reversed_nonnumeric_and_overflowing_bounds() {
        for value in [
            "custom:",
            "custom:1",
            "custom::2",
            "custom:1:",
            "custom:2:2",
            "custom:3:2",
            "custom:1:2:3",
            "custom:abc:2",
            "custom:-1:2",
            "custom:1:9223372036854775808",
            "custom:9223372036854775808:2",
        ] {
            assert!(
                UsageRange::parse(value, 1_700_000_000_000).is_err(),
                "{value}"
            );
        }
    }

    fn archived_record(
        external_id: &str,
        provider: &str,
        model: &str,
        timestamp: i64,
        total_tokens: i64,
    ) -> UsageRecord {
        UsageRecord {
            external_id: external_id.to_owned(),
            provider: provider.to_owned(),
            model: model.to_owned(),
            timestamp,
            input_tokens: 1,
            output_tokens: 2,
            cache_read_tokens: 3,
            cache_write_tokens: 0,
            total_tokens,
        }
    }

    #[test]
    fn the_archive_merges_every_source_in_one_query_per_model() {
        let root = env::temp_dir().join(format!("amc-archive-merge-{}", uuid::Uuid::new_v4()));
        let store = store::UsageStore::new(&root).unwrap();
        let timestamp = 1_700_000_000_000;
        store
            .ingest(
                "omp",
                &[archived_record(
                    "omp-1",
                    "zhipu-coding-plan",
                    "glm-5.3",
                    timestamp,
                    6,
                )],
            )
            .unwrap();
        // Codex names no provider; the catalog resolves it at query time.
        store
            .ingest(
                "codex",
                &[archived_record(
                    "codex-1",
                    "",
                    "gpt-6-sol",
                    timestamp,
                    40,
                )],
            )
            .unwrap();
        let prices = prices();
        let all = store
            .query(
                None,
                UsageRange::parse("all", timestamp + 1).unwrap(),
                &prices,
            )
            .unwrap()
            .finish();
        assert_eq!(all.total_requests, 2);
        assert_eq!(all.total_tokens, 46);
        assert_eq!(all.by_model.len(), 2);
        assert_eq!(all.by_model[0].provider, "openai");
        assert_eq!(all.by_model[0].model, "gpt-6-sol");
        assert_eq!(all.by_model[1].provider, "zhipu-coding-plan");
        assert_eq!(all.trend.len(), 1);
        assert_eq!(all.trend[0].requests, 2);
        // A single-agent view filters the archive by source.
        let omp = store
            .query(
                Some("omp"),
                UsageRange::parse("all", timestamp + 1).unwrap(),
                &prices,
            )
            .unwrap()
            .finish();
        assert_eq!(omp.total_requests, 1);
        assert_eq!(omp.total_tokens, 6);
        fs::remove_dir_all(root).unwrap();
    }

    // The point of the archive: once a record is in, deleting the agent's
    // own files must not take the statistics with it. Uses the public read
    // path against a Codex home that is removed between the two reads.
    #[test]
    fn usage_survives_the_agent_deleting_its_source_files() {
        // 本测试临时改写全局 CODEX_HOME，必须与其它依赖环境变量的测试互斥。
        let _env_guard = crate::test_support::env_lock();
        let home = env::temp_dir().join(format!("amc-survive-src-{}", uuid::Uuid::new_v4()));
        let data_dir = env::temp_dir().join(format!("amc-survive-data-{}", uuid::Uuid::new_v4()));
        let day = home.join("sessions").join("2026").join("07").join("13");
        fs::create_dir_all(&day).unwrap();
        fs::write(
            day.join("rollout.jsonl"),
            r#"{"timestamp":"2026-07-13T08:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":50},"total_token_usage":{"input_tokens":1000,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":50}}}}"#,
        )
        .unwrap();
        let price_file = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../model-pricing.json"
        ));
        // Only this test touches CODEX_HOME; the adapter tests construct
        // their adapters from explicit paths.
        env::set_var("CODEX_HOME", &home);
        let first = get_agent_usage("codex", "all", &data_dir, price_file).unwrap();
        assert_eq!(first.total_requests, 1);
        assert_eq!(first.total_tokens, 1050);
        fs::remove_dir_all(&home).unwrap();
        let second = get_agent_usage("codex", "all", &data_dir, price_file).unwrap();
        assert_eq!(second.total_requests, 1);
        assert_eq!(second.total_tokens, 1050);
        // A fresh archive without the source is still an error.
        let empty_dir =
            env::temp_dir().join(format!("amc-survive-empty-{}", uuid::Uuid::new_v4()));
        let error = match get_agent_usage("codex", "all", &empty_dir, price_file) {
            Err(error) => error,
            Ok(_) => panic!("没有源也没有归档时应当报错"),
        };
        assert!(error.contains("找不到 Codex"), "{error}");
        env::remove_var("CODEX_HOME");
        fs::remove_dir_all(data_dir).unwrap();
        fs::remove_dir_all(empty_dir).unwrap();
    }
}
