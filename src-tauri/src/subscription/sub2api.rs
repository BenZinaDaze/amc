//! Sub2API instance provider (self-hosted relay; key-only introspection).
//!
//! One endpoint per credential: `GET {base}/v1/usage` authenticates with the
//! API key (`Authorization: Bearer sk-…`) and skips billing, so a key can
//! read its own plan and usage without panel login. Response shape depends on
//! the key's group: subscription (订阅模式), wallet (余额模式) or quota-limited
//! (key 总额度/速率限制); all amounts are USD on the Sub2API instance.

use serde_json::Value;
use std::time::Duration;

use crate::omp::Result;

use super::{MetricUsage, ProviderReport, QuotaUsage};
use super::store::StoredSubscription;

/// Plug-in point for the vendor catalog; keeps token leakage out of errors.
pub(super) fn fetch_entry(entry: &StoredSubscription) -> Result<ProviderReport> {
    let base_url = entry
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| "缺少实例地址".to_owned())?;
    let token = entry.key.trim();
    fetch_usage(base_url, token).map_err(|error| sanitize(error, token))
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
        // 非 2xx 走下面的状态分支，把实例返回的错误体带出来。
        .http_status_as_error(false)
        .build()
        .into();
    let response = agent
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
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

// ------------------------------------------------------------- report build

fn fetch_usage(base_url: &str, token: &str) -> Result<ProviderReport> {
    let value = get_json(&format!("{base_url}/v1/usage"), token)?;
    if value.get("isValid") == Some(&Value::Bool(false)) {
        return Err("Key 已失效或被禁用".to_owned());
    }
    // 订阅模式带周窗口起点时，用 start_date 重取一次，让模型分项与重置周期
    // 对齐（默认是无参数的近 30 天）；重取失败则沿用第一次的响应。
    let window_start = value
        .get("subscription")
        .and_then(|s| s.get("weekly_window_start"))
        .and_then(Value::as_str)
        .and_then(window_start_date);
    let value = match window_start.as_deref() {
        Some(date) => get_json(&format!("{base_url}/v1/usage?start_date={date}"), token).unwrap_or(value),
        None => value,
    };
    let plan = value
        .get("planName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
        .map(str::to_owned);

    let mut quotas = Vec::new();
    let mut metrics = Vec::new();
    match value.get("mode").and_then(Value::as_str) {
        Some("quota_limited") => {
            quotas.extend(quota_limited_quotas(&value));
        }
        _ => quotas.extend(unrestricted_quotas(&value)),
    }
    metrics.extend(usage_metrics(value.get("usage")));
    // 与重置周期对齐后，给一行窗口内 Token 合计。start_date 只支持日期，
    // 从服务器当日零点起算，比真实重置时刻早一段（属日期精度统计）。
    if window_start.is_some() {
        let window_tokens: i64 = value
            .get("model_stats")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get("total_tokens").and_then(Value::as_i64))
            .sum();
        if window_tokens > 0 {
            metrics.push(MetricUsage {
                id: "tokens".to_owned(),
                label: "周期内 Token".to_owned(),
                value: window_tokens,
            });
        }
    }
    metrics.extend(model_metrics(value.get("model_stats")));
    if plan.is_none() && quotas.is_empty() && metrics.is_empty() {
        return Err("接口未返回有效的订阅或用量数据".to_owned());
    }
    Ok(ProviderReport {
        plan: plan.or_else(|| quota_limited_plan(&value)),
        quotas,
        metrics,
    })
}

