//! Subscription quota providers.
//!
//! Layout: [`store`] persists entries (one instance per subscription; the same
//! vendor may appear several times with different keys and names), each vendor
//! module turns a stored entry into a [`ProviderReport`], and this module owns
//! the shared types plus the vendor catalog. Credentials never cross the Tauri
//! boundary; only labels, counts and percentages do.
//!
//! Adding a key-based vendor: append a [`SubscriptionKind`] to [`KINDS`], add a
//! provider module exposing `fetch_entry(&StoredSubscription)`, and give it a
//! match arm in [`fetch_entry`].

use crate::omp::Result;
use serde::Serialize;
use std::path::Path;

mod glm;
mod store;

pub use store::{add_plan, remove_plan, update_plan};

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
fn fetch_entry(entry: &store::StoredSubscription) -> Result<ProviderReport> {
    match entry.kind.as_str() {
        "glm" => glm::fetch_entry(entry),
        other => Err(format!("未知的订阅套餐: {other}")),
    }
}

fn subscription_status(entry: &store::StoredSubscription) -> SubscriptionStatus {
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
    let stored = store::read_stored(data_dir);
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
