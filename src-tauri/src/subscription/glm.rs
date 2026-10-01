//! GLM Coding Plan provider (Z.ai 国际版 / 智谱 BigModel 中国).
//!
//! Three monitor endpoints per credential: `quota/limit` (credit bars),
//! `model-usage` (token totals plus the per-model split, windowed to the
//! weekly credit cycle) and `tool-usage` (MCP calls). The endpoints are
//! undocumented; [Z.ai's own usage plugin][plugin] is the reference.

use crate::omp::Result;
use chrono::{DateTime, Local, Timelike};
use serde::Deserialize;
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{MetricUsage, ProviderReport, QuotaDetail, QuotaUsage};
use super::store::StoredSubscription;

const ZAI_HOST: &str = "api.z.ai";
const BIGMODEL_HOST: &str = "open.bigmodel.cn";

struct GlmCredential {
    base_url: String,
    token: String,
}

/// Plug-in point for the vendor catalog; keeps token leakage out of errors.
pub(super) fn fetch_entry(entry: &StoredSubscription) -> Result<ProviderReport> {
    let credential = credential(entry)?;
    fetch_glm(&credential).map_err(|error| sanitize(error, &credential.token))
}

/// `zai` | `bigmodel` → monitor API host; rejects unknown values.
fn credential(entry: &StoredSubscription) -> Result<GlmCredential> {
    let host = match entry.platform.as_str() {
        "zai" => ZAI_HOST,
        "bigmodel" => BIGMODEL_HOST,
        _ => return Err("未知的平台类型".to_owned()),
    };
    Ok(GlmCredential {
        base_url: format!("https://{host}"),
        token: entry.key.clone(),
    })
}

fn sanitize(message: String, token: &str) -> String {
    if token.is_empty() {
        message
    } else {
        message.replace(token, "***")
    }
}

fn get_json(url: &str, token: &str) -> Result<Value> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let response = agent
        .get(url)
        .header("Authorization", token)
        .header("Accept", "application/json")
        .call()
        .map_err(|error| format!("请求 {url} 失败: {error}"))?;
    if response.status().as_u16() != 200 {
        let status = response.status().as_u16();
        let body = response.into_body().read_to_string().unwrap_or_default();
        let mut message = format!("接口返回 HTTP {status}");
        if !body.is_empty() {
            message.push_str(&format!(": {}", truncate(&body, 200)));
        }
        return Err(message);
    }
    response
        .into_body()
        .read_json::<Value>()
        .map_err(|error| format!("解析 {url} 响应失败: {error}"))
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_owned();
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

/// The monitor endpoints wrap payloads in `data` but tolerate the bare shape.
fn data_or_root(value: &Value) -> &Value {
    value.get("data").filter(|data| !data.is_null()).unwrap_or(value)
}

fn fetch_glm(credential: &GlmCredential) -> Result<ProviderReport> {
    let base = &credential.base_url;
    let token = &credential.token;
    let quota_root = get_json(&format!("{base}/api/monitor/usage/quota/limit"), token)?;
    let Some(quota_data) = quota_root.get("data").filter(|data| !data.is_null()) else {
        let message = quota_root
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let suffix = (!message.is_empty()).then(|| format!("：{message}")).unwrap_or_default();
        return Err(format!("配额接口未返回数据{suffix}"));
    };
    let quota: QuotaData =
        serde_json::from_value(quota_data.clone()).map_err(|e| format!("解析配额数据失败: {e}"))?;
    let quotas = quotas_from_data(&quota);
    if quotas.is_empty() {
        return Err("配额接口未返回有效的限额条目，请检查订阅是否有效".to_owned());
    }

    // Token stats follow the plan's weekly credit cycle when the quota response
    // exposes it, so totals line up with the reset bar instead of a rolling day.
    let weekly_start = weekly_cycle_start(&quota);
    let query = usage_window_query(weekly_start);
    let mut metrics = Vec::new();
    if let Ok(model_root) = get_json(
        &format!("{base}/api/monitor/usage/model-usage?{query}"),
        token,
    ) {
        metrics.extend(model_metrics(data_or_root(&model_root), weekly_start.is_some()));
    }
    if let Ok(tool_root) = get_json(
        &format!("{base}/api/monitor/usage/tool-usage?{query}"),
        token,
    ) {
        metrics.extend(tool_metrics(data_or_root(&tool_root)));
    }

    Ok(ProviderReport {
        plan: plan_label(&quota),
        quotas,
        metrics,
    })
}

