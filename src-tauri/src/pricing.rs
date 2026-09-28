use crate::omp::Result;
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub const FILE_NAME: &str = "model_prices_and_context_window.json";
const BUNDLED_PRICES: &str = include_str!("../resources/model_prices_and_context_window.json");

// The bundled JSON seeds the editable application-data copy once. Never overwrite
// user changes on a subsequent launch or silently fall back to stale bundled rates.
pub fn ensure_file(data_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(data_dir).map_err(|e| format!("创建 AMC 数据目录失败: {e}"))?;
    let path = data_dir.join(FILE_NAME);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            if let Err(error) = file.write_all(BUNDLED_PRICES.as_bytes()) {
                let _ = fs::remove_file(&path);
                return Err(format!("初始化计价文件 {} 失败: {error}", path.display()));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("创建计价文件 {} 失败: {error}", path.display())),
    }
    Ok(path)
}

#[derive(Deserialize)]
struct ModelPrice {
    litellm_provider: Option<String>,
    mode: Option<String>,
    input_cost_per_token: Option<f64>,
    output_cost_per_token: Option<f64>,
    cache_read_input_token_cost: Option<f64>,
    cache_creation_input_token_cost: Option<f64>,
    input_cost_per_token_above_200k_tokens: Option<f64>,
    output_cost_per_token_above_200k_tokens: Option<f64>,
    cache_read_input_token_cost_above_200k_tokens: Option<f64>,
    cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
    input_cost_per_token_above_272k_tokens: Option<f64>,
    output_cost_per_token_above_272k_tokens: Option<f64>,
    cache_read_input_token_cost_above_272k_tokens: Option<f64>,
    cache_creation_input_token_cost_above_272k_tokens: Option<f64>,
    long_context_input_token_threshold: Option<i64>,
    long_context_input_cost_multiplier: Option<f64>,
    long_context_output_cost_multiplier: Option<f64>,
}

pub struct Pricing {
    models: HashMap<String, ModelPrice>,
}

impl Pricing {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .map_err(|e| format!("读取计价文件 {} 失败: {e}", path.display()))?;
        let models = serde_json::from_str(&text)
            .map_err(|e| format!("解析计价文件 {} 失败: {e}", path.display()))?;
        Ok(Self { models })
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
        let price = self.models.get(model)?;
        // OMP only exposes text token totals. Image/audio/realtime/embedding modes
        // need usage units absent from stats.db; their text rates are not a bill.
        if !matches!(
            price.mode.as_deref(),
            Some("chat" | "responses" | "completion")
        ) {
            return None;
        }
        let listed_provider = price.litellm_provider.as_deref()?;
        let matching_provider = match provider {
            "openai-free" => listed_provider == "openai",
            "google" | "google-gemini" | "gemini" => {
                matches!(listed_provider, "gemini" | "vertex_ai-language-models")
            }
            _ => provider == listed_provider,
        };
        if !matching_provider {
            return None;
        }
        // OMP stores mutually exclusive uncached input, cache reads and cache writes.
        // Context thresholds apply to their combined prompt, not each bucket alone.
        let prompt = i128::from(input) + i128::from(read) + i128::from(write);
        let above_200k = prompt > 200_000;
        let above_272k = prompt > 272_000;
        let long = price
            .long_context_input_token_threshold
            .is_some_and(|threshold| prompt > i128::from(threshold));
        let input_rate = rate(
            price.input_cost_per_token,
            price.input_cost_per_token_above_200k_tokens,
            price.input_cost_per_token_above_272k_tokens,
            price.long_context_input_cost_multiplier,
            above_200k,
            above_272k,
            long,
        )?;
        let output_rate = rate(
            price.output_cost_per_token,
            price.output_cost_per_token_above_200k_tokens,
            price.output_cost_per_token_above_272k_tokens,
            price.long_context_output_cost_multiplier,
            above_200k,
            above_272k,
            long,
        )?;
        let read_rate = if read > 0 {
            Some(rate(
                price.cache_read_input_token_cost,
                price.cache_read_input_token_cost_above_200k_tokens,
                price.cache_read_input_token_cost_above_272k_tokens,
                price.long_context_input_cost_multiplier,
                above_200k,
                above_272k,
                long,
            )?)
        } else {
            None
        };
        let write_rate = if write > 0 {
            Some(rate(
                price.cache_creation_input_token_cost,
                price.cache_creation_input_token_cost_above_200k_tokens,
                price.cache_creation_input_token_cost_above_272k_tokens,
                price.long_context_input_cost_multiplier,
                above_200k,
                above_272k,
                long,
            )?)
        } else {
            None
        };
        let cost = input as f64 * input_rate
            + read as f64 * read_rate.unwrap_or(0.)
            + write as f64 * write_rate.unwrap_or(0.)
            + output as f64 * output_rate;
        cost.is_finite().then_some(cost)
    }
}

