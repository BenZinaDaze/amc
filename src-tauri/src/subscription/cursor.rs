//! Cursor 订阅供应商（个人 Pro / Pro+ / Ultra 订阅，网页登录凭据）。
//!
//! Cursor 没有公开 OAuth，但有 CLI 同款的浏览器登录流：本地生成 PKCE
//! verifier / challenge 加 uuid，打开 `cursor.com/loginDeepControl`，轮询
//! `api2.cursor.sh/auth/poll` 直到浏览器侧登录完成换回 token —— 与
//! `cursor-agent login`、magpie 社区插件（packages/cursor 的 index.mjs）
//! 同源。网页登录的 token 约两个月有效、没有可用的刷新端点，过期后重新
//! 登录。
//!
//! 额度查询走 `DashboardService/GetCurrentPeriodUsage`（Connect unary，
//! Bearer token）。已证实的响应字段只有三个使用比例（Cursor Models 池 /
//! Other Models 池 / Total）与 `billingCycleEnd` 重置时间；`planUsage`
//! 缺失（企业账号报 spend）或没有任何比例字段时返回空额度行，绝不臆造
//! 数值。

use chrono::Utc;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use uuid::Uuid;

use super::{ProviderReport, QuotaUsage};
use crate::platform::Result;

const API: &str = "https://api2.cursor.sh";
const WEBSITE: &str = "https://cursor.com";
/// API 会拒绝过旧的 client 版本；未探测本机 cursor-agent 版本时说这个
/// （与社区插件同款回退值）。
const CLIENT_VERSION: &str = "cli-2026.09.23-86fc751";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(180);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// 轮询里非 404 失败（403 除外）的容忍次数，与插件一致。
const MAX_POLL_ERRORS: u32 = 3;

// --------------------------------------------------------------------- login

/// 一次成功登录的产物；token 只进存储，永不出后端。
pub(super) struct Login {
    pub account: Option<String>,
    pub access_token: String,
}

/// 跑完浏览器登录流：生成 PKCE + uuid → 打开浏览器 → 轮询换 token →
/// 读取账号邮箱。
pub(super) fn login() -> Result<Login> {
    let verifier = random_verifier();
    let challenge = base64url_no_pad(&sha256(verifier.as_bytes()));
    let uuid = Uuid::new_v4().to_string();
    open_browser(&login_url(&challenge, &uuid))?;
    let access_token = poll_for_token(&verifier, &uuid)?;
    let account = get_me(&agent(), &access_token);
    Ok(Login {
        account,
        access_token,
    })
}

/// 登录深链的参数与 `cursor-agent login` 打出的链接同形；`cursor-agent`
/// 会把被换行截断的链接判为无效，这里一条拼完整。
fn login_url(challenge: &str, uuid: &str) -> String {
    format!("{WEBSITE}/loginDeepControl?challenge={challenge}&uuid={uuid}&mode=login&redirectTarget=cli")
}

fn open_browser(url: &str) -> Result<()> {
    tauri_plugin_opener::open_url(url, None::<&str>)
        .map_err(|e| format!("打开浏览器失败: {e}"))
}