// ------------------------------------------------------------ quota parsing

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct QuotaData {
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    plan_name: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    package_name: Option<String>,
    limits: Vec<RawLimit>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RawLimit {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    unit: Option<i64>,
    #[serde(default)]
    number: Option<i64>,
    #[serde(default)]
    percentage: Option<f64>,
    /// Total quota units; vendors report it as `usage` or `total`.
    #[serde(default)]
    usage: Option<i64>,
    #[serde(default)]
    total: Option<i64>,
    #[serde(default)]
    current_value: Option<i64>,
    #[serde(default)]
    remaining: Option<i64>,
    #[serde(default)]
    next_reset_time: Option<i64>,
    #[serde(default)]
    usage_details: Option<Vec<RawUsageDetail>>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RawUsageDetail {
    #[serde(default)]
    model_code: Option<String>,
    #[serde(default)]
    usage: Option<i64>,
}

fn plan_label(data: &QuotaData) -> Option<String> {
    [
        &data.plan_name,
        &data.plan,
        &data.plan_type,
        &data.package_name,
        &data.level,
    ]
    .into_iter()
    .flatten()
    .find(|value| !value.trim().is_empty())
    .cloned()
}

fn quotas_from_data(data: &QuotaData) -> Vec<QuotaUsage> {
    data.limits.iter().filter_map(normalize_limit).collect()
}

fn normalize_limit(raw: &RawLimit) -> Option<QuotaUsage> {
    let kind = match raw.r#type.as_deref()? {
        "TOKENS_LIMIT" => "tokens",
        "CREDIT_LIMIT" => "credits",
        "TIME_LIMIT" => "mcp",
        _ => return None,
    };
    let total = raw
        .usage
        .filter(|value| *value > 0)
        .or(raw.total.filter(|value| *value > 0));
    let mut percent = raw.percentage.unwrap_or(0.0);
    if let Some(total) = total {
        // Counts beat the reported percentage whenever the API exposes them.
        let used = match (raw.remaining, raw.current_value) {
            (Some(remaining), current) => {
                let by_remaining = total - remaining;
                Some(current.map_or(by_remaining, |value| by_remaining.max(value)))
            }
            (None, Some(current)) => Some(current),
            (None, None) => None,
        };
        if let Some(used) = used {
            percent = used as f64 / total as f64 * 100.0;
        }
    }
    percent = percent.clamp(0.0, 100.0);

    let unit = raw.unit.unwrap_or_default();
    let number = raw.number.unwrap_or_default();
    let is_monthly_mcp = kind == "mcp" && unit == 5 && number == 1;
    let window_minutes = if is_monthly_mcp {
        Some(30 * 24 * 60)
    } else {
        window_minutes(unit, number)
    };
    let label = match window_text(kind, unit, number) {
        Some(window) => format!("{}（{window}）", base_label(kind)),
        None => base_label(kind).to_owned(),
    };

    // A five-hour window cannot reset more than five hours plus clock skew out.
    let mut resets_at = raw.next_reset_time;
    if kind != "mcp"
        && window_minutes == Some(300)
        && resets_at.is_some_and(|reset| reset > now_millis() + (5 * 3600 + 60) * 1000)
    {
        resets_at = None;
    }

    let used = raw.current_value.or(match (total, raw.remaining) {
        (Some(total), Some(remaining)) => Some(total - remaining),
        _ => None,
    });
    let details = raw
        .usage_details
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|detail| {
            let name = detail.model_code.as_deref()?.trim();
            let usage = detail.usage?;
            (!name.is_empty()).then(|| QuotaDetail {
                name: tool_display_name(name),
                usage,
            })
        })
        .collect();

    Some(QuotaUsage {
        kind: kind.to_owned(),
        label,
        used_percent: percent,
        total,
        used,
        remaining: raw.remaining,
        resets_at,
        window_minutes,
        details,
    })
}

fn base_label(kind: &str) -> &'static str {
    match kind {
        "tokens" => "Token 用量",
        "credits" => "点数用量",
        _ => "MCP 用量",
    }
}