/// Key 总额度 + 5h/1d/7d 速率限制窗口（美分）。
fn quota_limited_quotas(value: &Value) -> Vec<QuotaUsage> {
    let mut quotas = Vec::new();
    let quota = value.get("quota");
    if let Some((total, used)) = quota.and_then(|q| q.get("limit").zip(q.get("used"))).and_then(money_pair) {
        quotas.push(QuotaUsage {
            kind: "credits".to_owned(),
            label: "总额度".to_owned(),
            used_percent: percent(used, total),
            total: Some(total),
            used: Some(used),
            remaining: quota.and_then(|q| q.get("remaining")).and_then(money),
            resets_at: None,
            window_minutes: None,
            details: Vec::new(),
            unit: Some("usd".to_owned()),
        });
    }
    for limit in value
        .get("rate_limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let window = limit.get("window").and_then(Value::as_str).unwrap_or("");
        let Some((total, used)) = limit.get("limit").zip(limit.get("used")).and_then(money_pair) else {
            continue;
        };
        quotas.push(QuotaUsage {
            kind: "credits".to_owned(),
            label: format!("{}额度", window_label(window)),
            used_percent: percent(used, total),
            total: Some(total),
            used: Some(used),
            remaining: limit.get("remaining").and_then(money),
            resets_at: limit
                .get("reset_at")
                .and_then(Value::as_str)
                .and_then(rfc3339_millis),
            window_minutes: match window {
                "5h" => Some(300),
                "1d" => Some(1440),
                "7d" => Some(10080),
                _ => None,
            },
            details: Vec::new(),
            unit: Some("usd".to_owned()),
        });
    }
    quotas
}

/// 订阅模式取日/周/月窗口，余额模式取钱包余额（美分）。
fn unrestricted_quotas(value: &Value) -> Vec<QuotaUsage> {
    let mut quotas = Vec::new();
    let subscription = value.get("subscription");
    for (label, limit_key, usage_key) in [
        ("每日额度", "daily_limit_usd", "daily_usage_usd"),
        ("每周额度", "weekly_limit_usd", "weekly_usage_usd"),
        ("每月额度", "monthly_limit_usd", "monthly_usage_usd"),
    ] {
        let limit = subscription
            .and_then(|s| s.get(limit_key))
            .and_then(money)
            .filter(|limit| *limit > 0);
        let Some(limit) = limit else { continue };
        let used = subscription
            .and_then(|s| s.get(usage_key))
            .and_then(money)
            .unwrap_or(0);
        quotas.push(QuotaUsage {
            kind: "credits".to_owned(),
            label: label.to_owned(),
            used_percent: percent(used, limit),
            total: Some(limit),
            used: Some(used),
            remaining: Some((limit - used).max(0)),
            resets_at: None,
            window_minutes: match usage_key {
                "daily_usage_usd" => Some(1440),
                "weekly_usage_usd" => Some(10080),
                _ => Some(43200),
            },
            details: Vec::new(),
            unit: Some("usd".to_owned()),
        });
    }
    // 余额模式：没有限额窗口，把余额展示成一行为不给百分比的额度条。
    if quotas.is_empty() {
        if let Some(balance) = value.get("balance").and_then(money) {
            quotas.push(QuotaUsage {
                kind: "credits".to_owned(),
                label: "钱包余额".to_owned(),
                used_percent: 0.0,
                total: None,
                used: Some(balance),
                remaining: value.get("remaining").and_then(money).or(Some(balance)),
                resets_at: None,
                window_minutes: None,
                details: Vec::new(),
                unit: Some("usd".to_owned()),
            });
        }
    }
    quotas
}

/// quota_limited 响应没有 planName，用状态推导卡片标签。
fn quota_limited_plan(value: &Value) -> Option<String> {
    let mode = value.get("mode").and_then(Value::as_str)?;
    if mode != "quota_limited" {
        return None;
    }
    let status = value.get("status").and_then(Value::as_str).unwrap_or("");
    let label = match status {
        "active" => "总额度 Key",
        "quota_exhausted" => "总额度 Key（已耗尽）",
        "expired" => "总额度 Key（已过期）",
        _ => "总额度 Key",
    };
    Some(label.to_owned())
}

