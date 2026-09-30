//! Subscription quota providers.
//!
//! One stored entry ([`StoredSubscription`]) is one subscription instance; the
//! same vendor may be added several times with different keys and names. Each
//! entry is turned into the normalized [`SubscriptionStatus`] rendered as an
//! overview card. Credentials never cross the Tauri boundary; only labels,
//! counts and percentages do. New vendors plug in by producing
//! [`ProviderReport`] from an entry and adding a match arm in [`fetch_entry`].

use crate::omp::Result;
use chrono::{DateTime, Local, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------- normalized

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionStatus {
    /// Stored entry id; addresses the card for edit/remove.
    pub id: String,
    /// Provider kind, e.g. `glm`.
    pub provider: String,
    /// User-defined card name.
    pub title: String,
    /// `zai` | `bigmodel`; prefills the credential form.
    pub platform: String,
    /// Masked key tail, e.g. `…a1b2`.
    pub key_hint: Option<String>,
    pub error: Option<String>,
    pub plan: Option<String>,
    pub quotas: Vec<QuotaUsage>,
    pub metrics: Vec<MetricUsage>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct QuotaUsage {
    /// `tokens` | `credits` | `mcp`; drives formatting and icons in the UI.
    pub kind: String,
    pub label: String,
    pub used_percent: f64,
    pub total: Option<i64>,
    pub used: Option<i64>,
    pub remaining: Option<i64>,
    pub resets_at: Option<i64>,
    pub window_minutes: Option<i64>,
    pub details: Vec<QuotaDetail>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct QuotaDetail {
    pub name: String,
    pub usage: i64,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MetricUsage {
    pub id: String,
    pub label: String,
    pub value: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub message: String,
}

pub struct ProviderReport {
    pub plan: Option<String>,
    pub quotas: Vec<QuotaUsage>,
    pub metrics: Vec<MetricUsage>,
}

/// One row of the vendor catalog shown in the add-plan form; the credential
/// fields render from these, so picking a kind swaps its own fields in.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionKind {
    pub id: &'static str,
    /// Default card name suggested when adding this kind.
    pub title: &'static str,
    /// Label above the credential input.
    pub key_label: &'static str,
    /// Placeholder inside the credential input.
    pub key_placeholder: &'static str,
    /// `(value, label)` endpoint choices; the first is the default.
    pub platforms: &'static [(&'static str, &'static str)],
}

/// Vendor catalog; appending a provider here plus a `fetch_entry` arm is all
/// a key-based vendor needs.
const KINDS: &[SubscriptionKind] = &[SubscriptionKind {
    id: "glm",
    title: "GLM Coding Plan",
    key_label: "GLM API Key",
    key_placeholder: "粘贴 GLM API Key",
    platforms: &[
        ("zai", "Z.ai（国际版）"),
        ("bigmodel", "智谱 BigModel（中国）"),
    ],
}];

/// Catalog for the add-plan form; one row per vendor.
pub fn list_kinds() -> Vec<SubscriptionKind> {
    KINDS.to_vec()
}

fn known_kind(kind: &str) -> bool {
    KINDS.iter().any(|known| known.id == kind)
}

/// Platforms are per-kind: the catalog is the single source of truth.
fn validate_platform(kind: &str, platform: &str) -> Result<String> {
    KINDS
        .iter()
        .find(|known| known.id == kind)
        .is_some_and(|known| known.platforms.iter().any(|(value, _)| *value == platform))
        .then(|| platform.to_owned())
        .ok_or_else(|| "未知的平台类型".to_owned())
}

/// Turns one stored entry into its vendor report; the plug-in point for new
/// providers.
fn fetch_entry(entry: &StoredSubscription) -> Result<ProviderReport> {
    match entry.kind.as_str() {
        "glm" => {
            let credential = entry.glm_credential()?;
            fetch_glm(&credential).map_err(|error| sanitize(error, &credential.token))
        }
        other => Err(format!("未知的订阅套餐: {other}")),
    }
}

fn subscription_status(entry: &StoredSubscription) -> SubscriptionStatus {
    SubscriptionStatus {
        id: entry.id.clone(),
        provider: entry.kind.clone(),
        title: entry.name.clone(),
        platform: entry.platform.clone(),
        key_hint: Some(entry.hint()),
        error: None,
        plan: None,
        quotas: Vec::new(),
        metrics: Vec::new(),
    }
}

/// One card per entry, queried in parallel so several keys stay responsive.
pub fn fetch_all(data_dir: &Path) -> Vec<SubscriptionStatus> {
    let stored = read_stored(data_dir);
    std::thread::scope(|scope| {
        let handles: Vec<_> = stored
            .entries
            .iter()
            .map(|entry| {
                scope.spawn(move || match fetch_entry(entry) {
                    Ok(report) => SubscriptionStatus {
                        plan: report.plan,
                        quotas: report.quotas,
                        metrics: report.metrics,
                        ..subscription_status(entry)
                    },
                    Err(error) => SubscriptionStatus {
                        error: Some(error),
                        ..subscription_status(entry)
                    },
                })
            })
            .collect();
        handles
            .into_iter()
            .zip(&stored.entries)
            .map(|(handle, entry)| {
                handle.join().unwrap_or_else(|_| SubscriptionStatus {
                    error: Some("查询订阅失败".to_owned()),
                    ..subscription_status(entry)
                })
            })
            .collect()
    })
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_millis() as i64)
}

// ----------------------------------------------------------- stored entries

/// Subscriptions the user saved inside AMC, persisted with owner-only
/// permissions under the app data dir. The frontend never receives keys.
const SUBSCRIPTIONS_FILE: &str = "subscriptions.json";

/// One subscription instance; a vendor may appear several times.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSubscription {
    /// Unique among the current entries: `"1"`, `"2"`, …
    pub id: String,
    /// Provider kind, e.g. `glm`; see [`known_kind`].
    pub kind: String,
    /// User-defined card name.
    pub name: String,
    /// `zai` (api.z.ai) or `bigmodel` (open.bigmodel.cn).
    pub platform: String,
    pub key: String,
}

impl StoredSubscription {
    fn hint(&self) -> String {
        let tail: String = self.key.chars().rev().take(4).collect();
        format!("…{}", tail.chars().rev().collect::<String>())
    }

    fn glm_credential(&self) -> Result<GlmCredential> {
        let host = platform_host(&self.platform).ok_or_else(|| "未知的平台类型".to_owned())?;
        Ok(GlmCredential {
            base_url: format!("https://{host}"),
            token: self.key.clone(),
        })
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StoredSubscriptions {
    /// Monotonic counter behind [`next_entry_id`]; ids are never reused.
    #[serde(default)]
    next_id: u64,
    #[serde(default)]
    entries: Vec<StoredSubscription>,
    /// Legacy single-plan shape, migrated by [`read_stored`] and dropped on
    /// the next save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    glm: Option<LegacyCredential>,
    #[serde(default)]
    plans: Vec<String>,
}

/// Credential fields of pre-multi-entry files.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyCredential {
    platform: String,
    key: String,
}

/// Older builds stored one optional credential plus a plan-id list; fold that
/// into a single entry so existing keys keep working.
fn migrate_legacy(mut stored: StoredSubscriptions) -> StoredSubscriptions {
    if stored.entries.is_empty()
        && stored.plans.iter().any(|id| id == "glm")
        && stored.glm.as_ref().is_some_and(|glm| !glm.key.trim().is_empty())
    {
        if let Some(legacy) = stored.glm.take() {
            stored.entries.push(StoredSubscription {
                id: "1".to_owned(),
                kind: "glm".to_owned(),
                name: "GLM Coding Plan".to_owned(),
                platform: legacy.platform,
                key: legacy.key,
            });
        }
    }
    stored
}

/// `zai` | `bigmodel` → monitor API host; rejects unknown values.
fn platform_host(platform: &str) -> Option<&'static str> {
    match platform {
        "zai" => Some(ZAI_HOST),
        "bigmodel" => Some(BIGMODEL_HOST),
        _ => None,
    }
}

fn read_stored(data_dir: &Path) -> StoredSubscriptions {
    let mut stored = fs::read_to_string(data_dir.join(SUBSCRIPTIONS_FILE))
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .map_or_else(Default::default, migrate_legacy);
    // Counter-less files (legacy or hand-written) would otherwise fall back to
    // surviving-entry ids; lift the counter above every stored id before any
    // deletion can empty the list and a re-add reuse an id.
    let highest = stored
        .entries
        .iter()
        .filter_map(|entry| entry.id.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    stored.next_id = stored.next_id.max(highest);
    stored
}

fn write_stored_entries(data_dir: &Path, stored: StoredSubscriptions) -> Result<()> {
    fs::create_dir_all(data_dir).map_err(|e| format!("创建数据目录失败: {e}"))?;
    let path = data_dir.join(SUBSCRIPTIONS_FILE);
    let mut clean = stored;
    clean.glm = None;
    clean.plans = Vec::new();
    let bytes =
        serde_json::to_vec_pretty(&clean).map_err(|e| format!("序列化失败: {e}"))?;
    write_private(&path, &bytes)
}

fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("套餐名称不能为空".to_owned());
    }
    if name.chars().count() > 100 {
        return Err("套餐名称过长".to_owned());
    }
    Ok(name.to_owned())
}

