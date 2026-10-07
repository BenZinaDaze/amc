//! DeepSeek provider（API 充值余额，key 认证）。
//!
//! 官方文档：<https://api-docs.deepseek.com/zh-cn/api/get-user-balance>。
//! `GET https://api.deepseek.com/user/balance`（`Authorization: Bearer sk-…`）返回
//! `is_available` 与按币种的余额明细（总额 / 赠金 / 充值，字符串金额）。没有配额
//! 窗口，卡片把每个币种渲染成一行无百分比的余额条（同 Sub2API 钱包模式），
//! 赠金/充值进 details 行。

use serde_json::Value;
use std::time::Duration;

use crate::platform::Result;

use super::store::StoredSubscription;
use super::{ProviderReport, QuotaDetail, QuotaUsage};

const BALANCE_URL: &str = "https://api.deepseek.com/user/balance";
pub(super) fn fetch_entry(entry: &StoredSubscription) -> Result<ProviderReport> {
    let token = entry.key.trim();
    if token.is_empty() {
        return Err("缺少 DeepSeek API Key".to_owned());
    }
    fetch_balance(BALANCE_URL, token).map_err(|error| sanitize(error, token))
}

fn sanitize(message: String, token: &str) -> String {
    if token.is_empty() {
        message
    } else {
        message.replace(token, "***")
    }
}

fn fetch_balance(url: &str, token: &str) -> Result<ProviderReport> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        // 非 2xx 走下面的状态分支，把接口的错误体带出来。
        .http_status_as_error(false)
        .build()
        .into();
    let response = agent
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .call()
        .map_err(|e| format!("请求 {url} 失败: {e}"))?;
    let status = response.status().as_u16();
    let body = response.into_body().read_to_string().unwrap_or_default();
    if status == 401 {
        return Err("DeepSeek API Key 无效".to_owned());
    }
    if status != 200 {
        return Err(format!("接口返回 HTTP {status}: {}", truncate(&body, 200)));
    }
    let value: Value = serde_json::from_str(&body)
        .map_err(|e| format!("解析余额响应失败: {e}"))?;
    report_from(&value).ok_or_else(|| "余额响应缺少可用数据".to_owned())
}

/// 响应 → 卡片：`is_available` 当套餐标签，`balance_infos` 每币种一行。
/// 金额是字符串小数（"110.00"），换成分存储；`None` 表示没有任何可用行。
fn report_from(value: &Value) -> Option<ProviderReport> {
    let plan = match value.get("is_available").and_then(Value::as_bool) {
        Some(true) => "可用",
        Some(false) => "余额耗尽",
        None => return None,
    };
    let mut quotas = Vec::new();
    for info in value.get("balance_infos")?.as_array()? {
        if let Some(quota) = balance_row(info) {
            quotas.push(quota);
        }
    }
    if quotas.is_empty() {
        return None;
    }
    Some(ProviderReport {
        plan: Some(plan.to_owned()),
        quotas,
        metrics: Vec::new(),
    })
}

fn balance_row(info: &Value) -> Option<QuotaUsage> {
    let currency = info.get("currency").and_then(Value::as_str)?;
    let unit = match currency {
        "CNY" => "cny",
        "USD" => "usd",
        other => return unknown_currency_row(other, info),
    };
    let total_text = info.get("total_balance").and_then(Value::as_str)?;
    let total = money_to_cents(total_text)?;
    let granted = info
        .get("granted_balance")
        .and_then(Value::as_str)
        .and_then(money_to_cents)
        .unwrap_or(0);
    let topped_up = info
        .get("topped_up_balance")
        .and_then(Value::as_str)
        .and_then(money_to_cents)
        .unwrap_or(0);
    let label = match unit {
        "cny" => "余额（人民币）",
        _ => "余额（美元）",
    };
    // details 按主币种单位展示（元/美元两位小数），与卡片金额行一致。
    let details = [
        ("赠金", granted),
        ("充值", topped_up),
    ]
    .into_iter()
    .filter(|&(_, cents)| cents > 0)
    .map(|(name, cents)| QuotaDetail {
        name: name.to_owned(),
        usage: cents / 100,
    })
    .collect();
    Some(QuotaUsage {
        kind: "balance".to_owned(),
        label: label.to_owned(),
        used_percent: 0.0,
        total: None,
        used: Some(total),
        remaining: Some(total),
        resets_at: None,
        window_minutes: None,
        details,
        unit: Some(unit.to_owned()),
    })
}

