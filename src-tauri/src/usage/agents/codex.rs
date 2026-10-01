// Codex usage: one rollout JSONL per session under `$CODEX_HOME/sessions/`;
// `token_count` events carry per-turn usage, so no CLI sync step exists.
#[cfg(test)]
use crate::usage::prices;
use crate::usage::{
    collect_jsonl, modified_millis, rfc3339_millis, AgentUsageAdapter, SourceScan, UsageRecord,
};
use crate::platform;
use serde::Serialize;
use std::{collections::HashSet, env, fs, path::PathBuf};

/// Raw token buckets of one Codex turn. `input` counts cached and
/// cache-written tokens (the CLI's own `non_cached_input` subtracts them
/// from it); `output` includes reasoning tokens.
#[derive(Clone, Copy, Default, PartialEq)]
struct CodexTokens {
    input: i64,
    cached: i64,
    write: i64,
    output: i64,
}

fn codex_tokens(value: &serde_json::Value) -> CodexTokens {
    let field = |name: &str| value.get(name).and_then(serde_json::Value::as_i64);
    CodexTokens {
        input: field("input_tokens").unwrap_or(0),
        cached: field("cached_input_tokens").unwrap_or(0),
        write: field("cache_write_input_tokens").unwrap_or(0),
        output: field("output_tokens").unwrap_or(0),
    }
}

/// Codex CLI appends one rollout JSONL per session under
/// `$CODEX_HOME/sessions/YYYY/MM/DD/` (default config root `~/.codex`).
/// Every completed turn emits an `event_msg` of type `token_count` with the
/// turn's `last_token_usage`, so there is no CLI sync step — scanning feeds
/// the archive, which deduplicates the copies that forks produce.
pub(crate) struct CodexUsageAdapter {
    sessions_dir: PathBuf,
}

impl CodexUsageAdapter {
    pub(crate) fn from_environment() -> platform::Result<Self> {
        let home = platform::home()?;
        Self::from_base(
            env::var("CODEX_HOME")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map_or_else(|| home.join(".codex"), PathBuf::from),
        )
    }

    fn from_base(base: PathBuf) -> platform::Result<Self> {
        let sessions_dir = base.join("sessions");
        if !sessions_dir.is_dir() {
            return Err(format!(
                "找不到 Codex 数据目录 {}；请确认已安装并使用过 Codex",
                sessions_dir.display()
            ));
        }
        Ok(Self { sessions_dir })
    }