fn usage_metrics(usage: Option<&Value>) -> Vec<MetricUsage> {
    let mut metrics = Vec::new();
    let total = usage.and_then(|usage| usage.get("total"));
    if let Some(tokens) = total.and_then(|t| t.get("total_tokens")).and_then(Value::as_i64) {
        metrics.push(MetricUsage {
            id: "tokens".to_owned(),
            label: "Token 用量".to_owned(),
            value: tokens,
        });
    }
    if let Some(requests) = total.and_then(|t| t.get("requests")).and_then(Value::as_i64) {
        metrics.push(MetricUsage {
            id: "requests".to_owned(),
            label: "请求总数".to_owned(),
            value: requests,
        });
    }
    let today = usage.and_then(|usage| usage.get("today"));
    if let Some(tokens) = today.and_then(|t| t.get("total_tokens")).and_then(Value::as_i64) {
        metrics.push(MetricUsage {
            id: "tokens".to_owned(),
            label: "今日 Token".to_owned(),
            value: tokens,
        });
    }
    metrics
}

/// 各模型近 30 天 Token 分项，按用量降序，最多 8 行。
fn model_metrics(stats: Option<&Value>) -> Vec<MetricUsage> {
    let mut models: Vec<(String, i64)> = stats
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let name = entry.get("model").and_then(Value::as_str)?.trim();
            let tokens = entry.get("total_tokens").and_then(Value::as_i64)?;
            (!name.is_empty()).then(|| (name.to_owned(), tokens))
        })
        .collect();
    models.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    models.truncate(8);
    models
        .into_iter()
        .map(|(name, tokens)| MetricUsage {
            id: "tokens".to_owned(),
            label: name,
            value: tokens,
        })
        .collect()
}

// ------------------------------------------------------------------ helpers

/// `(limit, used)` → `(美分总额, 美分已用)`；总额缺失或非正即返回 `None`。
fn money_pair(pair: (&Value, &Value)) -> Option<(i64, i64)> {
    let limit = money(pair.0)?;
    if limit <= 0 {
        return None;
    }
    Some((limit, money(pair.1).unwrap_or(0)))
}

fn percent(used: i64, total: i64) -> f64 {
    if total <= 0 {
        0.0
    } else {
        used as f64 / total as f64 * 100.0
    }
}

/// USD 浮点 → 美分整数（四舍五入）。
fn money(value: &Value) -> Option<i64> {
    value.as_f64().map(|usd| (usd * 100.0).round() as i64)
}

fn window_label(window: &str) -> &str {
    match window {
        "5h" => "5 小时",
        "1d" => "每日",
        "7d" => "每周",
        other => other,
    }
}

/// RFC 3339 时间戳 → 毫秒；解析失败时静默省略倒计时。
fn rfc3339_millis(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.timestamp_millis())
}

