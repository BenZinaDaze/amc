// Claude Code usage: one JSONL transcript per session under
// `$CLAUDE_CONFIG_DIR/projects/`; no CLI sync step exists.
#[cfg(test)]
use crate::usage::prices;
use crate::usage::{
    collect_jsonl, modified_millis, now_millis, rfc3339_millis, AgentUsageAdapter, RawUsage,
    Totals, UsageRange, LAST_SUCCESSFUL_SYNC,
};
use crate::{omp, pricing::Pricing};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    path::{PathBuf},
    sync::atomic::Ordering,
};

/// Claude Code CLI writes one JSONL transcript per session under
/// `$CLAUDE_CONFIG_DIR/projects/<project>/*.jsonl` (default config root
/// `~/.claude`); every API response lands as an assistant line carrying raw
/// token usage, so there is no CLI sync step — sync and read both scan the
/// files and only the reported sync time differs.
pub(crate) struct ClaudeCodeUsageAdapter {
    projects_dir: PathBuf,
}

impl ClaudeCodeUsageAdapter {
    pub(crate) fn from_environment() -> omp::Result<Self> {
        let home = omp::home()?;
        Self::from_base(
            env::var("CLAUDE_CONFIG_DIR")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map_or_else(|| home.join(".claude"), PathBuf::from),
        )
    }

    fn from_base(base: PathBuf) -> omp::Result<Self> {
        let projects_dir = base.join("projects");
        if !projects_dir.is_dir() {
            return Err(format!(
                "找不到 Claude Code 数据目录 {}；请确认已安装并使用过 Claude Code",
                projects_dir.display()
            ));
        }
        Ok(Self { projects_dir })
    }

    // Session transcripts sit at projects/<project>/*.jsonl; official
    // subagents live deeper at projects/<project>/<sessionId>/subagents/
    // agent-<id>.jsonl, so the walk must recurse below the project dir.
    fn transcripts(&self) -> omp::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        collect_jsonl("Claude Code", &self.projects_dir, &mut files)?;
        files.sort();
        Ok(files)
    }

    fn read_usage(&self, range: UsageRange, prices: &Pricing) -> omp::Result<RawUsage> {
        let mut totals = Totals::default();
        let mut models: BTreeMap<(String, String), Totals> = BTreeMap::new();
        let mut trend: BTreeMap<i64, (i64, i64)> = BTreeMap::new();
        // Retries, --resume and session forks rewrite the same API response
        // into several lines; message id plus request id identifies one.
        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut synced_at = 0i64;
        for file in self.transcripts()? {
            let modified = modified_millis(&file);
            if let Some(value) = modified {
                synced_at = synced_at.max(value);
            }
            // A transcript last written before the cutoff cannot contain
            // newer entries, so it is skipped without parsing.
            if modified.is_some_and(|value| value < range.cutoff) {
                continue;
            }
            let contents = fs::read_to_string(&file)
                .map_err(|e| format!("读取 Claude Code 转录文件 {} 失败: {e}", file.display()))?;
            for line in contents.lines() {
                // Most lines are user/tool records without usage blocks.
                if !line.contains("\"type\":\"assistant\"") || !line.contains("\"usage\"") {
                    continue;
                }
                let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                let Some(message) = entry.get("message") else {
                    continue;
                };
                let model = message
                    .get("model")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                // "<synthetic>" lines are local error notices, not API responses.
                if model.is_empty() || model == "<synthetic>" {
                    continue;
                }
                let Some(usage) = message.get("usage") else {
                    continue;
                };
                let token =
                    |field: &str| usage.get(field).and_then(serde_json::Value::as_i64).unwrap_or(0);
                let input = token("input_tokens");
                let output = token("output_tokens");
                let write = token("cache_creation_input_tokens");
                let read = token("cache_read_input_tokens");
                if input + output + write + read <= 0 {
                    continue;
                }
                let Some(timestamp) = entry
                    .get("timestamp")
                    .and_then(serde_json::Value::as_str)
                    .and_then(rfc3339_millis)
                else {
                    continue;
                };
                if timestamp < range.cutoff
                    || range.end_exclusive.is_some_and(|end| timestamp >= end)
                {
                    continue;
                }
                let id = message
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let request = entry
                    .get("requestId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if !id.is_empty() && !seen.insert((id.to_owned(), request.to_owned())) {
                    continue;
                }
                // Transcripts only name the model; routed models (GLM behind
                // Claude Code) resolve to their listed provider to price.
                let provider = prices.primary_provider(model).unwrap_or("anthropic");
                let cost = prices.cost(provider, model, input, read, write, output);
                let model_totals = models
                    .entry((provider.to_owned(), model.to_owned()))
                    .or_default();
                for item in [&mut totals, model_totals] {
                    item.requests += 1;
                    item.total_tokens += input + output + write + read;
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
                point.1 += input + output + write + read;
            }
        }
        Ok(RawUsage {
            totals,
            models,
            trend,
            synced_at,
        })
    }
}