/// Unit codes shared by both monitor APIs: 1=day, 3=hour, 5=minute, 6=week.
fn window_minutes(unit: i64, number: i64) -> Option<i64> {
    let multiplier = match unit {
        1 => 1440,
        3 => 60,
        5 => 1,
        6 => 10080,
        _ => return None,
    };
    (number > 0).then(|| number * multiplier)
}

fn window_text(kind: &str, unit: i64, number: i64) -> Option<String> {
    if kind == "mcp" && unit == 5 && number == 1 {
        return Some("月度".to_owned());
    }
    let unit_name = match unit {
        1 => "天",
        3 => "小时",
        5 => "分钟",
        6 => "周",
        _ => return None,
    };
    (number > 0).then(|| format!("{number} {unit_name}"))
}

fn tool_display_name(code: &str) -> String {
    match code {
        "search-prime" => "联网搜索".to_owned(),
        "web-reader" => "网页读取".to_owned(),
        "zread" => "ZRead".to_owned(),
        other => other.to_owned(),
    }
}

// ------------------------------------------------------------ usage metrics

fn model_metrics(data: &Value, weekly: bool) -> Vec<MetricUsage> {
    let total = data.get("totalUsage");
    let suffix = if weekly { "（本周）" } else { "" };
    let mut metrics = Vec::new();
    if let Some(tokens) = total.and_then(|t| t.get("totalTokensUsage")).and_then(Value::as_i64) {
        metrics.push(MetricUsage {
            id: "tokens".to_owned(),
            label: format!("Token 用量{suffix}"),
            value: tokens,
        });
    }
    if let Some(calls) = total
        .and_then(|t| t.get("totalModelCallCount"))
        .and_then(Value::as_i64)
    {
        metrics.push(MetricUsage {
            id: "requests".to_owned(),
            label: format!("模型请求{suffix}"),
            value: calls,
        });
    }
    // Per-model totals as reported by the API (modelSummaryList).
    if let Some(models) = total.and_then(|t| t.get("modelSummaryList")).and_then(Value::as_array) {
        let mut models: Vec<(String, i64)> = models
            .iter()
            .filter_map(|model| {
                let name = model.get("modelName").and_then(Value::as_str)?.trim();
                let tokens = model.get("totalTokens").and_then(Value::as_i64)?;
                (!name.is_empty()).then(|| (name.to_owned(), tokens))
            })
            .collect();
        models.sort_by_key(|(_, tokens)| std::cmp::Reverse(*tokens));
        for (name, tokens) in models {
            metrics.push(MetricUsage {
                id: "model".to_owned(),
                label: name,
                value: tokens,
            });
        }
    }
    metrics
}

fn tool_metrics(data: &Value) -> Vec<MetricUsage> {
    let total = data.get("totalUsage");
    let fields = [
        ("totalNetworkSearchCount", "search", "联网搜索"),
        ("totalWebReadMcpCount", "webread", "网页读取"),
        ("totalZreadMcpCount", "zread", "ZRead 调用"),
    ];
    fields
        .into_iter()
        .filter_map(|(field, id, label)| {
            let value = total
                .and_then(|t| t.get(field))
                .and_then(Value::as_i64)
                .filter(|value| *value > 0)?;
            Some(MetricUsage {
                id: id.to_owned(),
                label: label.to_owned(),
                value,
            })
        })
        .collect()
}

/// Query window for the usage endpoints, formatted as local naive timestamps.
/// `Some(start)` marks the beginning of the weekly credit cycle; the fallback
/// mirrors Z.ai's own plugin: yesterday at the current hour.
fn usage_window_query(start: Option<DateTime<Local>>) -> String {
    let now = Local::now();
    let start = match start {
        Some(start) => Some(start),
        None => (now - chrono::Duration::hours(24))
            .with_minute(0)
            .and_then(|time| time.with_second(0))
            .and_then(|time| time.with_nanosecond(0)),
    };
    let end = now
        .with_minute(59)
        .and_then(|time| time.with_second(59))
        .and_then(|time| time.with_nanosecond(999_999_999));
    let (start, end) = match (start, end) {
        (Some(start), Some(end)) => (start, end),
        _ => (now - chrono::Duration::hours(24), now),
    };
    format!(
        "startTime={}&endTime={}",
        encode_query(&format_time(start)),
        encode_query(&format_time(end))
    )
}