    fn transcripts(&self) -> platform::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        // Rollouts nest as sessions/YYYY/MM/DD/rollout-*.jsonl.
        collect_jsonl("Codex", &self.sessions_dir, &mut files)?;
        files.sort();
        Ok(files)
    }

    fn scan_usage(&self, since: i64) -> platform::Result<SourceScan> {
        let mut records = Vec::new();
        let mut synced_at = 0i64;
        // An event is archived once across every file: copied fork prefixes
        // repeat the parent's `(turn_id, payload)` pairs verbatim.
        let mut seen_events: HashSet<String> = HashSet::new();
        for file in self.transcripts()? {
            let modified = modified_millis(&file);
            if let Some(value) = modified {
                synced_at = synced_at.max(value);
            }
            // A rollout last written before the archive's high-water mark
            // cannot hold a newer record, so it is skipped without parsing.
            if modified.is_some_and(|value| value < since) {
                continue;
            }
            let contents = fs::read_to_string(&file)
                .map_err(|e| format!("读取 Codex 转录文件 {} 失败: {e}", file.display()))?;
            let mut model = String::new();
            let mut previous = CodexTokens::default();
            let mut thread_id = String::new();
            let mut fork_lineage = false;
            let mut boundary_reached = false;
            let mut current_turn = Option::<String>::None;
            for line in contents.lines() {
                // Only model context (`session_meta`, `turn_context`), usage
                // summaries (`token_count`) and thread ownership markers
                // (`thread_settings_applied`) matter; everything else is
                // conversation or tool traffic.
                if !line.contains("\"token_count\"")
                    && !line.contains("\"type\":\"turn_context\"")
                    && !line.contains("\"type\":\"session_meta\"")
                    && !line.contains("\"type\":\"thread_settings_applied\"")
                {
                    continue;
                }
                let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                let entry_type = entry
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let payload = entry.get("payload");
                match entry_type {
                    "session_meta" => {
                        // Startup model; each turn restates its own below.
                        if let Some(startup) = payload
                            .and_then(|payload| {
                                payload.pointer("/base_instructions/provenance/model")
                            })
                            .and_then(serde_json::Value::as_str)
                            .filter(|startup| !startup.is_empty())
                        {
                            model = startup.to_owned();
                        }
                        // Own thread id and the fork lineage marker: copied
                        // forks (`ForkPersistence::Copied`) persist the
                        // parent rollout prefix into this file.
                        let field = |name: &str| {
                            payload
                                .and_then(|payload| payload.get(name))
                                .and_then(serde_json::Value::as_str)
                                .filter(|value| !value.is_empty())
                                .map(str::to_owned)
                        };
                        if let Some(id) = field("id").or_else(|| field("session_id")) {
                            thread_id = id;
                        }
                        fork_lineage = field("forked_from_id").is_some();
                    }
                    "turn_context" => {
                        // `/model` switches emit a new turn context
                        // mid-file; `model_slug` is the oldest key. The
                        // `turn_id` names the logical turn, and copied fork
                        // prefixes keep the parent's ids.
                        if let Some(turn_model) = ["model", "model_slug"]
                            .iter()
                            .filter_map(|key| payload.and_then(|payload| payload.get(key)))
                            .find_map(|value| value.as_str())
                            .filter(|turn_model| !turn_model.is_empty())
                        {
                            model = turn_model.to_owned();
                        }
                        current_turn = payload
                            .and_then(|payload| payload.get("turn_id"))
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned);
                    }
                    "event_msg" => {
                        let payload_type = payload
                            .and_then(|payload| payload.get("type"))
                            .and_then(serde_json::Value::as_str);
                        if payload_type == Some("thread_settings_applied") {
                            // Copied snapshots retain their original owner's
                            // id; the fork itself appends one owned by this
                            // thread, marking the end of the inherited
                            // prefix.
                            if let Some(owner) = payload
                                .and_then(|payload| payload.get("thread_id"))
                                .and_then(serde_json::Value::as_str)
                            {
                                if owner == thread_id {
                                    boundary_reached = true;
                                }
                            }
                            continue;
                        }
                        if payload_type != Some("token_count") {
                            continue;
                        }
                        let Some(timestamp) = payload
                            .and_then(|_| entry.get("timestamp"))
                            .and_then(serde_json::Value::as_str)
                            .and_then(rfc3339_millis)
                        else {
                            continue;
                        };
                        // `info` is null on events carrying only rate limits.
                        let Some(info) = payload
                            .and_then(|payload| payload.get("info"))
                            .filter(|info| info.is_object())
                        else {
                            continue;
                        };
                        // Rollouts carry the session-cumulative
                        // `total_token_usage`, so a snapshot equal to the
                        // running total is a rate-limit re-broadcast (the
                        // CLI re-emits current state from
                        // `update_rate_limits`), not a new turn. The turn
                        // itself is `last_token_usage`: a forked session
                        // seeds its state with the parent's token info
                        // (`InitialHistory::Forked`), so its first
                        // snapshot's total includes the parent baseline
                        // while `last` is this response only. Cumulative
                        // deltas only fill in for rollouts that lack
                        // `last_token_usage` (oldest format, where `info`
                        // itself is the cumulative struct).
                        let last = info
                            .get("last_token_usage")
                            .filter(|usage| usage.is_object());
                        let cumulative = info
                            .get("total_token_usage")
                            .filter(|usage| usage.is_object())
                            .or_else(|| last.is_none().then_some(info));
                        if cumulative.map(codex_tokens) == Some(previous) {
                            continue;
                        }
                        let turn = match (last.map(codex_tokens), cumulative.map(codex_tokens)) {
                            (Some(turn), cumulative) => {
                                if let Some(current) = cumulative {
                                    previous = current;
                                }
                                turn
                            }
                            (None, Some(current)) => {
                                let delta = CodexTokens {
                                    input: current.input.saturating_sub(previous.input),
                                    cached: current.cached.saturating_sub(previous.cached),
                                    write: current.write.saturating_sub(previous.write),
                                    output: current.output.saturating_sub(previous.output),
                                };
                                previous = current;
                                delta
                            }
                            (None, None) => continue,
                        };
                        // Inside a copied fork the events before the
                        // thread's own settings marker are the inherited
                        // parent prefix; they belong to the parent thread
                        // and are counted from the parent's own rollout
                        // (or not at all when that rollout predates the
                        // range).
                        if fork_lineage && !boundary_reached {
                            continue;
                        }
                        let payload_key = info.to_string();
                        // A turn's events are archived once across every
                        // file: copied fork prefixes repeat the parent's
                        // events verbatim (`turn_id` and payload both survive
                        // the copy), while genuine turns — including a
                        // parent's own post-fork turns — carry their own id
                        // or values. The payload is part of the key because
                        // one turn can contain several API responses, each
                        // with its own `last_token_usage` delta under the
                        // same `turn_id`. The key doubles as the archive
                        // identity; rollouts without turn contexts fall back
                        // to the event's own content below.
                        let event_key = current_turn
                            .as_ref()
                            .map(|turn_id| format!("{turn_id}\u{1}{payload_key}"));
                        let seen_event =
                            event_key
                                .as_ref()
                                .is_some_and(|key| !seen_events.insert(key.clone()));
                        if seen_event {
                            continue;
                        }
                        let turn = CodexTokens {
                            input: turn.input.max(0),
                            cached: turn.cached.max(0),
                            write: turn.write.max(0),
                            output: turn.output.max(0),
                        };
                        if turn.input + turn.cached + turn.write + turn.output <= 0 {
                            continue;
                        }
                        let model = if model.is_empty() {
                            "unknown"
                        } else {
                            model.as_str()
                        };
                        let input = (turn.input - turn.cached - turn.write).max(0);
                        records.push(UsageRecord {
                            // The provider stays empty: rollouts only name
                            // the model; the catalog attributes it (and
                            // prices it) at query time.
                            provider: String::new(),
                            external_id: event_key.unwrap_or_else(|| {
                                // Old rollouts without turn contexts were
                                // never deduplicated, so their identity
                                // includes the file to keep distinct
                                // occurrences distinct while staying
                                // idempotent across rescans.
                                format!(
                                    "{}\u{1}{thread_id}\u{1}{timestamp}\u{1}{payload_key}",
                                    file.display()
                                )
                            }),
                            model: model.to_owned(),
                            timestamp,
                            input_tokens: input,
                            output_tokens: turn.output,
                            cache_read_tokens: turn.cached,
                            cache_write_tokens: turn.write,
                            total_tokens: input + turn.cached + turn.write + turn.output,
                        });
                    }
                    _ => {}
                }
            }
        }
        Ok(SourceScan { records, synced_at })
    }
}