/// 未知币种：仍展示总额数字，但币种名原样进标签、不带货币单位。
fn unknown_currency_row(currency: &str, info: &Value) -> Option<QuotaUsage> {
    let total_text = info.get("total_balance").and_then(Value::as_str)?;
    let total = money_to_cents(total_text)?;
    Some(QuotaUsage {
        kind: "balance".to_owned(),
        label: format!("余额（{currency}）"),
        used_percent: 0.0,
        total: None,
        used: Some(total),
        remaining: Some(total),
        resets_at: None,
        window_minutes: None,
        details: Vec::new(),
        unit: None,
    })
}

/// "110.00" → 11000（分）。解析失败按缺失处理，不臆造 0。
fn money_to_cents(text: &str) -> Option<i64> {
    let amount: f64 = text.trim().parse().ok()?;
    if !amount.is_finite() || amount < 0.0 {
        return None;
    }
    Some((amount * 100.0).round() as i64)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_cny_balance_with_details() {
        let body = json!({
            "is_available": true,
            "balance_infos": [{
                "currency": "CNY",
                "total_balance": "110.00",
                "granted_balance": "10.00",
                "topped_up_balance": "100.00"
            }]
        });
        let report = report_from(&body).unwrap();
        assert_eq!(report.plan.as_deref(), Some("可用"));
        assert_eq!(report.quotas.len(), 1);
        let quota = &report.quotas[0];
        assert_eq!(quota.label, "余额（人民币）");
        assert_eq!(quota.used, Some(11000));
        assert_eq!(quota.unit.as_deref(), Some("cny"));
        assert_eq!(quota.details.len(), 2);
        assert_eq!(quota.details[0].name, "赠金");
        assert_eq!(quota.details[0].usage, 10);
        assert_eq!(quota.details[1].usage, 100);
    }

    #[test]
    fn parses_usd_balance_without_granted() {
        let body = json!({
            "is_available": false,
            "balance_infos": [{
                "currency": "USD",
                "total_balance": "1.05",
                "granted_balance": "0.00",
                "topped_up_balance": "1.05"
            }]
        });
        let report = report_from(&body).unwrap();
        assert_eq!(report.plan.as_deref(), Some("余额耗尽"));
        let quota = &report.quotas[0];
        assert_eq!(quota.label, "余额（美元）");
        assert_eq!(quota.unit.as_deref(), Some("usd"));
        assert_eq!(quota.used, Some(105));
        // 0 值赠金不进 details。
        assert_eq!(quota.details.len(), 1);
        assert_eq!(quota.details[0].name, "充值");
    }

    #[test]
    fn multiple_currencies_get_one_row_each() {
        let body = json!({
            "is_available": true,
            "balance_infos": [
                { "currency": "CNY", "total_balance": "10.00", "granted_balance": "0", "topped_up_balance": "10.00" },
                { "currency": "USD", "total_balance": "2.50", "granted_balance": "2.50", "topped_up_balance": "0" }
            ]
        });
        let report = report_from(&body).unwrap();
        assert_eq!(report.quotas.len(), 2);
        assert_eq!(report.quotas[0].label, "余额（人民币）");
        assert_eq!(report.quotas[1].label, "余额（美元）");
        assert_eq!(report.quotas[1].details[0].name, "赠金");
    }

    #[test]
    fn unknown_currency_falls_back_to_plain_number() {
        let body = json!({
            "is_available": true,
            "balance_infos": [{ "currency": "EUR", "total_balance": "3.33" }]
        });
        let report = report_from(&body).unwrap();
        let quota = &report.quotas[0];
        assert_eq!(quota.label, "余额（EUR）");
        assert_eq!(quota.unit, None);
        assert_eq!(quota.used, Some(333));
    }

    #[test]
    fn missing_fields_yield_no_report() {
        assert!(report_from(&json!({ "balance_infos": [] })).is_none());
        assert!(report_from(&json!({ "is_available": true })).is_none());
        assert!(
            report_from(&json!({
                "is_available": true,
                "balance_infos": [{ "currency": "CNY", "total_balance": "abc" }]
            }))
            .is_none()
        );
    }

    #[test]
    fn cents_rounding_is_safe() {
        assert_eq!(money_to_cents("110.00"), Some(11000));
        assert_eq!(money_to_cents("0.005"), Some(1));
        assert_eq!(money_to_cents(""), None);
        assert_eq!(money_to_cents("-1"), None);
    }

    // ---- HTTP 层：本地服务端离线覆盖鉴权头、路径与错误分支 ----

    /// 起一次性本地 HTTP 服务；返回 (请求 URL, 收到的请求行与鉴权头)。
    fn serve(status: &str, body: &str) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let expected = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}");
        let handle = std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};
            let (mut stream, _) = listener.accept().unwrap();
            // 请求头可能分多个 TCP 段到达：读到头部结束（空行）为止。
            let mut request = String::new();
            let mut chunk = [0_u8; 1024];
            loop {
                let read = stream.read(&mut chunk).unwrap_or(0);
                if read == 0 {
                    break;
                }
                request.push_str(&String::from_utf8_lossy(&chunk[..read]));
                if request.contains("\r\n\r\n") {
                    break;
                }
            }
            let mut seen = vec![
                request.split_whitespace().take(2).collect::<Vec<_>>().join(" "),
            ];
            let lowered = request.to_lowercase();
            if let Some(index) = lowered.find("authorization:") {
                seen.push(request[index..].lines().next().unwrap_or("").trim().to_owned());
            }
            stream.write_all(expected.as_bytes()).unwrap();
            seen
        });
        (format!("http://127.0.0.1:{port}/user/balance"), handle)
    }

    #[test]
    fn http_layer_sends_bearer_and_maps_auth_error() {
        let (url, handle) = serve(
            "401 Unauthorized",
            r#"{"error":{"message":"Authentication Fails","type":"authentication_error"}}"#,
        );
        let error = fetch_balance(&url, "sk-test123456").unwrap_err();
        assert_eq!(error, "DeepSeek API Key 无效");
        let seen = handle.join().unwrap();
        assert_eq!(seen[0], "GET /user/balance");
        assert_eq!(seen[1], "authorization: Bearer sk-test123456");
    }

    #[test]
    fn http_layer_maps_balance_body() {
        let (url, handle) = serve(
            "200 OK",
            r#"{"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"110.00","granted_balance":"10.00","topped_up_balance":"100.00"}]}"#,
        );
        let report = fetch_balance(&url, "sk-test123456")
            .unwrap_or_else(|error| panic!("预期查询成功: {error}"));
        handle.join().unwrap();
        assert_eq!(report.plan.as_deref(), Some("可用"));
        assert_eq!(report.quotas[0].used, Some(11000));
    }

    #[test]
    fn http_layer_reports_non_json_error_body() {
        let (url, handle) = serve("503 Service Unavailable", "gateway down");
        let error = fetch_balance(&url, "sk-test123456").unwrap_err();
        handle.join().unwrap();
        assert!(
            error.starts_with("接口返回 HTTP 503"),
            "实际错误: {error}"
        );
        assert!(error.contains("gateway down"));
    }
}
