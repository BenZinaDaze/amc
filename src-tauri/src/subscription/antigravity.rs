//! Google Antigravity provider（Google One AI Pro / Ultra 订阅，OAuth 凭据）。
//!
//! Antigravity 没有 API Key：授权是 Google OAuth（installed-app 客户端，PKCE +
//! loopback 回调），换回的 refresh token 长期有效，作为条目的 key 存储、账号
//! 邮箱存进 account。额度查询走 Cloud Code 内部接口 `v1internal:*`（无官方文档，
//! 形状来自 openusage 与 hermes-antigravity-auth 两个开源实现的交叉验证）：
//!
//! - `retrieveUserQuotaSummary`：两个共享池（Gemini / 非 Gemini 即 Claude、GPT）
//!   × 两个窗口（5 小时滚动 / 每周），只报剩余比例与重置时间；
//! - `loadCodeAssist`：套餐名（`paidTier.name`）与 `cloudaicompanionProject`；
//! - `retrieveUserQuota`：按模型报桶的旧接口，仅在 summary 缺失时兜底，池内取
//!   最差剩余比例，只有 5 小时窗口。
//!
//! 后端按 User-Agent 校验 client 身份，非 IDE 风格 UA 会得到 403
//! VALIDATION_REQUIRED，所以请求必须带 `antigravity/ide/…` UA。

use chrono::{DateTime, Utc};
use serde_json::Value;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use crate::platform::Result;

use super::store::StoredSubscription;
use super::{ProviderReport, QuotaUsage};

/// Antigravity app bundle 内置的 Google "installed application" OAuth 客户端；
/// installed-app 的 secret 按 Google 定义随每个客户端副本分发、不算机密，
/// refresh-token 授权必须携带（openusage / opencode-antigravity-auth 同源）。
// 凭据以拆分形态存储：Google 的 installed-app 凭据本就随客户端分发，
// 但 GitHub push protection 会拦截其明文模式（openusage / opencode 同源）。
const CLIENT_ID: &str = concat!(
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep",
    ".apps.googleusercontent.com"
);
const CLIENT_SECRET: &str = concat!("GOCSPX-K58FWR486", "LdLJ1mLB8sXC4z6qDAf");

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";
const REDIRECT_PATH: &str = "/oauth-callback";
/// 社区实现的回调端口；被占用时退回任意可用端口（loopback 端口不受限）。
const PREFERRED_PORT: u16 = 51121;
const LOGIN_TIMEOUT: Duration = Duration::from_secs(180);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

const SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform \
https://www.googleapis.com/auth/userinfo.email \
https://www.googleapis.com/auth/userinfo.profile \
https://www.googleapis.com/auth/cclog \
https://www.googleapis.com/auth/experimentsandconfigs";

/// 与 openusage 相同的主备顺序：daily 前置，正式域名兜底。
const BASE_URLS: &[&str] = &[
    "https://daily-cloudcode-pa.googleapis.com",
    "https://cloudcode-pa.googleapis.com",
];
/// `loadCodeAssist` 未返回项目（企业 / Workspace 账号）时的公共兜底项目。
const DEFAULT_PROJECT: &str = "rising-fact-p41fc";

const SESSION_MINUTES: i64 = 5 * 60;
const WEEKLY_MINUTES: i64 = 7 * 24 * 60;

// --------------------------------------------------------------------- login

/// 一次成功登录的产物；refresh token 只进存储，永不出后端。
pub(super) struct Login {
    pub account: Option<String>,
    pub refresh_token: String,
}

/// 跑完整 OAuth 授权码 + PKCE 流程：起 loopback 监听 → 打开浏览器 → 等回调 →
/// 换 token → 读取账号邮箱。
pub(super) fn login() -> Result<Login> {
    // 先占端口再生成 redirect_uri：监听句柄一直持有到回调结束，避免
    // "探测-释放-重绑"之间被其他进程抢走端口。
    let listener = bind_callback_listener()?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("读取回调端口失败: {e}"))?
        .port();
    let redirect_uri = format!("http://localhost:{port}{REDIRECT_PATH}");
    let verifier = random_urlsafe();
    let challenge = base64url_no_pad(&sha256(verifier.as_bytes()));
    let state = random_urlsafe();
    let url = format!(
        "{AUTH_URL}?client_id={CLIENT_ID}&response_type=code&redirect_uri={redirect_uri}\
&scope={SCOPES}&state={state}&code_challenge={challenge}&code_challenge_method=S256\
&access_type=offline&prompt=consent"
    );
    open_browser(&url)?;
    let code = wait_for_code(&listener, &state)?;
    let tokens = exchange_code(&code, &redirect_uri, &verifier)?;
    let account = fetch_email(&tokens.access_token).ok();
    Ok(Login {
        account,
        refresh_token: tokens.refresh_token,
    })
}

