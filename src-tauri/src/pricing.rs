//! Hand-editable model pricing catalog.
//!
//! `model-pricing.json` lives at the repository root and is the single source
//! of truth: AMC bundles it as the offline fallback and re-downloads it from
//! the remote repository root whenever usage is refreshed. Updating a price
//! means editing that file and pushing; the app picks it up on next refresh.

use crate::platform::Result;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub const FILE_NAME: &str = "model-pricing.json";
pub const REMOTE_URL: &str =
    "https://raw.githubusercontent.com/BenZinaDaze/amc/main/model-pricing.json";
const MAX_REMOTE_BYTES: u64 = 8 * 1024 * 1024;
const PER_MILLION: f64 = 1_000_000.;
const BUNDLED_PRICES: &str = include_str!("../../model-pricing.json");

/// Seeds the application-data cache exactly once so first launch prices usage
/// offline. Later launches never restore the bundle over a refreshed cache.
pub fn ensure_cache(data_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(data_dir).map_err(|e| format!("创建 AMC 数据目录失败: {e}"))?;
    let path = data_dir.join(FILE_NAME);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            if let Err(error) = file.write_all(BUNDLED_PRICES.as_bytes()) {
                let _ = fs::remove_file(&path);
                return Err(format!("初始化计价缓存 {} 失败: {error}", path.display()));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("创建计价缓存 {} 失败: {error}", path.display())),
    }
    Ok(path)
}

/// Downloads the catalog from the remote repository root, validates it and
/// atomically swaps the cache. Any failure keeps the previous cache intact.
pub fn refresh(data_dir: &Path) -> Result<String> {
    let text = fetch_remote()?;
    store(data_dir, &text)
}

fn fetch_remote() -> Result<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let response = agent
        .get(REMOTE_URL)
        .header("Accept", "application/json")
        .call()
        .map_err(|e| format!("下载远程定价配置失败: {e}"))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("下载远程定价配置失败: 远程返回 HTTP {status}"));
    }
    let mut text = String::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_REMOTE_BYTES)
        .read_to_string(&mut text)
        .map_err(|e| format!("读取远程定价配置失败: {e}"))?;
    Ok(text)
}

fn store(data_dir: &Path, text: &str) -> Result<String> {
    let count = Pricing::parse(text)?.models.len();
    fs::create_dir_all(data_dir).map_err(|e| format!("创建 AMC 数据目录失败: {e}"))?;
    let path = data_dir.join(FILE_NAME);
    // A unique staging per refresh keeps concurrent swaps isolated; each
    // rename is atomic, so the cache always holds one complete catalog.
    let staging = data_dir.join(format!("{FILE_NAME}.{}.staging", uuid::Uuid::new_v4()));
    fs::write(&staging, text).map_err(|e| format!("写入计价缓存失败: {e}"))?;
    if let Err(error) = fs::rename(&staging, &path) {
        let _ = fs::remove_file(&staging);
        return Err(format!("替换计价缓存 {} 失败: {error}", path.display()));
    }
    Ok(format!("已更新模型定价配置（{count} 个模型）"))
}

#[derive(Deserialize)]
struct PricingFile {
    models: HashMap<String, ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    providers: Vec<String>,
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
    #[serde(default)]
    tiers: Vec<ModelTier>,
}

#[derive(Deserialize)]
struct ModelTier {
    above_tokens: i64,
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

/// A USD-per-token rate: a base plus overrides that apply once the combined
/// prompt tokens exceed a threshold; the highest exceeded threshold wins.
struct Rate {
    base: f64,
    overrides: Vec<(i64, f64)>,
}

impl Rate {
    fn value_for(&self, prompt: i128) -> f64 {
        self.overrides
            .iter()
            .find(|&&(threshold, _)| prompt > i128::from(threshold))
            .map_or(self.base, |&(_, value)| value)
    }
}

struct PricedModel {
    providers: Vec<String>,
    input: Rate,
    output: Rate,
    cache_read: Rate,
    cache_write: Rate,
}

pub struct Pricing {
    models: HashMap<String, PricedModel>,
}

impl Pricing {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .map_err(|e| format!("读取计价文件 {} 失败: {e}", path.display()))?;
        Self::parse(&text)
    }