impl AgentUsageAdapter for ClaudeCodeUsageAdapter {
    fn sync(&self, range: UsageRange, prices: &Pricing) -> omp::Result<RawUsage> {
        let mut raw = self.read_usage(range, prices)?;
        let now = now_millis()?;
        LAST_SUCCESSFUL_SYNC.store(now, Ordering::Relaxed);
        raw.synced_at = now;
        Ok(raw)
    }

    fn read(&self, range: UsageRange, prices: &Pricing) -> omp::Result<RawUsage> {
        let mut raw = self.read_usage(range, prices)?;
        raw.synced_at = raw
            .synced_at
            .max(LAST_SUCCESSFUL_SYNC.load(Ordering::Relaxed));
        Ok(raw)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCodeStatus {
    pub installed: bool,
    pub version: String,
}

pub fn claude_code_status() -> ClaudeCodeStatus {
    let executable = if cfg!(windows) { "claude.exe" } else { "claude" };
    let mut found = false;
    for directory in env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .chain(
            omp::home()
                .ok()
                .into_iter()
                .flat_map(|home| [home.join(".local/bin"), home.join(".claude/local")]),
        )
        .chain(
            ["/opt/homebrew/bin", "/usr/local/bin"]
                .into_iter()
                .map(PathBuf::from),
        )
    {
        let program = directory.join(executable);
        if !program.is_file() {
            continue;
        }
        found = true;
        if let Ok(mut command) = omp::omp_command(&program) {
            if let Ok(output) = command.arg("--version").output() {
                if output.status.success() {
                    return ClaudeCodeStatus {
                        installed: true,
                        version: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
                    };
                }
            }
        }
    }
    ClaudeCodeStatus {
        installed: found,
        version: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_transcripts_aggregate_dedupe_and_skip_non_api_entries() {
        let root = env::temp_dir().join(format!("amc-claude-{}", uuid::Uuid::new_v4()));
        let project = root.join("projects").join("-Users-demo");
        fs::create_dir_all(&project).unwrap();
        let start = 1_760_000_000_000_i64;
        let end = start + 86_400_000;
        let stamp = |offset: i64| {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(start + offset)
                .unwrap()
                .to_rfc3339()
        };
        let line = |timestamp: String, id: &str, request: &str, model: &str, usage: &str| {
            format!(
                r#"{{"parentUuid":null,"isSidechain":false,"type":"assistant","requestId":"{request}","timestamp":"{timestamp}","message":{{"id":"{id}","model":"{model}","usage":{usage}}}}}"#
            )
        };
        let sonnet_usage = r#"{"input_tokens":1000,"output_tokens":200,"cache_creation_input_tokens":300,"cache_read_input_tokens":4000}"#;
        let entries = [
            // In-range request that gets rewritten verbatim into a second
            // file by --resume; counted once.
            line(stamp(0), "msg_1", "req_1", "claude-sonnet-4-5-20250929", sonnet_usage),
            line(stamp(1000), "msg_1", "req_1", "claude-sonnet-4-5-20250929", sonnet_usage),
            // Local "<synthetic>" notices and zero-token records are not API calls.
            line(stamp(2000), "msg_2", "req_2", "<synthetic>", r#"{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}"#),
            // Range boundaries behave like the OMP query: cutoff inclusive, end exclusive.
            line(stamp(-1), "msg_3", "req_3", "claude-sonnet-4-5-20250929", sonnet_usage),
            line(stamp(86_400_000), "msg_5", "req_5", "claude-sonnet-4-5-20250929", sonnet_usage),
            line(stamp(86_399_999), "msg_4", "req_4", "claude-haiku-4-5-20251001", r#"{"input_tokens":10,"output_tokens":20,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}"#),
            // Entries without a timestamp cannot be bucketed.
            line(String::new(), "msg_6", "req_6", "claude-haiku-4-5-20251001", r#"{"input_tokens":10,"output_tokens":20,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}"#),
            // Routed non-Anthropic model resolves to its catalog provider.
            line(stamp(3000), "msg_7", "req_7", "glm-5.3-flash", r#"{"input_tokens":5000,"output_tokens":1000,"cache_creation_input_tokens":0,"cache_read_input_tokens":2000}"#),
        ];
        fs::write(project.join("session-a.jsonl"), entries.join("\n")).unwrap();
        fs::write(
            project.join("session-b.jsonl"),
            format!(
                r#"{{"type":"user","timestamp":"{}","message":{{"usage":{{"input_tokens":9}}}}}}
{}"#,
                stamp(0),
                line(stamp(0), "msg_1", "req_1", "claude-sonnet-4-5-20250929", sonnet_usage)
            ),
        )
        .unwrap();
        // Official subagent transcripts nest two levels below the project dir
        // and rewrite main-thread requests plus their own API calls.
        let subagents = project
            .join("11111111-2222-3333-4444-555555555555")
            .join("subagents");
        fs::create_dir_all(&subagents).unwrap();
        fs::write(
            subagents.join("agent-abc.jsonl"),
            format!(
                "{}\n{}",
                line(stamp(0), "msg_1", "req_1", "claude-sonnet-4-5-20250929", sonnet_usage),
                line(stamp(4000), "msg_8", "req_8", "claude-haiku-4-5-20251001", r#"{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}"#),
            ),
        )
        .unwrap();
        let adapter = ClaudeCodeUsageAdapter::from_base(root).unwrap();
        let range = UsageRange::parse(&format!("custom:{start}:{end}"), end + 1).unwrap();
        let stats = adapter.read_usage(range, &prices()).unwrap().finish();
        assert_eq!(stats.total_requests, 4);
        assert_eq!(stats.input_tokens, 6110);
        assert_eq!(stats.output_tokens, 1270);
        assert_eq!(stats.cache_read_tokens, 6000);
        assert_eq!(stats.total_tokens, 6110 + 1270 + 6000 + 300);
        let sonnet = (1000. * 3. + 200. * 15. + 300. * 3.75 + 4000. * 0.3) / 1_000_000.;
        let haiku = (110. * 1. + 70. * 5.) / 1_000_000.;
        let glm = (5000. * 0.15 + 1000. * 0.5 + 2000. * 0.03) / 1_000_000.;
        assert!((stats.total_cost.unwrap() - sonnet - haiku - glm).abs() < 1e-12);
        let models: Vec<(String, String)> = stats.by_model.iter().map(|m| (m.provider.clone(), m.model.clone())).collect();
        assert_eq!(models, vec![
            ("anthropic".into(), "claude-haiku-4-5-20251001".into()),
            ("anthropic".into(), "claude-sonnet-4-5-20250929".into()),
            ("zhipu-coding-plan".into(), "glm-5.3-flash".into()),
        ]);
        assert_eq!(stats.trend.len(), 2);
        assert_eq!(stats.trend[0].bucket, 1_759_996_800_000);
        assert_eq!(stats.trend[0].requests, 3);
        assert_eq!(stats.trend[0].total_tokens, 13650);
        assert_eq!(stats.trend[1].bucket, 1_760_083_200_000);
        assert_eq!(stats.trend[1].requests, 1);
        assert_eq!(stats.trend[1].total_tokens, 30);
    }

    #[test]
    fn claude_adapter_reports_missing_projects_directory() {
        let root = env::temp_dir().join(format!("amc-claude-missing-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let error = ClaudeCodeUsageAdapter::from_base(root).err().unwrap();
        assert!(error.contains("找不到 Claude Code 数据目录"), "{error}");
    }

}