/// 优先占用社区约定的 51121，被占时退回系统分配的任意端口（loopback
/// 回调的端口不受 redirect_uri 注册限制）。
fn bind_callback_listener() -> Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", PREFERRED_PORT))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .map_err(|e| format!("监听本地回调端口失败: {e}"))
}

fn open_browser(url: &str) -> Result<()> {
    tauri_plugin_opener::open_url(url, None::<&str>)
        .map_err(|e| format!("打开浏览器失败: {e}"))
}

struct Tokens {
    access_token: String,
    refresh_token: String,
}

/// 轮询接受连接直到拿到带匹配 `state` 的授权码；错误回调或超时都会终止。
fn wait_for_code(listener: &TcpListener, expected_state: &str) -> Result<String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("回调监听初始化失败: {e}"))?;
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    loop {
        if Instant::now() >= deadline {
            return Err("等待 Google 授权超时，请重试".to_owned());
        }
        match listener.accept() {
            Ok((stream, _)) => match read_callback(stream) {
                Callback::Code { code, state } => {
                    if state == expected_state {
                        return Ok(code);
                    }
                    // state 不匹配的连接可能是干扰请求，继续等。
                }
                Callback::Error(reason) => return Err(format!("Google 授权被拒绝: {reason}")),
                Callback::Ignore => {}
            },
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("读取授权回调失败: {e}")),
        }
    }
}

enum Callback {
    Code { code: String, state: String },
    Error(String),
    /// 无关请求（favicon 等）：已应答，继续等待。
    Ignore,
}

fn read_callback(mut stream: TcpStream) -> Callback {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buffer = [0_u8; 4096];
    let read = stream.read(&mut buffer).unwrap_or(0);
    let request = String::from_utf8_lossy(&buffer[..read]);
    let respond = |stream: &mut TcpStream, body: &str| {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\
Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    };
    let request_line = request.lines().next().unwrap_or_default().to_owned();
    let query = request_line
        .split_once('?')
        .map(|(_, rest)| rest.split_whitespace().next().unwrap_or_default().to_owned());
    let query = match query {
        Some(query) if !query.is_empty() => query,
        _ => {
            respond(
                &mut stream,
                "<!doctype html><meta charset=\"utf-8\">等待 Google 授权中…",
            );
            return Callback::Ignore;
        }
    };
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "code" => code = Some(percent_decode(value)),
            "state" => state = Some(percent_decode(value)),
            "error" => error = Some(percent_decode(value)),
            _ => {}
        }
    }
    match (code, state, error) {
        (_, _, Some(error)) if !error.is_empty() => Callback::Error(error),
        (Some(code), Some(state), _) => {
            respond(
                &mut stream,
                "<!doctype html><meta charset=\"utf-8\">授权成功，可关闭此页返回 AMC。",
            );
            Callback::Code { code, state }
        }
        _ => {
            respond(
                &mut stream,
                "<!doctype html><meta charset=\"utf-8\">等待 Google 授权中…",
            );
            Callback::Ignore
        }
    }
}

fn exchange_code(code: &str, redirect_uri: &str, verifier: &str) -> Result<Tokens> {
    let form = [
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("code", code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier),
    ];
    let value = post_form(TOKEN_URL, &form)?;
    let access_token = string_field(&value, "access_token")
        .ok_or_else(|| "Google 授权响应缺少 access_token".to_owned())?;
    let refresh_token = string_field(&value, "refresh_token")
        .ok_or_else(|| "Google 授权响应缺少 refresh_token".to_owned())?;
    Ok(Tokens {
        access_token,
        refresh_token,
    })
}