impl AgentUsageAdapter for CodexUsageAdapter {
    fn scan(&self, since: i64) -> platform::Result<SourceScan> {
        self.scan_usage(since)
    }
}

/// Claude Code CLI availability for the Agents overview; mirrors

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexStatus {
    pub installed: bool,
    pub version: String,
}

pub fn codex_status() -> CodexStatus {
    let executable = if cfg!(windows) { "codex.exe" } else { "codex" };
    let status = platform::cli_status(executable, &[".local/bin", ".codex/bin"]);
    CodexStatus {
        installed: status.installed,
        version: status.version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Scans the adapter's source into a fresh archive under `root` and
    // aggregates the requested range — the exact production flow.
    fn archive_and_query(
        adapter: &CodexUsageAdapter,
        root: &std::path::Path,
        range: &str,
        prices: &crate::pricing::Pricing,
    ) -> crate::usage::UsageStats {
        let store = crate::usage::store::UsageStore::new(root).unwrap();
        let scan = adapter.scan(0).unwrap();
        store.ingest("codex", &scan.records).unwrap();
        store
            .query(
                Some("codex"),
                crate::usage::UsageRange::parse(range, crate::usage::now_millis().unwrap())
                    .unwrap(),
                prices,
            )
            .unwrap()
            .finish()
    }

    #[test]
    fn codex_rollouts_attribute_turns_and_fall_back_to_cumulative_deltas() {
        let root = env::temp_dir().join(format!("amc-codex-{}", uuid::Uuid::new_v4()));
        let day = root.join("sessions").join("2026").join("07").join("13");
        fs::create_dir_all(&day).unwrap();
        let start = 1_760_000_000_000_i64;
        let end = start + 86_400_000;
        let stamp = |offset: i64| {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(start + offset)
                .unwrap()
                .to_rfc3339()
        };
        let token_count = |timestamp: String, info: &str| {
            format!(
                r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"token_count","info":{info}}}}}"#
            )
        };
        let usage = |last: (i64, i64, i64, i64), total: (i64, i64, i64, i64)| {
            format!(
                r#"{{"last_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"cache_write_input_tokens":{},"output_tokens":{}}},"total_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"cache_write_input_tokens":{},"output_tokens":{}}}}}"#,
                last.0, last.1, last.2, last.3, total.0, total.1, total.2, total.3
            )
        };
        let totals = |input: i64, cached: i64, write: i64, output: i64| {
            format!(
                r#"{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"cache_write_input_tokens":{write},"output_tokens":{output}}}}}"#
            )
        };
        let meta = r#"{"timestamp":"2026-07-13T08:00:00.000Z","type":"session_meta","payload":{"session_id":"s","base_instructions":{"provenance":{"model":"glm-5.3"}}}}"#;
        let turn_context = |timestamp: String, model: &str| {
            format!(
                r#"{{"timestamp":"{timestamp}","type":"turn_context","payload":{{"model":"{model}"}}}}"#
            )
        };
        let entries = [
            // The startup model prices turns until the first turn context.
            meta.to_owned(),
            turn_context(stamp(0), "gpt-5.6-terra"),
            // A pre-cutoff snapshot only sets the cumulative baseline.
            token_count(stamp(-1), &totals(5_000, 4_000, 0, 200)),
            // In-range turn attributed to the current turn context; the
            // cumulative series advances by the reported turn delta.
            token_count(stamp(1000), &usage((10_000, 8_000, 0, 500), (15_000, 12_000, 0, 700))),
            // Rate-limit-only events carry null `info` and are skipped.
            token_count(stamp(1100), "null"),
            // Oldest rollouts only carry session-cumulative totals; the
            // turn delta is the difference from the previous total.
            token_count(stamp(2000), &totals(35_000, 24_000, 0, 1_700)),
            // A rate-limit update re-broadcasts the unchanged usage and
            // must not count again.
            token_count(stamp(2100), &usage((20_000, 12_000, 0, 1_000), (35_000, 24_000, 0, 1_700))),
            // A mid-session `/model` switch re-prices the next turns.
            turn_context(stamp(3000), "glm-5.3"),
            token_count(stamp(4000), &usage((2_000, 1_000, 0, 100), (37_000, 25_000, 0, 1_800))),
            // The end boundary is exclusive but still advances the
            // baseline for later turns.
            token_count(stamp(86_400_000), &totals(38_000, 25_500, 0, 1_850)),
            // Zero-token notices are not turns (and repeat the total).
            token_count(stamp(4100), &usage((0, 0, 0, 0), (38_000, 25_500, 0, 1_850))),
            // Turns before any model context resolve to the CLI vendor.
            token_count(stamp(5000), &usage((20, 0, 0, 4), (38_020, 25_500, 0, 1_854))),
            // Conversation and tool traffic carries no usage and is skipped
            // by the line prefilter.
            r#"{"timestamp":"2026-07-13T08:00:01.000Z","type":"response_item","payload":{"type":"message"}}"#.to_owned(),
        ];
        fs::write(day.join("rollout-a.jsonl"), entries.join("\n")).unwrap();
        // A rollout with an explicit turn context prices that model.
        fs::write(
            day.join("rollout-b.jsonl"),
            [
                turn_context(stamp(0), "gpt-5-codex"),
                token_count(stamp(6000), &usage((30, 0, 0, 6), (30, 0, 0, 6))),
            ]
            .join("\n"),
        )
        .unwrap();
        // A rollout without any model context falls back to the CLI
        // vendor's provider and stays unpriced.
        fs::write(
            day.join("rollout-c.jsonl"),
            token_count(stamp(6000), &usage((30, 0, 0, 6), (30, 0, 0, 6))),
        )
        .unwrap();
        let adapter = CodexUsageAdapter::from_base(root.clone()).unwrap();
        let prices = prices();
        let stats = archive_and_query(
            &adapter,
            &root,
            &format!("custom:{start}:{end}"),
            &prices,
        );
        // 6 turns: two gpt-5.6-terra, two glm-5.3 (the switched model keeps
        // pricing following turns), one gpt-5-codex, one unknown-model.
        assert_eq!(stats.total_requests, 6);
        assert_eq!(stats.input_tokens, 2_000 + 8_000 + 1_000 + 20 + 30 + 30);
        assert_eq!(stats.output_tokens, 500 + 1_000 + 100 + 4 + 6 + 6);
        assert_eq!(stats.cache_read_tokens, 8_000 + 12_000 + 1_000);
        assert_eq!(
            stats.total_tokens,
            stats.input_tokens + stats.cache_read_tokens + stats.output_tokens
        );
        let terra = |input: i64, cached: i64, output: i64| {
            prices
                .cost("openai", "gpt-5.6-terra", input, cached, 0, output)
                .unwrap()
        };
        let glm_provider = prices.primary_provider("glm-5.3").unwrap().to_owned();
        let glm = |input: i64, cached: i64, output: i64| {
            prices
                .cost(&glm_provider, "glm-5.3", input, cached, 0, output)
                .unwrap()
        };
        let codex = prices.cost("openai", "gpt-5-codex", 30, 0, 0, 6).unwrap();
        // The unknown-model turn is unpriced; the blended total sums only
        // the priced turns.
        assert_eq!(stats.unpriced_requests, 1);
        let terra_cost = terra(2_000, 8_000, 500) + terra(8_000, 12_000, 1_000);
        let glm_cost = glm(1_000, 1_000, 100) + glm(20, 0, 4);
        assert!((stats.total_cost.unwrap() - (terra_cost + glm_cost + codex)).abs() < 1e-12);
        let by_model: Vec<(String, String, i64, Option<f64>)> = stats
            .by_model
            .iter()
            .map(|m| (m.provider.clone(), m.model.clone(), m.requests, m.cost))
            .collect();
        let model = |name: &str| {
            by_model
                .iter()
                .find(|entry| entry.1 == name)
                .cloned()
                .unwrap()
        };
        let terra_row = model("gpt-5.6-terra");
        assert_eq!((terra_row.0.as_str(), terra_row.2), ("openai", 2));
        assert!((terra_row.3.unwrap() - terra_cost).abs() < 1e-12);
        let glm_row = model("glm-5.3");
        assert_eq!((glm_row.0, glm_row.2), (glm_provider, 2));
        assert!((glm_row.3.unwrap() - glm_cost).abs() < 1e-12);
        let codex_row = model("gpt-5-codex");
        assert_eq!((codex_row.0.as_str(), codex_row.2), ("openai", 1));
        assert!((codex_row.3.unwrap() - codex).abs() < 1e-12);
        let unknown_row = model("unknown");
        assert_eq!((unknown_row.0.as_str(), unknown_row.2), ("openai", 1));
        assert!(unknown_row.3.is_none());
    }

    #[test]
    fn codex_adapter_reports_missing_sessions_directory() {
        let root = env::temp_dir().join(format!("amc-codex-missing-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let error = CodexUsageAdapter::from_base(root.clone()).err().unwrap();
        assert!(error.contains("找不到 Codex 数据目录"), "{error}");
    }

    // `update_rate_limits` re-emits the unchanged token info next to fresh
    // rate limits; those snapshots must not add turns, tokens or cost.
    #[test]
    fn codex_rate_limit_snapshots_are_not_double_counted() {
        let root = env::temp_dir().join(format!("amc-codex-dup-{}", uuid::Uuid::new_v4()));
        let day = root.join("sessions").join("2026").join("07").join("13");
        fs::create_dir_all(&day).unwrap();
        let start = 1_760_000_000_000_i64;
        let stamp = |offset: i64| {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(start + offset)
                .unwrap()
                .to_rfc3339()
        };
        let info = r#"{"last_token_usage":{"input_tokens":20000,"cached_input_tokens":12000,"cache_write_input_tokens":0,"output_tokens":1000},"total_token_usage":{"input_tokens":35000,"cached_input_tokens":24000,"cache_write_input_tokens":0,"output_tokens":1700}}"#;
        let rollout = [
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{info}}}}}"#,
                stamp(1000)
            ),
            // The identical info re-broadcast next to new rate limits.
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{info},"rate_limits":{{"limit_id":"codex"}}}}}}"#,
                stamp(1100)
            ),
            // A following real turn still counts.
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":100,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":10}},"total_token_usage":{{"input_tokens":35100,"cached_input_tokens":24000,"cache_write_input_tokens":0,"output_tokens":1710}}}}}}}}"#,
                stamp(2000)
            ),
        ];
        fs::write(day.join("rollout.jsonl"), rollout.join("\n")).unwrap();
        let adapter = CodexUsageAdapter::from_base(root.clone()).unwrap();
        let stats = archive_and_query(
            &adapter,
            &root,
            &format!("custom:0:{}", start + 86_400_000),
            &prices(),
        );
        assert_eq!(stats.total_requests, 2);
        // Each turn counts its `last_token_usage`; the re-broadcast adds
        // nothing.
        assert_eq!(stats.input_tokens, 8_000 + 100);
        assert_eq!(stats.output_tokens, 1_000 + 10);
        assert_eq!(stats.cache_read_tokens, 12_000);
    }

    // `InitialHistory::Forked` seeds the child session with the parent's
    // token info: the first snapshot's `total_token_usage` includes the
    // parent baseline, while `last_token_usage` is this response only.
    #[test]
    fn codex_forked_sessions_count_child_turns_not_inherited_baseline() {
        let root = env::temp_dir().join(format!("amc-codex-fork-{}", uuid::Uuid::new_v4()));
        let day = root.join("sessions").join("2026").join("07").join("13");
        fs::create_dir_all(&day).unwrap();
        let start = 1_760_000_000_000_i64;
        let stamp = |offset: i64| {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(start + offset)
                .unwrap()
                .to_rfc3339()
        };
        let token_count = |timestamp: String,
                           last: (i64, i64, i64, i64),
                           total: (i64, i64, i64, i64)| {
            format!(
                r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"cache_write_input_tokens":{},"output_tokens":{}}},"total_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"cache_write_input_tokens":{},"output_tokens":{}}}}}}}}}"#,
                last.0, last.1, last.2, last.3, total.0, total.1, total.2, total.3
            )
        };
        let rollout = [
            // First child turn on top of the parent baseline.
            token_count(
                stamp(1000),
                (1_000, 500, 0, 100),
                (50_000, 40_000, 0, 3_000),
            ),
            // Rate-limit re-broadcast of the unchanged usage.
            token_count(
                stamp(1100),
                (1_000, 500, 0, 100),
                (50_000, 40_000, 0, 3_000),
            ),
            // The second turn advances past the seeded baseline.
            token_count(stamp(2000), (200, 0, 0, 20), (50_200, 40_000, 0, 3_020)),
        ];
        fs::write(day.join("rollout.jsonl"), rollout.join("\n")).unwrap();
        let adapter = CodexUsageAdapter::from_base(root.clone()).unwrap();
        let stats = archive_and_query(
            &adapter,
            &root,
            &format!("custom:{start}:{}", start + 86_400_000),
            &prices(),
        );
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.input_tokens, 500 + 200);
        assert_eq!(stats.cache_read_tokens, 500);
        assert_eq!(stats.output_tokens, 100 + 20);
        assert_eq!(
            stats.total_tokens,
            stats.input_tokens + stats.cache_read_tokens + stats.output_tokens
        );
    }

    // `ForkPersistence::Copied` (non-paginated) persists the parent rollout
    // prefix into the child file: parent history first, then the child's own
    // turns. The parent may keep running after the fork, and a child turn can
    // then carry token counts identical to a later parent turn — `turn_id`
    // identity, not payload values, must tell them apart. Scanning both files
    // must count the inherited prefix once, the parent's own turns, and the
    // child's own turns.
    #[test]
    fn codex_copied_fork_prefix_is_not_counted_twice() {
        let root = env::temp_dir().join(format!("amc-codex-copy-{}", uuid::Uuid::new_v4()));
        let day = root.join("sessions").join("2026").join("07").join("13");
        fs::create_dir_all(&day).unwrap();
        let start = 1_760_000_000_000_i64;
        let stamp = |offset: i64| {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(start + offset)
                .unwrap()
                .to_rfc3339()
        };
        let session_meta = |id: &str, forked_from: Option<&str>| {
            let inherited = forked_from
                .map(|parent| format!(r#","forked_from_id":"{parent}""#))
                .unwrap_or_default();
            format!(
                r#"{{"timestamp":"{}","type":"session_meta","payload":{{"session_id":"{id}","id":"{id}","base_instructions":{{"provenance":{{"model":"gpt-5.6-terra"}}}}{inherited}}}}}"#,
                stamp(0)
            )
        };
        let turn_context = |timestamp: String, turn_id: &str| {
            format!(
                r#"{{"timestamp":"{timestamp}","type":"turn_context","payload":{{"turn_id":"{turn_id}","model":"gpt-5.6-terra"}}}}"#
            )
        };
        let token_count = |timestamp: String,
                           last: (i64, i64, i64, i64),
                           total: (i64, i64, i64, i64)| {
            format!(
                r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"cache_write_input_tokens":{},"output_tokens":{}}},"total_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"cache_write_input_tokens":{},"output_tokens":{}}}}}}}}}"#,
                last.0, last.1, last.2, last.3, total.0, total.1, total.2, total.3
            )
        };
        let settings_applied = |timestamp: String, thread: &str| {
            format!(
                r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"thread_settings_applied","thread_id":"{thread}"}}}}"#
            )
        };
        let parent_turn = (1_000, 500, 0, 100);
        let parent_turn_total = (1_000, 500, 0, 100);
        // Identical token counts on the parent's post-fork turn and the
        // child's own turn: payload deduplication would drop one.
        let later_turn = (300, 0, 0, 30);
        let later_turn_total = (1_300, 500, 0, 130);
        // Parent rollout: the turn the child inherits, then a turn produced
        // after the fork.
        fs::write(
            day.join("rollout-2026-07-13T08-00-00-aaaaaaaa-1111-2222-3333-444444444444.jsonl"),
            [
                session_meta("parent-thread", None),
                turn_context(stamp(1000), "turn-parent-inherited"),
                token_count(stamp(1100), parent_turn, parent_turn_total),
                turn_context(stamp(2000), "turn-parent-late"),
                token_count(stamp(2100), later_turn, later_turn_total),
            ]
            .join("\n"),
        )
        .unwrap();
        // Child rollout: the parent prefix copied verbatim (same payloads,
        // same parent turn ids, re-stamped timestamps) followed by the
        // fork's own settings marker and the child's own turn.
        fs::write(
            day.join("rollout-2026-07-13T09-00-00-bbbbbbbb-1111-2222-3333-444444444444.jsonl"),
            [
                session_meta("child-thread", Some("parent-thread")),
                turn_context(stamp(3600), "turn-parent-inherited"),
                token_count(stamp(3650), parent_turn, parent_turn_total),
                // A copied snapshot keeps the parent's owner id and must not
                // end the inherited prefix.
                settings_applied(stamp(3700), "parent-thread"),
                settings_applied(stamp(3800), "child-thread"),
                turn_context(stamp(4000), "turn-child-own"),
                token_count(stamp(4100), later_turn, later_turn_total),
            ]
            .join("\n"),
        )
        .unwrap();
        let adapter = CodexUsageAdapter::from_base(root.clone()).unwrap();
        let stats = archive_and_query(
            &adapter,
            &root,
            &format!("custom:{start}:{}", start + 86_400_000),
            &prices(),
        );
        // The parent's two turns plus the child's own turn; the inherited
        // copy in the child file is skipped.
        assert_eq!(stats.total_requests, 3);
        assert_eq!(stats.input_tokens, 500 + 300 + 300);
        assert_eq!(stats.output_tokens, 100 + 30 + 30);
        assert_eq!(stats.cache_read_tokens, 500);
    }

    // The parent rollout can sit outside the range by mtime while the copied
    // fork child was written recently: its inherited prefix must still be
    // recognized without the parent file having been parsed.
    #[test]
    fn codex_copied_prefix_is_skipped_when_parent_file_is_stale() {
        let root = env::temp_dir().join(format!("amc-codex-stale-{}", uuid::Uuid::new_v4()));
        let day = root.join("sessions").join("2026").join("07").join("13");
        fs::create_dir_all(&day).unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        let stamp = |offset: i64| {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now + offset)
                .unwrap()
                .to_rfc3339()
        };
        let session_meta = |id: &str, forked_from: Option<&str>| {
            let inherited = forked_from
                .map(|parent| format!(r#","forked_from_id":"{parent}""#))
                .unwrap_or_default();
            format!(
                r#"{{"timestamp":"{}","type":"session_meta","payload":{{"session_id":"{id}","id":"{id}","base_instructions":{{"provenance":{{"model":"gpt-5.6-terra"}}}}{inherited}}}}}"#,
                stamp(-172_800_000)
            )
        };
        let turn_context = |timestamp: String, turn_id: &str| {
            format!(
                r#"{{"timestamp":"{timestamp}","type":"turn_context","payload":{{"turn_id":"{turn_id}","model":"gpt-5.6-terra"}}}}"#
            )
        };
        let token_count = |timestamp: String,
                           last: (i64, i64, i64, i64),
                           total: (i64, i64, i64, i64)| {
            format!(
                r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"cache_write_input_tokens":{},"output_tokens":{}}},"total_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"cache_write_input_tokens":{},"output_tokens":{}}}}}}}}}"#,
                last.0, last.1, last.2, last.3, total.0, total.1, total.2, total.3
            )
        };
        let settings_applied = |timestamp: String, thread: &str| {
            format!(
                r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"thread_settings_applied","thread_id":"{thread}"}}}}"#
            )
        };
        let parent_path =
            day.join("rollout-2026-07-11T08-00-00-aaaaaaaa-1111-2222-3333-444444444444.jsonl");
        fs::write(
            &parent_path,
            [
                session_meta("parent-thread", None),
                turn_context(stamp(-172_800_000), "turn-parent"),
                token_count(
                    stamp(-172_799_000),
                    (1_000, 500, 0, 100),
                    (1_000, 500, 0, 100),
                ),
            ]
            .join("\n"),
        )
        .unwrap();
        // The parent was last written two days ago; backdate its mtime past
        // the 24h cutoff.
        let stale = filetime::FileTime::from_unix_time((now - 172_800_000) / 1000, 0);
        filetime::set_file_mtime(&parent_path, stale).unwrap();
        // The child was forked minutes ago: fresh mtime, copied parent turn,
        // then its own turn inside the range.
        fs::write(
            day.join("rollout-2026-07-13T09-00-00-bbbbbbbb-1111-2222-3333-444444444444.jsonl"),
            [
                session_meta("child-thread", Some("parent-thread")),
                turn_context(stamp(-60_000), "turn-parent"),
                token_count(stamp(-59_000), (1_000, 500, 0, 100), (1_000, 500, 0, 100)),
                settings_applied(stamp(-58_000), "child-thread"),
                turn_context(stamp(-50_000), "turn-child"),
                token_count(stamp(-40_000), (200, 0, 0, 20), (1_200, 500, 0, 120)),
            ]
            .join("\n"),
        )
        .unwrap();
        let adapter = CodexUsageAdapter::from_base(root.clone()).unwrap();
        let stats = archive_and_query(&adapter, &root, "24h", &prices());
        // Only the child's own turn is in range; the inherited copy must not
        // resurrect the parent's out-of-range turn.
        assert_eq!(stats.total_requests, 1);
        assert_eq!(stats.input_tokens, 200);
        assert_eq!(stats.output_tokens, 20);
    }

    // One turn can contain several API responses, each emitting its own
    // `token_count` under the same `turn_id` with a different
    // `last_token_usage` delta; only verbatim repeats are duplicates.
    #[test]
    fn codex_multi_response_turns_keep_every_distinct_event() {
        let root = env::temp_dir().join(format!("amc-codex-multi-{}", uuid::Uuid::new_v4()));
        let day = root.join("sessions").join("2026").join("07").join("13");
        fs::create_dir_all(&day).unwrap();
        let start = 1_760_000_000_000_i64;
        let stamp = |offset: i64| {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(start + offset)
                .unwrap()
                .to_rfc3339()
        };
        let token_count = |timestamp: String,
                           last: (i64, i64, i64, i64),
                           total: (i64, i64, i64, i64)| {
            format!(
                r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"cache_write_input_tokens":{},"output_tokens":{}}},"total_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"cache_write_input_tokens":{},"output_tokens":{}}}}}}}}}"#,
                last.0, last.1, last.2, last.3, total.0, total.1, total.2, total.3
            )
        };
        let rollout = [
            format!(
                r#"{{"timestamp":"{}","type":"turn_context","payload":{{"turn_id":"turn-one","model":"gpt-5.6-terra"}}}}"#,
                stamp(0)
            ),
            // First response of the turn.
            token_count(
                stamp(1000),
                (10_000, 8_000, 0, 500),
                (10_000, 8_000, 0, 500),
            ),
            // Second response of the same turn: same `turn_id`, own delta.
            token_count(stamp(2000), (2_000, 1_000, 0, 100), (12_000, 9_000, 0, 600)),
            // Rate-limit re-broadcast of the unchanged totals.
            token_count(stamp(2100), (2_000, 1_000, 0, 100), (12_000, 9_000, 0, 600)),
        ];
        fs::write(day.join("rollout.jsonl"), rollout.join("\n")).unwrap();
        let adapter = CodexUsageAdapter::from_base(root.clone()).unwrap();
        let stats = archive_and_query(
            &adapter,
            &root,
            &format!("custom:{start}:{}", start + 86_400_000),
            &prices(),
        );
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.input_tokens, 2_000 + 1_000);
        assert_eq!(stats.output_tokens, 500 + 100);
        assert_eq!(stats.cache_read_tokens, 8_000 + 1_000);
    }

    // Snapshots before the cutoff must advance the cumulative baseline, or
    // the first cumulative-only in-range event would absorb the pre-cutoff
    // usage into its delta.
    #[test]
    fn codex_pre_cutoff_snapshots_set_the_delta_baseline() {
        let root = env::temp_dir().join(format!("amc-codex-base-{}", uuid::Uuid::new_v4()));
        let day = root.join("sessions").join("2026").join("07").join("13");
        fs::create_dir_all(&day).unwrap();
        let start = 1_760_000_000_000_i64;
        let stamp = |offset: i64| {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(start + offset)
                .unwrap()
                .to_rfc3339()
        };
        let totals = |timestamp: String, input: i64, cached: i64, output: i64| {
            format!(
                r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"cache_write_input_tokens":0,"output_tokens":{output}}}}}}}}}"#,
            )
        };
        let rollout = [
            totals(stamp(-3_600_000), 10_000, 8_000, 500),
            totals(stamp(-60_000), 12_000, 9_000, 600),
            totals(stamp(1000), 30_000, 24_000, 700),
        ];
        fs::write(day.join("rollout.jsonl"), rollout.join("\n")).unwrap();
        let adapter = CodexUsageAdapter::from_base(root.clone()).unwrap();
        let stats = archive_and_query(
            &adapter,
            &root,
            &format!("custom:{start}:{}", start + 86_400_000),
            &prices(),
        );
        assert_eq!(stats.total_requests, 1);
        // Only the delta across the cutoff: (30_000-12_000, 24_000-9_000, 700-600),
        // of which the uncached input is 18_000 - 15_000.
        assert_eq!(stats.input_tokens, 3_000);
        assert_eq!(stats.cache_read_tokens, 15_000);
        assert_eq!(stats.output_tokens, 100);
    }
}