    fn parse(text: &str) -> Result<Self> {
        let file: PricingFile =
            serde_json::from_str(text).map_err(|e| format!("解析计价配置失败: {e}"))?;
        let mut models = HashMap::with_capacity(file.models.len());
        for (name, entry) in file.models {
            models.insert(name.clone(), priced_model(&name, entry)?);
        }
        if models.is_empty() {
            return Err("计价配置没有任何模型条目".into());
        }
        Ok(Self { models })
    }

    /// First provider listed for a model. Sources that record no provider of
    /// their own (e.g. Claude Code transcripts only name the model, which may
    /// be routed to non-Anthropic backends) use it to stay priceable.
    pub fn primary_provider(&self, model: &str) -> Option<&str> {
        self.models.get(model).map(|entry| entry.providers[0].as_str())
    }

    pub fn cost(
        &self,
        provider: &str,
        model: &str,
        input: i64,
        read: i64,
        write: i64,
        output: i64,
    ) -> Option<f64> {
        if [input, read, write, output].iter().any(|&n| n < 0) {
            return None;
        }
        let entry = self.models.get(model)?;
        if !entry.providers.iter().any(|listed| listed == provider) {
            return None;
        }
        // OMP stores mutually exclusive uncached input, cache reads and cache
        // writes; context thresholds apply to their combined prompt.
        let prompt = i128::from(input) + i128::from(read) + i128::from(write);
        let cost = input as f64 * entry.input.value_for(prompt)
            + read as f64 * entry.cache_read.value_for(prompt)
            + write as f64 * entry.cache_write.value_for(prompt)
            + output as f64 * entry.output.value_for(prompt);
        cost.is_finite().then_some(cost)
    }
}

fn priced_model(name: &str, entry: ModelEntry) -> Result<PricedModel> {
    if entry.providers.is_empty() || entry.providers.iter().any(String::is_empty) {
        return Err(format!("计价配置条目 {name} 的 providers 不能为空"));
    }
    let mut input_overrides = BTreeMap::new();
    let mut output_overrides = BTreeMap::new();
    let mut read_overrides = BTreeMap::new();
    let mut write_overrides = BTreeMap::new();
    for tier in &entry.tiers {
        if tier.above_tokens <= 0 {
            return Err(format!("计价配置条目 {name} 的 above_tokens 必须为正"));
        }
        insert_override(&mut input_overrides, name, tier, tier.input)?;
        insert_override(&mut output_overrides, name, tier, tier.output)?;
        insert_override(&mut read_overrides, name, tier, tier.cache_read)?;
        insert_override(&mut write_overrides, name, tier, tier.cache_write)?;
    }
    Ok(PricedModel {
        providers: entry.providers,
        input: Rate {
            base: base_rate(name, "input", entry.input)?,
            overrides: descending(input_overrides),
        },
        output: Rate {
            base: base_rate(name, "output", entry.output)?,
            overrides: descending(output_overrides),
        },
        cache_read: Rate {
            base: optional_rate(name, "cache_read", entry.cache_read)?,
            overrides: descending(read_overrides),
        },
        cache_write: Rate {
            base: optional_rate(name, "cache_write", entry.cache_write)?,
            overrides: descending(write_overrides),
        },
    })
}

fn insert_override(
    overrides: &mut BTreeMap<i64, f64>,
    name: &str,
    tier: &ModelTier,
    value: Option<f64>,
) -> Result<()> {
    if let Some(value) = valid_rate(name, "tiers", value)? {
        overrides.insert(tier.above_tokens, value / PER_MILLION);
    }
    Ok(())
}

fn base_rate(name: &str, field: &str, value: Option<f64>) -> Result<f64> {
    valid_rate(name, field, value)?
        .map(|value| value / PER_MILLION)
        .ok_or_else(|| format!("计价配置条目 {name} 缺少 {field} 单价"))
}

fn optional_rate(name: &str, field: &str, value: Option<f64>) -> Result<f64> {
    Ok(valid_rate(name, field, value)?.map_or(0., |value| value / PER_MILLION))
}