fn rate(
    base: Option<f64>,
    above_200k: Option<f64>,
    above_272k: Option<f64>,
    long_multiplier: Option<f64>,
    over_200k: bool,
    over_272k: bool,
    long: bool,
) -> Option<f64> {
    let base = base?;
    let value = if over_272k && above_272k.is_some() {
        above_272k?
    } else if long && long_multiplier.is_some() {
        base * long_multiplier?
    } else if over_200k && above_200k.is_some() {
        above_200k?
    } else {
        base
    };
    (value.is_finite() && value >= 0.).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled() -> Pricing {
        serde_json::from_str::<HashMap<String, ModelPrice>>(BUNDLED_PRICES)
            .map(|models| Pricing { models })
            .unwrap()
    }

    #[test]
    fn prices_each_provider_and_cache_category_from_upstream_json() {
        let prices = bundled();
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
        let threshold = prices
            .cost("openai-free", "gpt-6-luna", 271_999, 1, 0, 1_000_000)
            .unwrap();
        assert!((threshold - (271_999. * 0.1 + 0.01 + 500_000.) / 1_000_000.).abs() < 1e-9);
        let above = prices
            .cost("openai", "gpt-6-luna", 272_000, 1, 0, 1_000_000)
            .unwrap();
        assert!((above - (272_000. * 0.2 + 0.02 + 750_000.) / 1_000_000.).abs() < 1e-9);

        let anthropic = prices
            .cost("anthropic", "claude-sonnet-4-5", 200_000, 1, 1, 100)
            .unwrap();
        assert!((anthropic - (200_000. * 6e-6 + 6e-7 + 7.5e-6 + 100. * 2.25e-5)).abs() < 1e-9);
        assert!(
            (prices
                .cost("google", "gemini-2.5-pro", 1_000, 0, 0, 1_000)
                .unwrap()
                - 0.01125)
                .abs()
                < 1e-9
        );
        assert!(
            (prices
                .cost("gemini", "gemini-2.5-pro", 1_000, 0, 0, 1_000)
                .unwrap()
                - 0.01125)
                .abs()
                < 1e-9
        );
        assert!(
            (prices.cost("xai", "grok-4.7", 200_001, 0, 0, 1).unwrap() - (200_001. * 4e-6 + 12e-6))
                .abs()
                < 1e-9
        );
        assert_eq!(prices.cost("openai", "gpt-5.4", 100, 0, 1, 0), None);
        assert_eq!(prices.cost("typesafe", "gpt-6-sol", 100, 0, 0, 0), None);
        assert_eq!(prices.cost("openai", "unknown-model", 100, 0, 0, 0), None);
        assert_eq!(prices.cost("openai", "gpt-6-sol", -1, 0, 0, 0), None);
    }

    #[test]
    fn non_text_models_are_unpriced_even_when_text_rates_are_zero() {
        let prices = bundled();
        assert_eq!(
            prices.cost(
                "gemini",
                "gemini-2.0-flash-exp-image-generation",
                100,
                0,
                0,
                10
            ),
            None
        );
        assert_eq!(
            prices.cost("google", "gemini-3-pro-image-preview", 100, 0, 0, 10),
            None
        );
        assert_eq!(
            prices.cost("openai", "gpt-5.4", 100, 0, 0, 10),
            Some(100. * 2.5e-6 + 10. * 15e-6)
        );
        assert_eq!(
            prices.cost("openai", "gpt-5-codex", 100, 0, 0, 10),
            Some(100. * 1.25e-6 + 10. * 10e-6)
        );
        assert_eq!(
            prices.cost(
                "text-completion-openai",
                "gpt-3.5-turbo-instruct",
                100,
                0,
                0,
                10
            ),
            Some(100. * 1.5e-6 + 10. * 2e-6)
        );
    }

    #[test]
    fn data_file_keeps_user_edits_and_reports_invalid_json() {
        let dir = std::env::temp_dir().join(format!("amc-price-file-{}", uuid::Uuid::new_v4()));
        let file = ensure_file(&dir).unwrap();
        assert_eq!(
            Pricing::load(&file)
                .unwrap()
                .cost("openai", "gpt-6-sol", 1, 0, 0, 0),
            Some(2e-6)
        );
        fs::write(&file, r#"{"new-model":{"litellm_provider":"anthropic","mode":"chat","input_cost_per_token":0.001,"output_cost_per_token":0.002}}"#).unwrap();
        ensure_file(&dir).unwrap();
        let updated = Pricing::load(&file).unwrap();
        assert_eq!(
            updated.cost("anthropic", "new-model", 100, 0, 0, 50),
            Some(0.2)
        );
        assert_eq!(updated.cost("openai", "gpt-6-sol", 1, 0, 0, 0), None);
        fs::write(&file, "{").unwrap();
        assert!(Pricing::load(&file).err().unwrap().contains("解析计价文件"));
        fs::remove_dir_all(dir).unwrap();
    }
}