fn validate_key(key: &str) -> Result<String> {
    let key = key.trim();
    if key.len() < 8 || key.chars().any(char::is_whitespace) {
        return Err("GLM Key 格式无效".to_owned());
    }
    Ok(key.to_owned())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut file = options
        .open(path)
        .map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    file.write_all(bytes)
        .map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

// -------------------------------------------------------------- entry CRUD

pub fn add_plan(data_dir: &Path, kind: &str, name: &str, platform: &str, key: &str) -> Result<()> {
    if !known_kind(kind) {
        return Err(format!("未知的订阅套餐: {kind}"));
    }
    let name = validate_name(name)?;
    let platform = validate_platform(kind, platform)?;
    let key = validate_key(key)?;
    let mut stored = read_stored(data_dir);
    let id = next_entry_id(&mut stored);
    stored.entries.push(StoredSubscription {
        id,
        kind: kind.to_owned(),
        name,
        platform,
        key,
    });
    write_stored_entries(data_dir, stored)
}

/// A `None` key keeps the stored one, so edits never need the raw key.
pub fn update_plan(
    data_dir: &Path,
    id: &str,
    name: &str,
    platform: &str,
    key: Option<&str>,
) -> Result<()> {
    let name = validate_name(name)?;
    let key = key.map(validate_key).transpose()?;
    let mut stored = read_stored(data_dir);
    let entry = stored
        .entries
        .iter_mut()
        .find(|entry| entry.id == id)
        .ok_or_else(|| "订阅套餐不存在".to_owned())?;
    entry.platform = validate_platform(&entry.kind, platform)?;
    entry.name = name;
    if let Some(key) = key {
        entry.key = key;
    }
    write_stored_entries(data_dir, stored)
}

pub fn remove_plan(data_dir: &Path, id: &str) -> Result<()> {
    let mut stored = read_stored(data_dir);
    let before = stored.entries.len();
    stored.entries.retain(|entry| entry.id != id);
    if stored.entries.len() != before {
        write_stored_entries(data_dir, stored)?;
    }
    Ok(())
}

/// Entry ids count up monotonically and are never reused, so a stale card
/// can never act on a different subscription after a delete + re-add.
/// Invariant: [`read_stored`] lifts `next_id` above every stored id before
/// any mutation happens.
fn next_entry_id(stored: &mut StoredSubscriptions) -> String {
    stored.next_id += 1;
    stored.next_id.to_string()
}

// ------------------------------------------------------------ GLM Coding Plan

const ZAI_HOST: &str = "api.z.ai";
const BIGMODEL_HOST: &str = "open.bigmodel.cn";

struct GlmCredential {
    base_url: String,
    token: String,
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

    let query = usage_window_query();
    let mut metrics = Vec::new();
    if let Ok(model_root) = get_json(
        &format!("{base}/api/monitor/usage/model-usage?{query}"),
        token,
    ) {
        metrics.extend(model_metrics(data_or_root(&model_root)));
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
    #[serde(default)]
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

fn model_metrics(data: &Value) -> Vec<MetricUsage> {
    let total = data.get("totalUsage");
    let mut metrics = Vec::new();
    if let Some(tokens) = total.and_then(|t| t.get("totalTokensUsage")).and_then(Value::as_i64) {
        metrics.push(MetricUsage {
            id: "tokens".to_owned(),
            label: "Token 用量".to_owned(),
            value: tokens,
        });
    }
    if let Some(calls) = total
        .and_then(|t| t.get("totalModelCallCount"))
        .and_then(Value::as_i64)
    {
        metrics.push(MetricUsage {
            id: "requests".to_owned(),
            label: "模型请求".to_owned(),
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

/// Rolling window used by Z.ai's own plugin: yesterday at the current hour to
/// the end of the current hour, formatted as local naive timestamps.
fn usage_window_query() -> String {
    let now = Local::now();
    let start = (now - chrono::Duration::hours(24))
        .with_minute(0)
        .and_then(|time| time.with_second(0))
        .and_then(|time| time.with_nanosecond(0));
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

// -------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

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
            model_metrics(&model),
            vec![
                MetricUsage { id: "tokens".into(), label: "Token 用量".into(), value: 50925756 },
                MetricUsage { id: "requests".into(), label: "模型请求".into(), value: 318 },
                MetricUsage { id: "model".into(), label: "GLM-5.3-Flash".into(), value: 44035108 },
                MetricUsage { id: "model".into(), label: "GLM-5.3".into(), value: 6890648 },
            ]
        );
        let no_models = json!({ "totalUsage": { "totalModelCallCount": 3, "totalTokensUsage": 100 } });
        assert_eq!(model_metrics(&no_models).len(), 2);
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
        let query = usage_window_query();
        assert!(query.starts_with("startTime=20"));
        assert!(query.contains("&endTime=20"));
        assert!(!query.contains(' '));
    }

    #[test]
    fn sanitize_strips_tokens() {
        assert_eq!(sanitize("failed sk-secret".into(), "sk-secret"), "failed ***");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("amc-stored-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn entry_crud_and_validation() {
        let dir = temp_dir("crud");
        assert!(add_plan(&dir, "glm", "  ", "zai", "12345678").is_err());
        assert!(add_plan(&dir, "nope", "名", "zai", "12345678").is_err());
        assert!(add_plan(&dir, "glm", "名", "unknown", "12345678").is_err());
        assert!(add_plan(&dir, "glm", "名", "zai", "short").is_err());
        assert!(fetch_all(&dir).is_empty());

        // The same vendor can be added twice with different keys.
        add_plan(&dir, "glm", "  主号  ", "zai", "  12345678abcdef  ").unwrap();
        add_plan(&dir, "glm", "备用", "bigmodel", "fedcba9876543210").unwrap();
        let entries = read_stored(&dir).entries;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "1");
        assert_eq!(entries[1].id, "2");
        assert_eq!(entries[0].name, "主号");
        assert_eq!(entries[0].platform, "zai");
        assert_eq!(entries[0].key, "12345678abcdef");
        assert_eq!(entries[0].hint(), "…cdef");

        // A missing key keeps the stored one; a blank one is rejected.
        update_plan(&dir, "1", "主力", "bigmodel", None).unwrap();
        assert!(update_plan(&dir, "1", "主力", "bigmodel", Some("  ")).is_err());
        let entry = &read_stored(&dir).entries[0];
        assert_eq!(entry.name, "主力");
        assert_eq!(entry.platform, "bigmodel");
        assert_eq!(entry.key, "12345678abcdef");
        update_plan(&dir, "1", "主力", "zai", Some("aaaa1234")).unwrap();
        assert_eq!(read_stored(&dir).entries[0].key, "aaaa1234");
        assert!(update_plan(&dir, "99", "x", "zai", None).is_err());

        remove_plan(&dir, "1").unwrap();
        assert_eq!(read_stored(&dir).entries.len(), 1);
        remove_plan(&dir, "1").unwrap(); // removing again is a no-op

        // Each entry renders as its own card with its own name and masked key.
        let statuses = fetch_all(&dir);
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].id, "2");
        assert_eq!(statuses[0].title, "备用");
        assert_eq!(statuses[0].provider, "glm");
        assert_eq!(statuses[0].platform, "bigmodel");
        assert_eq!(statuses[0].key_hint.as_deref(), Some("…3210"));
    }

    #[test]
    fn entry_ids_are_never_reused() {
        let dir = temp_dir("ids");
        add_plan(&dir, "glm", "甲", "zai", "12345678abcdef").unwrap();
        add_plan(&dir, "glm", "乙", "zai", "12345678abcdef").unwrap();
        remove_plan(&dir, "2").unwrap();
        add_plan(&dir, "glm", "丙", "zai", "12345678abcdef").unwrap();
        let stored = read_stored(&dir);
        let ids: Vec<&str> = stored.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["1", "3"]);
        // A stale view of the deleted entry cannot touch the new one.
        assert!(update_plan(&dir, "2", "幽灵", "zai", None).is_err());
        remove_plan(&dir, "2").unwrap(); // removing a stale id is a no-op
        assert_eq!(read_stored(&dir).entries.len(), 2);

        remove_plan(&dir, "1").unwrap();
        add_plan(&dir, "glm", "丁", "zai", "12345678abcdef").unwrap();
        let stored = read_stored(&dir);
        let ids: Vec<&str> = stored.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["3", "4"]);
    }

    #[test]
    fn delete_before_add_never_reuses_id() {
        // Legacy migration yields entry "1" with no counter; deleting it must
        // not let the next add claim "1" again.
        let dir = temp_dir("legacy-reuse");
        let legacy = json!({
            "glm": { "platform": "zai", "key": "12345678abcdef" },
            "plans": ["glm"]
        });
        fs::write(dir.join(SUBSCRIPTIONS_FILE), serde_json::to_vec(&legacy).unwrap()).unwrap();
        remove_plan(&dir, "1").unwrap();
        add_plan(&dir, "glm", "新的", "zai", "12345678abcdef").unwrap();
        let stored = read_stored(&dir);
        assert_eq!(stored.entries[0].id, "2");

        // Same for new-shape files written without the counter.
        let dir = temp_dir("counterless");
        let file = json!({ "entries": [
            { "id": "5", "kind": "glm", "name": "手写", "platform": "zai", "key": "12345678abcdef" }
        ] });
        fs::write(dir.join(SUBSCRIPTIONS_FILE), serde_json::to_vec(&file).unwrap()).unwrap();
        remove_plan(&dir, "5").unwrap();
        add_plan(&dir, "glm", "新的", "zai", "12345678abcdef").unwrap();
        let stored = read_stored(&dir);
        assert_eq!(stored.entries[0].id, "6");
    }

    #[test]
    fn legacy_file_migrates_to_one_entry() {
        let dir = temp_dir("legacy");
        let legacy = json!({
            "glm": { "platform": "zai", "key": "12345678abcdef" },
            "plans": ["glm"]
        });
        fs::write(dir.join(SUBSCRIPTIONS_FILE), serde_json::to_vec(&legacy).unwrap()).unwrap();
        let statuses = fetch_all(&dir);
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].id, "1");
        assert_eq!(statuses[0].title, "GLM Coding Plan");
        assert_eq!(statuses[0].platform, "zai");

        // The next save writes the new shape only.
        add_plan(&dir, "glm", "二号", "zai", "fedcba9876543210").unwrap();
        let raw: Value =
            serde_json::from_slice(&fs::read(dir.join(SUBSCRIPTIONS_FILE)).unwrap()).unwrap();
        assert!(raw.get("glm").is_none());
        assert_eq!(raw["entries"].as_array().unwrap().len(), 2);

        // A legacy key whose plan was never added stays hidden, as before.
        let dir = temp_dir("legacy-hidden");
        let legacy = json!({ "glm": { "platform": "zai", "key": "12345678abcdef" } });
        fs::write(dir.join(SUBSCRIPTIONS_FILE), serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert!(fetch_all(&dir).is_empty());
    }
}