/// 轮询 `/auth/poll`：404 表示浏览器侧还没完成，继续等并重置失败计数；
/// 403 是 Cursor 明确拒绝；其余失败累计 [`MAX_POLL_ERRORS`] 次视为不可达。
fn poll_for_token(verifier: &str, uuid: &str) -> Result<String> {
    let agent = agent();
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    let mut wait = Duration::from_secs(1);
    let mut errors = 0_u32;
    loop {
        if Instant::now() >= deadline {
            return Err("等待 Cursor 登录超时，请重试".to_owned());
        }
        let response = agent
            .get(&format!("{API}/auth/poll?uuid={uuid}&verifier={verifier}"))
            .header("Content-Type", "application/json")
            .header("x-cursor-client-version", CLIENT_VERSION)
            .header("x-cursor-client-type", "cli")
            .call();
        match response {
            Ok(response) => {
                let value: Value = response
                    .into_body()
                    .read_json()
                    .map_err(|e| format!("解析 Cursor 登录响应失败: {e}"))?;
                return string_field(&value, "accessToken")
                    .ok_or_else(|| "Cursor 登录响应缺少 accessToken".to_owned());
            }
            Err(ureq::Error::StatusCode(404)) => errors = 0,
            Err(ureq::Error::StatusCode(403)) => {
                return Err("Cursor 拒绝了登录请求（403）".to_owned());
            }
            Err(ureq::Error::StatusCode(status)) => {
                errors += 1;
                if errors >= MAX_POLL_ERRORS {
                    return Err(sanitize(
                        format!("Cursor 登录轮询返回 HTTP {status}"),
                        &[verifier, uuid],
                    ));
                }
            }
            Err(error) => {
                errors += 1;
                if errors >= MAX_POLL_ERRORS {
                    return Err(sanitize(
                        format!("连接 Cursor 失败: {error}"),
                        &[verifier, uuid],
                    ));
                }
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(wait.min(remaining));
        wait = (wait * 12 / 10).min(Duration::from_secs(10));
    }
}

// -------------------------------------------------------------- quota fetch

pub(super) fn fetch_entry(entry: &super::store::StoredSubscription) -> Result<ProviderReport> {
    let access_token = entry.key.trim();
    if access_token.is_empty() {
        return Err("缺少 Cursor 登录凭据".to_owned());
    }
    if jwt_expired(access_token) {
        return Err("Cursor 登录已过期（约两个月有效），请删除该套餐后重新登录".to_owned());
    }
    let agent = agent();
    let usage = connect_unary(
        &agent,
        "aiserver.v1.DashboardService/GetCurrentPeriodUsage",
        access_token,
    )?;
    let plan = plan_name(&agent, access_token);
    Ok(ProviderReport {
        plan,
        quotas: parse_usage(&usage),
        metrics: Vec::new(),
    })
}

/// `GetCurrentPeriodUsage` → 三条比例窗口（Cursor Models 池 / Other Models
/// 池 / Total），`resets_at` 取 `billingCycleEnd`；比例字段缺失的按 0 计、
/// 越界的收进 0–100，但三个比例全缺（形状变了或企业账号）时返回空列表。
fn parse_usage(value: &Value) -> Vec<QuotaUsage> {
    let Some(usage) = value.get("planUsage") else {
        return Vec::new();
    };
    let resets_at = value.get("billingCycleEnd").and_then(epoch_millis);
    let percent = |field: &str| usage.get(field).and_then(Value::as_f64);
    if percent("autoPercentUsed").is_none()
        && percent("apiPercentUsed").is_none()
        && percent("totalPercentUsed").is_none()
    {
        return Vec::new();
    }
    let bounded =
        |value: Option<f64>| value.unwrap_or(0.0).clamp(0.0, 100.0);
    vec![
        quota("Cursor Models", bounded(percent("autoPercentUsed")), resets_at),
        quota("Other Models", bounded(percent("apiPercentUsed")), resets_at),
        quota("Total", bounded(percent("totalPercentUsed")), resets_at),
    ]
}

/// 只报比例：金额字段（used/total/remaining）未在响应里得到证实，不填。
fn quota(label: &str, used_percent: f64, resets_at: Option<i64>) -> QuotaUsage {
    QuotaUsage {
        kind: "percent".to_owned(),
        label: label.to_owned(),
        used_percent,
        total: None,
        used: None,
        remaining: None,
        resets_at,
        window_minutes: None,
        details: Vec::new(),
        unit: None,
    }
}

/// 账号邮箱（登录后落库进 account）；失败只降级为空，不阻断登录。
fn get_me(agent: &ureq::Agent, access_token: &str) -> Option<String> {
    let value = connect_unary(agent, "aiserver.v1.DashboardService/GetMe", access_token)
        .ok()?;
    string_field(&value, "email")
}

/// 套餐名（`planInfo.planName`）；失败只降级为空，不阻断额度。
fn plan_name(agent: &ureq::Agent, access_token: &str) -> Option<String> {
    let value = connect_unary(
        agent,
        "aiserver.v1.DashboardService/GetPlanInfo",
        access_token,
    )
    .ok()?;
    value
        .get("planInfo")
        .and_then(|plan| string_field(plan, "planName"))
}

// ---------------------------------------------------------------- http layer

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .into()
}

/// Connect unary 调用：POST 空 JSON，非 2xx 映射成带状态的可读错误。
fn connect_unary(agent: &ureq::Agent, path: &str, access_token: &str) -> Result<Value> {
    let response = agent
        .post(&format!("{API}/{path}"))
        .header("Content-Type", "application/json")
        .header("Connect-Protocol-Version", "1")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("x-cursor-client-version", CLIENT_VERSION)
        .header("x-cursor-client-type", "cli")
        .header("x-ghost-mode", "true")
        .send_json(&json!({}))
        .map_err(|error| match error {
            ureq::Error::StatusCode(401) => {
                "Cursor 登录已过期，请删除该套餐后重新登录".to_owned()
            }
            ureq::Error::StatusCode(status) => format!("请求 {path} 返回 HTTP {status}"),
            other => sanitize(format!("请求 {path} 失败: {other}"), &[access_token]),
        })?;
    response
        .into_body()
        .read_json()
        .map_err(|e| format!("解析 {path} 响应失败: {e}"))
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// 错误信息里不回显 verifier / uuid / token。
fn sanitize(mut message: String, secrets: &[&str]) -> String {
    for secret in secrets {
        if !secret.is_empty() {
            message = message.replace(secret, "***");
        }
    }
    message
}

/// `billingCycleEnd`：毫秒时间戳，数字或数字字符串都收，非法丢弃。
fn epoch_millis(value: &Value) -> Option<i64> {
    let millis = match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    };
    millis.filter(|millis| *millis > 0)
}

/// 网页登录 token 是 JWT；`exp` 已过即本地判过期（无 exp 时交给 401 兜底）。
fn jwt_expired(token: &str) -> bool {
    let Some(payload) = token.split('.').nth(1) else {
        return false;
    };
    let Ok(bytes) = base64url_decode(payload) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    value
        .get("exp")
        .and_then(Value::as_i64)
        .filter(|exp| *exp > 0)
        .is_some_and(|exp| exp <= Utc::now().timestamp())
}

// ------------------------------------------------------------ pkce plumbing

fn sha256(bytes: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).to_vec()
}