/// Start of the running weekly credit cycle: the weekly `CREDIT_LIMIT` entry's
/// next reset minus seven days. Model-usage windows anchored here make the
/// token totals cover the same span as the weekly reset bar.
fn weekly_cycle_start(quota: &QuotaData) -> Option<DateTime<Local>> {
    let next = quota
        .limits
        .iter()
        .find(|limit| {
            limit.r#type.as_deref() == Some("CREDIT_LIMIT") && limit.unit == Some(6)
        })?
        .next_reset_time
        .filter(|reset| *reset > now_millis())?;
    DateTime::from_timestamp_millis(next - 7 * 24 * 3600 * 1000)
        .map(|start| start.with_timezone(&Local))
}

fn format_time(time: DateTime<Local>) -> String {
    time.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn limit(value: Value) -> QuotaUsage {
        normalize_limit(&serde_json::from_value(value).unwrap()).unwrap()
    }

    #[test]
    fn recomputes_percent_from_counts() {
        let quota = limit(json!({
            "type": "TOKENS_LIMIT", "unit": 3, "number": 5,
            "percentage": 1, "usage": 200, "currentValue": 90, "remaining": 110
        }));
        assert_eq!(quota.used_percent, 45.0);
        assert_eq!(quota.total, Some(200));
        assert_eq!(quota.used, Some(90));
        assert_eq!(quota.remaining, Some(110));
        assert_eq!(quota.label, "Token 用量（5 小时）");
        assert_eq!(quota.window_minutes, Some(300));
    }

    #[test]
    fn remaining_alone_derives_used() {
        let quota = limit(json!({
            "type": "CREDIT_LIMIT", "unit": 3, "number": 5,
            "percentage": 0, "total": 100, "remaining": 25
        }));
        assert_eq!(quota.used_percent, 75.0);
        assert_eq!(quota.total, Some(100));
        assert_eq!(quota.used, Some(75));
    }

    #[test]
    fn keeps_reported_percent_without_counts() {
        let quota = limit(json!({
            "type": "TIME_LIMIT", "unit": 1, "number": 7, "percentage": 12.5
        }));
        assert_eq!(quota.used_percent, 12.5);
        assert_eq!(quota.label, "MCP 用量（7 天）");
        assert!(quota.total.is_none());
    }

    #[test]
    fn clamps_out_of_range_percent() {
        let quota = limit(json!({
            "type": "TOKENS_LIMIT", "unit": 3, "number": 5,
            "percentage": 140, "usage": 10, "currentValue": 99
        }));
        assert_eq!(quota.used_percent, 100.0);
    }

    #[test]
    fn skips_unknown_limit_types() {
        let raw: RawLimit = serde_json::from_value(json!({
            "type": "SOMETHING_ELSE", "unit": 3, "number": 5, "percentage": 10
        }))
        .unwrap();
        assert!(normalize_limit(&raw).is_none());
    }

    #[test]
    fn monthly_mcp_marker_becomes_month_window() {
        let quota = limit(json!({
            "type": "TIME_LIMIT", "unit": 5, "number": 1, "percentage": 30,
            "usageDetails": [
                { "modelCode": "search-prime", "usage": 12 },
                { "modelCode": "web-reader", "usage": 34 },
                { "modelCode": "zread", "usage": 5 }
            ]
        }));
        assert_eq!(quota.label, "MCP 用量（月度）");
        assert_eq!(quota.window_minutes, Some(30 * 24 * 60));
        let names: Vec<&str> = quota.details.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["联网搜索", "网页读取", "ZRead"]);
        let usage: Vec<i64> = quota.details.iter().map(|d| d.usage).collect();
        assert_eq!(usage, [12, 34, 5]);
    }

    #[test]
    fn five_hour_reset_sanity() {
        let now = now_millis();
        let plausible = limit(json!({
            "type": "TOKENS_LIMIT", "unit": 3, "number": 5,
            "percentage": 10, "nextResetTime": now + 3 * 3600 * 1000
        }));
        assert!(plausible.resets_at.is_some());
        let implausible = limit(json!({
            "type": "TOKENS_LIMIT", "unit": 3, "number": 5,
            "percentage": 10, "nextResetTime": now + 9 * 3600 * 1000
        }));
        assert!(implausible.resets_at.is_none());
    }

    #[test]
    fn parses_quota_envelope_like_the_live_api() {
        let root: Value = json!({
            "success": true, "code": 200,
            "data": {
                "level": "Max",
                "limits": [
                    {
                        "type": "TOKENS_LIMIT", "unit": 3, "number": 5, "percentage": 42,
                        "usage": 240000, "currentValue": 100800, "remaining": 139200,
                        "nextResetTime": 1790000000000i64
                    },
                    {
                        "type": "TIME_LIMIT", "unit": 5, "number": 1, "percentage": 7,
                        "usage": 600, "currentValue": 42, "usageDetails": [
                            { "modelCode": "search-prime", "usage": 40 },
                            { "modelCode": "web-reader", "usage": 2 }
                        ]
                    }
                ]
            }
        });
        let data: QuotaData =
            serde_json::from_value(root.get("data").cloned().unwrap()).unwrap();
        let quotas = quotas_from_data(&data);
        assert_eq!(quotas.len(), 2);
        assert_eq!(plan_label(&data).as_deref(), Some("Max"));
        assert_eq!(quotas[0].used_percent, 42.0);
        assert_eq!(quotas[0].total, Some(240000));
        assert!((quotas[1].used_percent - 7.0).abs() < 1e-9);
        assert_eq!(quotas[1].details.len(), 2);
    }

    #[test]
    fn extracts_model_and_tool_metrics() {
        let model = json!({
            "totalUsage": {
                "totalModelCallCount": 318,
                "totalTokensUsage": 50925756,
                "modelSummaryList": [
                    { "modelName": "GLM-5.3", "totalTokens": 6890648, "sortOrder": 1 },
                    { "modelName": "GLM-5.3-Flash", "totalTokens": 44035108, "sortOrder": 2 }
                ]
            }
        });
        assert_eq!(
            model_metrics(&model, true),
            vec![
                MetricUsage { id: "tokens".into(), label: "Token 用量（本周）".into(), value: 50925756 },
                MetricUsage { id: "requests".into(), label: "模型请求（本周）".into(), value: 318 },
                MetricUsage { id: "model".into(), label: "GLM-5.3-Flash".into(), value: 44035108 },
                MetricUsage { id: "model".into(), label: "GLM-5.3".into(), value: 6890648 },
            ]
        );
        let no_models = json!({ "totalUsage": { "totalModelCallCount": 3, "totalTokensUsage": 100 } });
        assert_eq!(model_metrics(&no_models, true).len(), 2);
        assert_eq!(model_metrics(&no_models, false)[0].label, "Token 用量");
        let tool = json!({
            "data": {
                "totalUsage": {
                    "totalNetworkSearchCount": 21,
                    "totalWebReadMcpCount": 0,
                    "totalZreadMcpCount": 4
                }
            }
        });
        let metrics = tool_metrics(data_or_root(&tool));
        let ids: Vec<&str> = metrics.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["search", "zread"]);
    }

    #[test]
    fn encodes_query_values() {
        assert_eq!(
            encode_query("2026-09-30 14:00:00"),
            "2026-09-30%2014%3A00%3A00"
        );
        let query = usage_window_query(None);
        assert!(query.starts_with("startTime=20"));
        assert!(query.contains("&endTime=20"));
        assert!(!query.contains(' '));
        let weekly = usage_window_query(Some(Local.with_ymd_and_hms(2026, 9, 30, 12, 24, 0).unwrap()));
        assert!(weekly.starts_with("startTime=2026-09-30%2012%3A24%3A00"));
    }

    #[test]
    fn weekly_cycle_start_comes_from_credit_limit() {
        let now = now_millis();
        let data: QuotaData = serde_json::from_value(json!({
            "limits": [
                { "type": "CREDIT_LIMIT", "unit": 3, "number": 5, "nextResetTime": now },
                { "type": "CREDIT_LIMIT", "unit": 6, "number": 1, "nextResetTime": now + 3600 * 1000 }
            ]
        }))
        .unwrap();
        let start = weekly_cycle_start(&data).unwrap();
        assert_eq!(start.timestamp_millis(), now + 3600 * 1000 - 7 * 24 * 3600 * 1000);
        let no_weekly: QuotaData =
            serde_json::from_value(json!({ "limits": [{ "type": "CREDIT_LIMIT", "unit": 3, "number": 5 }] }))
                .unwrap();
        assert!(weekly_cycle_start(&no_weekly).is_none());
    }

    #[test]
    fn sanitize_strips_tokens() {
        assert_eq!(sanitize("failed sk-secret".into(), "sk-secret"), "failed ***");
    }
}
