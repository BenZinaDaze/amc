// Agent usage collection: shared aggregation types, the adapter contract and
// the registry. One file per source lives beside it (`omp.rs`, `claude.rs`,
// `codex.rs`); a new agent registers in [`AGENT_IDS`] and implements
// [`AgentUsageAdapter`], nothing else.
use crate::{omp, platform, pricing::Pricing};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicI64},
    time::{SystemTime, UNIX_EPOCH},
};

trait AgentUsageAdapter {
    fn sync(&self, range: UsageRange, prices: &Pricing) -> platform::Result<RawUsage>;
    fn read(&self, range: UsageRange, prices: &Pricing) -> platform::Result<RawUsage>;
}

mod agents;

pub(crate) use agents::claude::{claude_code_status, ClaudeCodeStatus};
pub(crate) use agents::codex::{codex_status, CodexStatus};

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

    fn absorb(&mut self, other: &Totals) {
        self.requests += other.requests;
        self.total_tokens += other.total_tokens;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cost += other.cost;
        self.unpriced_requests += other.unpriced_requests;
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

    fn merge(&mut self, other: RawUsage) {
        self.totals.absorb(&other.totals);
        for (key, totals) in other.models {
            self.models.entry(key).or_default().absorb(&totals);
        }
        for (bucket, (requests, total_tokens)) in other.trend {
            let point = self.trend.entry(bucket).or_default();
            point.0 += requests;
            point.1 += total_tokens;
        }
        self.synced_at = self.synced_at.max(other.synced_at);
    }
}

fn merge_into(merged: &mut Option<RawUsage>, raw: RawUsage) {
    match merged {
        Some(merged) => merged.merge(raw),
        None => *merged = Some(raw),
    }
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
        } else if path.extension().is_some_and(|extension| extension == "jsonl") {
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
            if omp::omp_executable().is_none() {
                return Err("找不到 OMP 可执行文件；请安装 omp 后重试".into());
            }
            Ok(Box::new(OmpUsageAdapter::from_environment()?))
        }
        "claude-code" => Ok(Box::new(ClaudeCodeUsageAdapter::from_environment()?)),
        "codex" => Ok(Box::new(CodexUsageAdapter::from_environment()?)),
        _ => Err(format!("不支持的 Agent 用量来源: {agent_id}")),
    }
}

fn with_adapter(
    agent_id: &str,
    price_file: &Path,
    operation: impl FnOnce(&dyn AgentUsageAdapter, &Pricing) -> platform::Result<RawUsage>,
) -> platform::Result<UsageStats> {
    let raw = operation(adapter_for(agent_id)?.as_ref(), &Pricing::load(price_file)?)?;
    Ok(raw.finish())
}

/// Agents that cannot provide usage (e.g. not installed) are skipped as long
/// as at least one source succeeds; only a total failure is reported.
fn collect(
    price_file: &Path,
    operation: impl Fn(&dyn AgentUsageAdapter, &Pricing) -> platform::Result<RawUsage>,
) -> platform::Result<UsageStats> {
    let prices = Pricing::load(price_file)?;
    let mut merged: Option<RawUsage> = None;
    let mut first_error: Option<String> = None;
    for agent_id in AGENT_IDS {
        match adapter_for(agent_id).and_then(|adapter| operation(adapter.as_ref(), &prices)) {
            Ok(raw) => merge_into(&mut merged, raw),
            Err(error) => {
                let _ = first_error.get_or_insert(error);
            }
        }
    }
    match merged {
        Some(raw) => Ok(raw.finish()),
        None => Err(first_error
            .unwrap_or_else(|| "没有可统计的 Agent 用量来源".to_owned())),
    }
}

pub fn sync_agent_usage(agent_id: &str, range: &str, price_file: &Path) -> platform::Result<UsageStats> {
    let range = UsageRange::parse(range, now_millis()?)?;
    with_adapter(agent_id, price_file, |adapter, prices| {
        adapter.sync(range, prices)
    })
}

pub fn get_agent_usage(agent_id: &str, range: &str, price_file: &Path) -> platform::Result<UsageStats> {
    let range = UsageRange::parse(range, now_millis()?)?;
    with_adapter(agent_id, price_file, |adapter, prices| {
        adapter.read(range, prices)
    })
}

pub fn sync_agents_usage(range: &str, price_file: &Path) -> platform::Result<UsageStats> {
    let range = UsageRange::parse(range, now_millis()?)?;
    collect(price_file, |adapter, prices| adapter.sync(range, prices))
}

pub fn get_agents_usage(range: &str, price_file: &Path) -> platform::Result<UsageStats> {
    let range = UsageRange::parse(range, now_millis()?)?;
    collect(price_file, |adapter, prices| adapter.read(range, prices))
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
}