/// base64url 无填充（RFC 7636 code_challenge 编码）。
fn base64url_no_pad(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        out.push(TABLE[(n >> 18 & 63) as usize] as char);
        out.push(TABLE[(n >> 12 & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(n >> 6 & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 63) as usize] as char);
        }
    }
    out
}

/// base64url 解码（容忍缺省填充）；只用于 JWT payload。
fn base64url_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for byte in text.trim_end_matches('=').bytes() {
        let value = match byte {
            b'A'..=b'Z' => (byte - b'A') as u32,
            b'a'..=b'z' => (byte - b'a' + 26) as u32,
            b'0'..=b'9' => (byte - b'0' + 52) as u32,
            b'-' => 62,
            b'_' => 63,
            other => return Err(format!("非法 base64url 字符: {}", other as char)),
        };
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Ok(out)
}

/// 两个 simple UUID 拼接：64 个 hex 字符，落在 RFC 7636 的 43–128 区间。
fn random_verifier() -> String {
    format!(
        "{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn token_with_exp(exp: i64) -> String {
        let payload = format!(r#"{{"exp":{exp}}}"#);
        format!(
            "{}.{}.sig",
            base64url_no_pad(br#"{"alg":"none"}"#),
            base64url_no_pad(payload.as_bytes())
        )
    }

    #[test]
    fn expired_jwt_is_detected_and_valid_jwt_passes() {
        let past = Utc::now().timestamp() - 60;
        let future = Utc::now().timestamp() + 3_600;
        assert!(jwt_expired(&token_with_exp(past)));
        assert!(!jwt_expired(&token_with_exp(future)));
        // 无 exp（0 同义）与非 JWT 形状：不本地判过期，交给 401 兜底。
        assert!(!jwt_expired(&token_with_exp(0)));
        assert!(!jwt_expired("not-a-jwt"));
    }

    #[test]
    fn base64url_decode_round_trips_full_byte_range() {
        let bytes: Vec<u8> = (0..=255_u8).collect();
        let encoded = base64url_no_pad(&bytes);
        assert!(!encoded.contains('='));
        assert_eq!(base64url_decode(&encoded).unwrap(), bytes);
    }

    #[test]
    fn login_url_carries_pkce_and_cli_params() {
        let url = login_url("chal", "uuid-1");
        assert!(url.starts_with("https://cursor.com/loginDeepControl?"), "{url}");
        assert!(url.contains("challenge=chal"), "{url}");
        assert!(url.contains("uuid=uuid-1"), "{url}");
        assert!(url.contains("mode=login"), "{url}");
        assert!(url.contains("redirectTarget=cli"), "{url}");
    }

    #[test]
    fn usage_parses_three_windows_and_cycle_end() {
        let value = json!({
            "planUsage": {
                "autoPercentUsed": 12.5,
                "apiPercentUsed": 40,
                "totalPercentUsed": 26.25
            },
            "billingCycleEnd": "1790000000000",
            "autoBucketModels": ["auto", "composer-1"]
        });
        let quotas = parse_usage(&value);
        assert_eq!(quotas.len(), 3);
        assert_eq!(quotas[0].label, "Cursor Models");
        assert_eq!(quotas[0].used_percent, 12.5);
        assert_eq!(quotas[1].label, "Other Models");
        assert_eq!(quotas[1].used_percent, 40.0);
        assert_eq!(quotas[2].label, "Total");
        assert_eq!(quotas[2].used_percent, 26.25);
        assert!(quotas
            .iter()
            .all(|quota| quota.resets_at == Some(1_790_000_000_000)));
        // 只报比例：金额字段不存在，绝不臆造。
        assert!(quotas
            .iter()
            .all(|quota| quota.used.is_none() && quota.total.is_none() && quota.remaining.is_none()));
    }

    #[test]
    fn usage_accepts_numeric_cycle_end_and_defaults_missing_percents() {
        let value = json!({
            "planUsage": { "totalPercentUsed": 7 },
            "billingCycleEnd": 1_790_000_000_000_i64
        });
        let quotas = parse_usage(&value);
        assert_eq!(quotas.len(), 3);
        assert_eq!(quotas[0].used_percent, 0.0);
        assert_eq!(quotas[1].used_percent, 0.0);
        assert_eq!(quotas[2].used_percent, 7.0);
        assert_eq!(quotas[0].resets_at, Some(1_790_000_000_000));
    }

    #[test]
    fn usage_without_percent_fields_is_empty() {
        assert!(parse_usage(&json!({ "billingCycleEnd": "1790000000000" })).is_empty());
        assert!(parse_usage(&json!({ "planUsage": {} })).is_empty());
        assert!(parse_usage(&json!({ "planUsage": { "other": 1 } })).is_empty());
    }

    #[test]
    fn usage_clamps_out_of_range_percent() {
        let value = json!({
            "planUsage": {
                "autoPercentUsed": 140.0,
                "apiPercentUsed": -3.0,
                "totalPercentUsed": 50.0
            }
        });
        let quotas = parse_usage(&value);
        assert_eq!(quotas[0].used_percent, 100.0);
        assert_eq!(quotas[1].used_percent, 0.0);
        assert_eq!(quotas[2].used_percent, 50.0);
    }

    #[test]
    fn catalog_offers_cursor_as_oauth_kind() {
        let kinds = super::super::list_kinds();
        let kind = kinds
            .iter()
            .find(|kind| kind.id == "cursor")
            .unwrap_or_else(|| panic!("cursor 未进订阅目录"));
        assert_eq!(kind.auth, "oauth");
        assert_eq!(kind.title, "Cursor");
    }

    /// 真实连通性冒烟：默认 rustls 直连 api2.cursor.sh 应得到干净的
    /// HTTP 状态（无效凭据 → 401），而不是 TLS / 防护层失败。
    /// `cargo test cursor -- --ignored` 单独运行。
    #[test]
    #[ignore = "needs network"]
    fn api2_reachable_with_default_tls() {
        let agent = agent();
        let error = connect_unary(
            &agent,
            "aiserver.v1.DashboardService/GetCurrentPeriodUsage",
            "invalid-token",
        )
        .err()
        .unwrap();
        assert!(
            error.contains("HTTP") || error.contains("过期"),
            "非 HTTP 层失败，疑似 TLS 被拦: {error}"
        );
    }
}