fn valid_rate(name: &str, field: &str, value: Option<f64>) -> Result<Option<f64>> {
    match value {
        None => Ok(None),
        Some(value) if value.is_finite() && value >= 0. => Ok(Some(value)),
        Some(value) => Err(format!("计价配置条目 {name} 的 {field} 单价无效: {value}")),
    }
}

fn descending(overrides: BTreeMap<i64, f64>) -> Vec<(i64, f64)> {
    let mut overrides: Vec<(i64, f64)> = overrides.into_iter().collect();
    overrides.reverse();
    overrides
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn catalog() -> Pricing {
        Pricing::parse(BUNDLED_PRICES).unwrap()
    }

    #[test]
    fn bundled_catalog_prices_providers_cache_and_threshold_tiers() {
        let prices = catalog();
        let short = prices
            .cost("openai", "gpt-6-sol", 100_000, 100_000, 50_000, 100_000)
            .unwrap();
        assert!((short - 1.345).abs() < 1e-10);
        let long = prices
            .cost(
                "openai",
                "gpt-6-sol",
                1_000_000,
                1_000_000,
                1_000_000,
                1_000_000,
            )
            .unwrap();
        assert!((long - 24.4).abs() < 1e-10);
        // openai-free shares OpenAI list prices; the threshold is exclusive.
        let threshold = prices
            .cost("openai-free", "gpt-6-luna", 271_999, 1, 0, 1_000_000)
            .unwrap();
        assert!((threshold - (271_999. * 0.1 + 0.01 + 500_000.) / 1_000_000.).abs() < 1e-9);
        let above = prices
            .cost("openai-free", "gpt-6-luna", 272_000, 1, 0, 1_000_000)
            .unwrap();
        assert!((above - (272_000. * 0.2 + 0.02 + 750_000.) / 1_000_000.).abs() < 1e-9);
        // Overrides apply per rate once the combined prompt passes 200k.
        let anthropic = prices
            .cost("anthropic", "claude-sonnet-4-5", 200_000, 1, 1, 100)
            .unwrap();
        assert!((anthropic - (200_000. * 6e-6 + 6e-7 + 7.5e-6 + 100. * 2.25e-5)).abs() < 1e-9);
        for provider in [
            "google",
            "google-gemini",
            "gemini",
            "vertex_ai-language-models",
        ] {
            assert!(
                (prices
                    .cost(provider, "gemini-2.5-pro", 1_000, 0, 0, 1_000)
                    .unwrap()
                    - 0.01125)
                    .abs()
                    < 1e-9
            );
        }
        assert!(
            (prices
                .cost("zhipu-coding-plan", "glm-5.3", 1_000_000, 0, 0, 1_000_000)
                .unwrap()
                - 5.8)
                .abs()
                < 1e-9
        );
        // gpt-6.1-sol switches to its long-context tier past 272k prompt tokens.
        let sol61 = prices
            .cost("openai", "gpt-6.1-sol", 272_000, 1, 1, 1_000_000)
            .unwrap();
        assert!((sol61 - (272_000. * 4e-6 + 0.2e-6 + 5e-6 + 15.)).abs() < 1e-9);
        assert!(
            (prices
                .cost("openai", "gpt-6.1-sol", 1_000, 0, 0, 1_000)
                .unwrap()
                - 0.012)
                .abs()
                < 1e-12
        );
        // Jev prices input at $42 per billion and output is free, not unpriced.
        assert!(
            (prices
                .cost("typesafe", "jev-latest", 1_000_000_000, 0, 0, 0)
                .unwrap()
                - 42.)
                .abs()
                < 1e-9
        );
        assert_eq!(
            prices.cost("typesafe", "jev-latest", 0, 0, 0, 1_000_000),
            Some(0.)
        );
        assert_eq!(prices.cost("typesafe", "gpt-6-sol", 100, 0, 0, 0), None);
        assert_eq!(prices.cost("openai", "unknown-model", 100, 0, 0, 0), None);
        assert_eq!(prices.cost("openai", "gpt-6-sol", -1, 0, 0, 0), None);
    }

    #[test]
    fn tier_overrides_apply_per_rate_with_base_fallback() {
        let prices = Pricing::parse(
            r#"{"models":{"m":{"providers":["p"],"input":1,"output":2,
               "tiers":[{"above_tokens":100,"input":3}]}}}"#,
        )
        .unwrap();
        // At the threshold the base still applies; past it only listed rates
        // change and unlisted buckets fall back to their base.
        let at = prices.cost("p", "m", 50, 50, 0, 10).unwrap();
        assert!((at - (50. * 1e-6 + 10. * 2e-6)).abs() < 1e-12);
        let past = prices.cost("p", "m", 101, 0, 0, 0).unwrap();
        assert!((past - 101. * 3e-6).abs() < 1e-12);
    }

    #[test]
    fn highest_exceeded_tier_wins_and_later_duplicates_replace() {
        let prices = Pricing::parse(
            r#"{"models":{"m":{"providers":["p"],"input":1,"output":1,
               "tiers":[{"above_tokens":100,"input":3},{"above_tokens":200,"input":5},
                        {"above_tokens":200,"input":7}]}}}"#,
        )
        .unwrap();
        assert_eq!(
            prices.cost("p", "m", 150, 0, 0, 0).unwrap(),
            150. * 3e-6
        );
        assert_eq!(
            prices.cost("p", "m", 201, 0, 0, 0).unwrap(),
            201. * 7e-6
        );
    }

    #[test]
    fn malformed_entries_are_rejected() {
        for text in [
            "{}",
            r#"{"models":{}}"#,
            r#"{"models":{"m":{"providers":[],"input":1,"output":1}}}"#,
            r#"{"models":{"m":{"providers":[""],"input":1,"output":1}}}"#,
            r#"{"models":{"m":{"providers":["p"],"output":1}}}"#,
            r#"{"models":{"m":{"providers":["p"],"input":-1,"output":1}}}"#,
            r#"{"models":{"m":{"providers":["p"],"input":1,"output":1,"tiers":[{"above_tokens":0,"input":2}]}}}"#,
            r#"{"models":{"m":{"providers":["p"],"input":1,"output":1,"tiers":[{"above_tokens":100,"input":-2}]}}}"#,
        ] {
            assert!(Pricing::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn remote_store_validates_swaps_and_preserves_previous_cache() {
        let dir = env::temp_dir().join(format!("amc-pricing-store-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        assert!(store(&dir, "not json").is_err());
        assert!(!dir.join(FILE_NAME).exists());
        let text = r#"{"version":1,"models":{"m":{"providers":["p"],"input":1,"output":2}}}"#;
        assert_eq!(store(&dir, text).unwrap(), "已更新模型定价配置（1 个模型）");
        let loaded = Pricing::load(&dir.join(FILE_NAME)).unwrap();
        assert_eq!(loaded.cost("p", "m", 1, 0, 0, 1).unwrap(), 3e-6);
        // A failed refresh keeps the previous cache and leaves no staging file.
        assert!(store(&dir, r#"{"models":{}}"#).is_err());
        let kept = Pricing::load(&dir.join(FILE_NAME)).unwrap();
        assert_eq!(kept.cost("p", "m", 1, 0, 0, 1).unwrap(), 3e-6);
        assert_eq!(staging_files(&dir), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    fn staging_files(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".staging"))
            .count()
    }

    #[test]
    fn concurrent_store_swaps_are_isolated_and_leave_one_valid_cache() {
        let dir = env::temp_dir().join(format!("amc-pricing-race-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        std::thread::scope(|scope| {
            for index in 0..4 {
                let dir = &dir;
                scope.spawn(move || {
                    let text = format!(
                        r#"{{"models":{{"m{index}":{{"providers":["p"],"input":1,"output":2}}}}}}"#
                    );
                    store(dir, &text).unwrap();
                });
            }
        });
        // Every rename consumed its own staging file and the cache holds
        // exactly one complete catalog, never an interleaved mix.
        let loaded = Pricing::load(&dir.join(FILE_NAME)).unwrap();
        let kept = (0..4).filter(|index| loaded.models.contains_key(&format!("m{index}")));
        assert_eq!(kept.count(), 1);
        assert_eq!(staging_files(&dir), 0);
        fs::remove_dir_all(&dir).unwrap();
    }
}