/// `2026-09-27T01:22:03+08:00` → `2026-09-27`：实例时区下的窗口起始日期，
/// 直接作为 `start_date` 查询参数。
fn window_start_date(value: &str) -> Option<String> {
    let date = value.split('T').next()?;
    (date.len() == 10 && date.bytes().all(|byte| byte.is_ascii_digit() || byte == b'-'))
        .then(|| date.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};

    /// Minimal HTTP server answering `requests` sequential connections with
    /// `(status, body)`, recording each request's head.
    fn serve(
        requests: usize,
        status_line: &str,
        body: &str,
    ) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (status_line, body) = (status_line.to_owned(), body.to_owned());
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = String::new();
                let mut buffer = [0u8; 4096];
                let read = stream.read(&mut buffer).unwrap_or(0);
                request.push_str(&String::from_utf8_lossy(&buffer[..read]));
                let body_bytes = body.as_bytes();
                let response = format!(
                    "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body_bytes.len()
                );
                stream.write_all(response.as_bytes()).unwrap_or(());
                seen.push(request);
            }
            seen
        });
        (port, handle)
    }

    fn entry(base_url: String, key: &str) -> StoredSubscription {
        StoredSubscription {
            id: "1".to_owned(),
            kind: "sub2api".to_owned(),
            name: "自建".to_owned(),
            platform: String::new(),
            base_url: Some(base_url),
            key: key.to_owned(),
        }
    }

    const SUBSCRIPTION_BODY: &str = r#"{
        "mode": "unrestricted", "isValid": true, "planName": "Claude Max 共享",
        "remaining": 21.5, "unit": "USD",
        "subscription": {
            "daily_usage_usd": 2.5, "weekly_usage_usd": 8.25, "monthly_usage_usd": 28.5,
            "daily_limit_usd": 10, "weekly_limit_usd": 50, "monthly_limit_usd": 200,
            "weekly_window_start": "2026-09-28T00:00:00Z", "expires_at": "2026-10-28T00:00:00Z"
        },
        "usage": { "today": { "requests": 5, "total_tokens": 12000 },
                   "total": { "requests": 210, "total_tokens": 987654 } },
        "model_stats": [ { "model": "claude-sonnet", "total_tokens": 900000 } ]
    }"#;

    #[test]
    fn window_start_date_extracts_date_part() {
        assert_eq!(
            window_start_date("2026-09-27T01:22:03.171467+08:00").as_deref(),
            Some("2026-09-27")
        );
        assert_eq!(window_start_date("2026-09-28T00:00:00Z").as_deref(), Some("2026-09-28"));
        assert_eq!(window_start_date("garbage"), None);
        assert_eq!(window_start_date("2026-9-7T00:00:00Z"), None);
    }

    #[test]
    fn fetch_entry_hits_v1_usage_with_bearer_key() {
        let (port, handle) = serve(2, "HTTP/1.1 200 OK", SUBSCRIPTION_BODY);
        let report = fetch_entry(&entry(format!("http://127.0.0.1:{port}"), "sk-good-key")).unwrap();
        let requests = handle.join().unwrap();
        let first = requests[0].to_ascii_lowercase();
        assert!(requests[0].starts_with("GET /v1/usage HTTP/1.1"), "{}", requests[0]);
        assert!(first.contains("authorization: bearer sk-good-key"), "{}", requests[0]);
        // 周窗口起点回传为 start_date，第二次请求对齐重置周期。
        assert!(
            requests[1].starts_with("GET /v1/usage?start_date=2026-09-28 HTTP/1.1"),
            "{}", requests[1]
        );

        assert_eq!(report.plan.as_deref(), Some("Claude Max 共享"));
        assert_eq!(report.quotas.len(), 3);
        assert_eq!(report.quotas[0].label, "每日额度");
        assert_eq!(report.quotas[0].unit.as_deref(), Some("usd"));
        assert_eq!(report.quotas[0].total, Some(1000));
        assert_eq!(report.quotas[0].used, Some(250));
        assert_eq!(report.quotas[2].used, Some(2850));
        let tokens: i64 = report
            .metrics
            .iter()
            .filter(|metric| metric.id == "tokens")
            .map(|metric| metric.value)
            .sum();
        // Token 用量 + 今日 Token + 本周 Token（窗口合计）+ 模型分项。
        assert_eq!(tokens, 987654 + 12000 + 900000 + 900000);
        let week = report
            .metrics
            .iter()
            .find(|metric| metric.label == "周期内 Token")
            .unwrap();
        assert_eq!(week.value, 900000);
    }

    #[test]
    fn fetch_entry_reports_http_errors_without_leaking_key() {
        let (port, handle) = serve(
            1,
            "HTTP/1.1 401 Unauthorized",
            r#"{"error":{"type":"authentication_error","message":"Invalid API key"}}"#,
        );
        let Err(error) = fetch_entry(&entry(format!("http://127.0.0.1:{port}"), "sk-leaky-key")) else {
            panic!("expected 401 to fail");
        };
        let _ = handle.join().unwrap();
        assert!(error.contains("HTTP 401"), "{error}");
        assert!(!error.to_ascii_lowercase().contains("sk-leaky-key"), "{error}");
    }

    #[test]
    fn fetch_entry_requires_a_base_url() {
        let Err(error) = fetch_entry(&entry(String::new(), "sk-x")) else {
            panic!("expected missing base URL to fail");
        };
        assert_eq!(error, "缺少实例地址");
    }

    #[test]
    fn catalog_serializes_form_fields() {
        let kinds: Vec<serde_json::Value> = super::super::list_kinds()
            .iter()
            .map(|kind| serde_json::to_value(kind).unwrap())
            .collect();
        let glm = &kinds[0];
        assert_eq!(glm["id"], "glm");
        assert!(glm["urlLabel"].is_null());
        assert_eq!(glm["platforms"][0][0], "zai");
        let sub2api = kinds.iter().find(|kind| kind["id"] == "sub2api").unwrap();
        assert_eq!(sub2api["urlLabel"], "实例地址");
        assert_eq!(sub2api["urlPlaceholder"], "例如 https://sub2api.example.com");
        assert_eq!(sub2api["platforms"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn subscription_mode_maps_plan_windows_and_metrics() {
        let value = json!({
            "mode": "unrestricted",
            "isValid": true,
            "planName": "Claude Pro 共享",
            "remaining": 12.345,
            "unit": "USD",
            "subscription": {
                "daily_usage_usd": 1.2, "weekly_usage_usd": 3.4, "monthly_usage_usd": 15.6,
                "daily_limit_usd": 10.0, "weekly_limit_usd": 50.0, "monthly_limit_usd": 200.0,
                "weekly_window_start": "2026-09-28T00:00:00Z"
            },
            "usage": {
                "today": { "requests": 12, "total_tokens": 3456 },
                "total": { "requests": 99, "total_tokens": 789012 }
            },
            "model_stats": [
                { "model": "claude-sonnet", "total_tokens": 500000 },
                { "model": "claude-opus", "total_tokens": 289012 }
            ]
        });
        let quotas = unrestricted_quotas(&value);
        assert_eq!(quotas.len(), 3);
        assert_eq!(quotas[0].label, "每日额度");
        assert_eq!(quotas[0].unit.as_deref(), Some("usd"));
        assert_eq!(quotas[0].total, Some(1000));
        assert_eq!(quotas[0].used, Some(120));
        assert_eq!(quotas[1].label, "每周额度");
        assert_eq!(quotas[2].total, Some(20000));
        let metrics = usage_metrics(value.get("usage"));
        assert_eq!(metrics.len(), 3);
        assert_eq!(metrics[0].value, 789012);
        let models = model_metrics(value.get("model_stats"));
        assert_eq!(models[0].label, "claude-sonnet");
        assert_eq!(models[1].value, 289012);
    }

    #[test]
    fn quota_limited_mode_maps_quota_and_rate_limits() {
        let value = json!({
            "mode": "quota_limited",
            "isValid": true,
            "status": "active",
            "quota": { "limit": 100.0, "used": 25.5, "remaining": 74.5, "unit": "USD" },
            "rate_limits": [
                { "window": "5h", "limit": 10.0, "used": 9.9, "remaining": 0.1,
                  "window_start": "2026-10-01T00:00:00Z", "reset_at": "2026-10-01T05:00:00Z" }
            ],
            "expires_at": "2026-12-31T00:00:00Z"
        });
        let quotas = quota_limited_quotas(&value);
        assert_eq!(quotas.len(), 2);
        assert_eq!(quotas[0].label, "总额度");
        assert_eq!(quotas[0].unit.as_deref(), Some("usd"));
        assert_eq!(quotas[0].total, Some(10000));
        assert_eq!(quotas[0].used, Some(2550));
        assert_eq!(quotas[1].label, "5 小时额度");
        assert_eq!(quotas[1].remaining, Some(10));
        assert_eq!(quotas[1].window_minutes, Some(300));
        assert!(quotas[1].resets_at.is_some());
        assert_eq!(quota_limited_plan(&value).as_deref(), Some("总额度 Key"));
    }

    #[test]
    fn wallet_mode_maps_balance_row() {
        let value = json!({
            "mode": "unrestricted",
            "isValid": true,
            "planName": "钱包余额",
            "remaining": 3.2,
            "balance": 3.2,
            "unit": "USD",
            "usage": { "total": { "requests": 7, "total_tokens": 1234 } }
        });
        let quotas = unrestricted_quotas(&value);
        assert_eq!(quotas.len(), 1);
        assert_eq!(quotas[0].label, "钱包余额");
        assert_eq!(quotas[0].unit.as_deref(), Some("usd"));
        assert_eq!(quotas[0].used, Some(320));
        assert_eq!(quotas[0].total, None);
    }
}