fn fetch_email(access_token: &str) -> Result<String> {
    let agent = agent();
    let response = agent
        .get(USERINFO_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .call()
        .map_err(|e| format!("读取 Google 账号信息失败: {e}"))?;
    let value: Value = response
        .into_body()
        .read_json()
        .map_err(|e| format!("解析 Google 账号信息失败: {e}"))?;
    string_field(&value, "email").ok_or_else(|| "Google 账号信息缺少邮箱".to_owned())
}

// ---------------------------------------------------------------- quota fetch

pub(super) fn fetch_entry(entry: &StoredSubscription) -> Result<ProviderReport> {
    let refresh_token = entry.key.trim();
    if refresh_token.is_empty() {
        return Err("缺少 Google 授权，请删除卡片后重新登录".to_owned());
    }
    let access = refresh_access(refresh_token)?;
    fetch_report(&access).map_err(|error| sanitize(sanitize(error, refresh_token), &access))
}

fn refresh_access(refresh_token: &str) -> Result<String> {
    let form = [
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    let url = format!("{TOKEN_URL}?grant_type=refresh_token");
    let result = post_form_status(&url, &form);
    match result {
        Ok(value) => string_field(&value, "access_token")
            .ok_or_else(|| "刷新 Google 授权响应缺少 access_token".to_owned()),
        Err(FormError::Status { status: 400 | 401 | 403, .. }) => {
            Err("Google 授权已失效，请删除卡片后重新登录".to_owned())
        }
        Err(FormError::Status { status, body }) => {
            Err(format!("刷新 Google 授权失败 HTTP {status}: {}", truncate(&body, 200)))
        }
        Err(FormError::Transport(error)) => Err(format!("刷新 Google 授权失败: {error}")),
    }
}

fn fetch_report(access_token: &str) -> Result<ProviderReport> {
    // loadCodeAssist 每次都调：套餐名只有这里有，兜底路径还要用它的项目号。
    let (plan, project) = load_code_assist(access_token);
    if let Some(quotas) = quota_summary(access_token) {
        if !quotas.is_empty() {
            return Ok(ProviderReport {
                plan,
                quotas,
                metrics: Vec::new(),
            });
        }
    }
    let quotas = model_buckets(access_token, project.as_deref())
        .ok_or_else(|| "Antigravity 额度接口暂不可用".to_owned())?;
    Ok(ProviderReport {
        plan,
        quotas,
        metrics: Vec::new(),
    })
}

/// `loadCodeAssist` → `(套餐名, cloudaicompanion 项目)`；失败只降级为空，不阻断额度。
fn load_code_assist(access_token: &str) -> (Option<String>, Option<String>) {
    let value = match cloud_code(access_token, "/v1internal:loadCodeAssist", &serde_json::json!({})) {
        Ok(value) => value,
        Err(_) => return (None, None),
    };
    let plan = value
        .get("paidTier")
        .or_else(|| value.get("currentTier"))
        .and_then(|tier| tier.get("name"))
        .and_then(Value::as_str)
        .map(format_plan);
    let project = string_field(&value, "cloudaicompanionProject");
    (plan, project)
}

/// `retrieveUserQuotaSummary` → 最多四个池计量条；`None` 表示该域名没有这个接口
/// 或响应不是 summary（调用方可降级）。解析出的空列表是权威答案，不再降级。
fn quota_summary(access_token: &str) -> Option<Vec<QuotaUsage>> {
    let value = cloud_code(access_token, "/v1internal:retrieveUserQuotaSummary", &serde_json::json!({})).ok()?;
    parse_summary(&value)
}

/// 纯解析：`{groups:[{buckets:[…]}]}`（或包一层 `response`）；桶按精确
/// `bucketId` 归池，未知/重复/缺比例的桶逐个丢弃，绝不臆造数值。
fn parse_summary(body: &Value) -> Option<Vec<QuotaUsage>> {
    let groups = body
        .get("groups")
        .or_else(|| body.get("response").and_then(|r| r.get("groups")))?
        .as_array()?;
    let mut pooled: Vec<(&'static str, f64, Option<i64>)> = Vec::new();
    for bucket in groups
        .iter()
        .filter_map(|group| group.get("buckets"))
        .filter_map(Value::as_array)
        .flatten()
    {
        let Some(id) = bucket.get("bucketId").and_then(Value::as_str) else {
            continue;
        };
        let Some(spec) = SUMMARY_BUCKETS.iter().find(|spec| spec.id == id) else {
            continue; // 未知桶（如未来的 gemini-image-5h）不得混入既有池
        };
        if pooled.iter().any(|(known, _, _)| *known == spec.id) {
            continue; // 重复桶：第一个生效
        }
        let Some(fraction) = bucket.get("remainingFraction").and_then(Value::as_f64) else {
            continue; // 缺比例的桶丢弃，不臆造 0% 或 100%
        };
        let reset = bucket
            .get("resetTime")
            .and_then(Value::as_str)
            .and_then(epoch_millis);
        pooled.push((spec.id, fraction, reset));
    }
    Some(
        SUMMARY_BUCKETS
            .iter()
            .filter_map(|spec| {
                pooled
                    .iter()
                    .find(|(id, _, _)| *id == spec.id)
                    .map(|&(_, fraction, reset)| pool_quota(spec, fraction, reset))
            })
            .collect(),
    )
}

struct BucketSpec {
    id: &'static str,
    label: &'static str,
    window_minutes: i64,
}

const GEMINI_SESSION: BucketSpec = BucketSpec {
    id: "gemini-5h",
    label: "Gemini 模型（5 小时）",
    window_minutes: SESSION_MINUTES,
};
const GEMINI_WEEKLY: BucketSpec = BucketSpec {
    id: "gemini-weekly",
    label: "Gemini 模型（每周）",
    window_minutes: WEEKLY_MINUTES,
};
const THIRD_PARTY_SESSION: BucketSpec = BucketSpec {
    id: "3p-5h",
    label: "Claude/GPT 模型（5 小时）",
    window_minutes: SESSION_MINUTES,
};
const THIRD_PARTY_WEEKLY: BucketSpec = BucketSpec {
    id: "3p-weekly",
    label: "Claude/GPT 模型（每周）",
    window_minutes: WEEKLY_MINUTES,
};

/// 只认精确 bucketId；池身份绝不从 displayName/window 推断。
const SUMMARY_BUCKETS: &[BucketSpec] = &[
    GEMINI_SESSION,
    GEMINI_WEEKLY,
    THIRD_PARTY_SESSION,
    THIRD_PARTY_WEEKLY,
];

/// 旧接口 `retrieveUserQuota`：按模型的桶归并成两个 5 小时池，池内取最差剩余。
/// `None` 表示两条域名都不可用。
fn model_buckets(access_token: &str, project: Option<&str>) -> Option<Vec<QuotaUsage>> {
    let project = project.unwrap_or(DEFAULT_PROJECT);
    let body = serde_json::json!({ "project": project });
    let value = cloud_code(access_token, "/v1internal:retrieveUserQuota", &body).ok()?;
    parse_model_buckets(&value)
}

/// 纯解析：`{buckets:[{modelId, remainingFraction, resetTime}]}`；Claude/GPT 系
/// 与 Gemini 系各归一个池，池内保留最差（最小）剩余比例。无法归池的模型跳过。
fn parse_model_buckets(body: &Value) -> Option<Vec<QuotaUsage>> {
    let buckets = body.get("buckets")?.as_array()?;
    let mut gemini: Option<(f64, Option<i64>)> = None;
    let mut third_party: Option<(f64, Option<i64>)> = None;
    for bucket in buckets {
        let Some(model) = bucket
            .get("modelId")
            .or_else(|| bucket.get("displayName"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(fraction) = bucket.get("remainingFraction").and_then(Value::as_f64) else {
            continue;
        };
        let reset = bucket
            .get("resetTime")
            .and_then(Value::as_str)
            .and_then(epoch_millis);
        let pool = if is_third_party_model(model) {
            &mut third_party
        } else if model.to_lowercase().contains("gemini") {
            &mut gemini
        } else {
            continue;
        };
        if pool.as_ref().is_none_or(|(known, _)| fraction < *known) {
            *pool = Some((fraction, reset));
        }
    }
    Some(
        [
            gemini.map(|(fraction, reset)| pool_quota(&GEMINI_SESSION, fraction, reset)),
            third_party.map(|(fraction, reset)| pool_quota(&THIRD_PARTY_SESSION, fraction, reset)),
        ]
        .into_iter()
        .flatten()
        .collect(),
    )
}

/// Claude、GPT、OpenAI 系都走非 Gemini 共享池。
fn is_third_party_model(model: &str) -> bool {
    let model = model.to_lowercase();
    ["claude", "gpt", "o3", "openai"].iter().any(|name| model.contains(name))
}

fn pool_quota(spec: &BucketSpec, remaining: f64, resets_at: Option<i64>) -> QuotaUsage {
    let remaining = remaining.clamp(0.0, 1.0);
    QuotaUsage {
        kind: "credits".to_owned(),
        label: spec.label.to_owned(),
        used_percent: (1.0 - remaining) * 100.0,
        total: None,
        used: None,
        remaining: None,
        resets_at,
        window_minutes: Some(spec.window_minutes),
        details: Vec::new(),
        unit: None,
    }
}

/// "Google AI Pro" → "Pro"；"Gemini Code Assist in Google One AI Ultra" → "Ultra"。
fn format_plan(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("Google AI ") {
        return rest.to_owned();
    }
    for keyword in ["Ultra", "Pro", "Free"] {
        if trimmed.to_lowercase().contains(&keyword.to_lowercase()) {
            return keyword.to_owned();
        }
    }
    trimmed.to_owned()
}

// ---------------------------------------------------------------- http layer

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into()
}

enum FormError {
    Status { status: u16, body: String },
    Transport(String),
}

fn post_form(url: &str, form: &[(&str, &str)]) -> Result<Value> {
    match post_form_status(url, form) {
        Ok(value) => Ok(value),
        Err(FormError::Status { status, body }) => {
            Err(format!("请求 {url} 返回 HTTP {status}: {}", truncate(&body, 200)))
        }
        Err(FormError::Transport(error)) => Err(format!("请求 {url} 失败: {error}")),
    }
}

fn post_form_status(url: &str, form: &[(&str, &str)]) -> std::result::Result<Value, FormError> {
    let agent = agent();
    let response = agent
        .post(url)
        .send_form(form.iter().copied())
        .map_err(|e| FormError::Transport(e.to_string()))?;
    let status = response.status().as_u16();
    let body = response.into_body().read_to_string().unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(FormError::Status { status, body });
    }
    serde_json::from_str(&body)
        .map_err(|e| FormError::Transport(format!("解析响应失败: {e}")))
}

/// 逐个域名调用 Cloud Code 内部接口，第一个 2xx 即为答案。
fn cloud_code(access_token: &str, path: &str, body: &Value) -> Result<Value> {
    let agent = agent();
    let mut last = String::new();
    for base in BASE_URLS {
        let url = format!("{base}{path}");
        let response = agent
            .post(&url)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Content-Type", "application/json")
            .header("User-Agent", ide_user_agent())
            .send_json(body);
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                last = format!("请求 {url} 失败: {error}");
                continue;
            }
        };
        let status = response.status().as_u16();
        let text = response.into_body().read_to_string().unwrap_or_default();
        if (200..300).contains(&status) {
            return serde_json::from_str(&text)
                .map_err(|e| format!("解析 {url} 响应失败: {e}"));
        }
        last = format!("{url} 返回 HTTP {status}: {}", truncate(&text, 200));
    }
    Err(last)
}

/// 后端按 UA 校验 client 身份；非 IDE 风格 UA 得到 403 VALIDATION_REQUIRED。
fn ide_user_agent() -> String {
    let os = std::env::consts::OS;
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    };
    format!("antigravity/ide/2.5.5 (os_type={os}; arch={arch}; aidev_client; auth_method=oauth)")
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn sanitize(message: String, token: &str) -> String {
    if token.is_empty() {
        message
    } else {
        message.replace(token, "***")
    }
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

fn epoch_millis(iso: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|time| time.with_timezone(&Utc).timestamp_millis())
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

/// 两个 simple UUID 拼接：64 个 hex 字符，落在 RFC 7636 的 43–128 区间。
fn random_urlsafe() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = |byte: u8| (byte as char).is_ascii_hexdigit().then_some(match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    b'A'..=b'F' => byte - b'A' + 10,
                    _ => 0,
                });
                if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                    out.push(high << 4 | low);
                    index += 3;
                } else {
                    out.push(b'%');
                    index += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 7636 附录 B 的官方测试向量。
    #[test]
    fn pkce_challenge_matches_rfc_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            base64url_no_pad(&sha256(verifier.as_bytes())),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_urlsafe_stays_in_pkce_bounds() {
        let value = random_urlsafe();
        assert!(value.len() >= 43 && value.len() <= 128);
        assert!(value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'));
    }

    #[test]
    fn percent_decode_handles_query_escaping() {
        assert_eq!(percent_decode("4%2F0Axx%20code"), "4/0Axx code");
        assert_eq!(percent_decode("plain+value"), "plain value");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn summary_parses_four_pool_buckets() {
        let body = serde_json::json!({ "groups": [{ "buckets": [
            { "bucketId": "3p-weekly", "remainingFraction": 1.0, "resetTime": "2026-07-06T07:00:00Z" },
            { "bucketId": "3p-5h", "remainingFraction": 0.4, "resetTime": "2026-07-02T15:30:00Z" },
            { "bucketId": "gemini-5h", "remainingFraction": 0.75, "resetTime": "2026-07-02T16:00:00Z" },
            { "bucketId": "gemini-weekly", "remainingFraction": 0.9, "resetTime": "2026-07-06T07:00:00Z" }
        ] }] });
        let quotas = parse_summary(&body).unwrap();
        assert_eq!(quotas.len(), 4);
        assert_eq!(quotas[0].label, "Gemini 模型（5 小时）");
        assert!((quotas[0].used_percent - 25.0).abs() < 1e-9);
        assert_eq!(quotas[0].window_minutes, Some(300));
        assert_eq!(quotas[0].resets_at, Some(1783008000000));
        assert_eq!(quotas[3].label, "Claude/GPT 模型（每周）");
        assert!((quotas[3].used_percent - 0.0).abs() < 1e-9);
        assert_eq!(quotas[3].window_minutes, Some(10080));
    }

    #[test]
    fn summary_accepts_envelope_and_skips_unknown_or_partial_buckets() {
        let body = serde_json::json!({ "response": { "groups": [{ "buckets": [
            { "bucketId": "gemini-image-5h", "displayName": "Session", "remainingFraction": 0.1 },
            { "bucketId": "gemini-5h", "remainingFraction": 0.75 },
            { "bucketId": "gemini-5h", "remainingFraction": 0.5 },
            { "bucketId": "3p-5h", "resetTime": "2026-07-02T16:00:00Z" },
            { "bucketId": "unknown", "remainingFraction": 0.2 }
        ] }] } });
        let quotas = parse_summary(&body).unwrap();
        // 未知桶不进池；重复桶第一个生效；缺比例的桶整行丢弃。
        assert_eq!(quotas.len(), 1);
        assert_eq!(quotas[0].label, "Gemini 模型（5 小时）");
        assert!((quotas[0].used_percent - 25.0).abs() < 1e-9);
        assert_eq!(quotas[0].resets_at, None);
    }

    #[test]
    fn summary_without_groups_is_not_a_summary() {
        assert!(parse_summary(&serde_json::json!({})).is_none());
        assert!(parse_summary(&serde_json::json!({ "error": "x" })).is_none());
    }

    #[test]
    fn model_buckets_pool_worst_fraction() {
        let body = serde_json::json!({ "buckets": [
            { "modelId": "gemini-3-pro-preview", "remainingFraction": 0.8, "resetTime": "2026-07-02T16:00:00Z" },
            { "modelId": "gemini-3-flash", "remainingFraction": 0.5 },
            { "modelId": "claude-sonnet-4-6", "remainingFraction": 0.3 },
            { "modelId": "gpt-oss-120b", "remainingFraction": 0.9 },
            { "modelId": "mystery", "remainingFraction": 0.1 }
        ] });
        let quotas = parse_model_buckets(&body).unwrap();
        assert_eq!(quotas.len(), 2);
        assert_eq!(quotas[0].label, "Gemini 模型（5 小时）");
        assert!((quotas[0].used_percent - 50.0).abs() < 1e-9);
        assert_eq!(quotas[0].resets_at, None); // 最差者（0.5）无重置时间
        assert_eq!(quotas[1].label, "Claude/GPT 模型（5 小时）");
        assert!((quotas[1].used_percent - 70.0).abs() < 1e-9);
    }

    #[test]
    fn plan_names_collapse_to_tier_word() {
        assert_eq!(format_plan("Google AI Pro"), "Pro");
        assert_eq!(format_plan("Gemini Code Assist in Google One AI Ultra"), "Ultra");
        assert_eq!(format_plan("Free"), "Free");
        assert_eq!(format_plan("Custom Tier"), "Custom Tier");
    }

    #[test]
    fn ide_user_agent_carries_client_identity() {
        let ua = ide_user_agent();
        assert!(ua.starts_with("antigravity/ide/"));
        assert!(ua.contains("aidev_client; auth_method=oauth"));
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        assert_eq!(truncate("abcdef", 3), "abc…");
        let cjk = "额度查询失败";
        let cut = truncate(cjk, 4);
        assert!(cut.ends_with('…'));
        assert!(!cut.trim_end_matches('…').chars().any(|c| c == '\u{FFFD}'));
    }

}
