use crate::platform::Result;
use serde::Serialize;
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

/// GitHub Releases 的 latest 端点总是返回语义上最新的正式 release。
const RELEASES_API: &str = "https://api.github.com/repos/BenZinaDaze/amc/releases/latest";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BODY: u64 = 64 * 1024;

#[derive(Serialize)]
pub struct AppUpdate {
    /// 最新版本号，已去掉 `v` 前缀。
    pub latest: String,
    /// release 页面链接（html_url）。
    pub url: String,
}

/// 查询 GitHub 最新 release。失败一律返回 Err，由前端静默忽略——
/// 版本提示是纯增益信息，不能打扰主流程。
pub fn check() -> Result<AppUpdate> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .into();
    // GitHub API 强制要求 User-Agent，缺省会 403。
    let response = agent
        .get(RELEASES_API)
        .header("User-Agent", "amc-desktop")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("查询版本更新失败: {e}"))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("查询版本更新失败: 远程返回 HTTP {status}"));
    }
    let mut text = String::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_BODY)
        .read_to_string(&mut text)
        .map_err(|e| format!("读取版本更新响应失败: {e}"))?;
    parse_release(&text).ok_or_else(|| "版本更新响应缺少 tag_name 或 html_url".to_owned())
}

/// 纯解析：只取 `tag_name`（去 `v` 前缀）与 `html_url`，字段缺失或
/// 类型不符返回 None，绝不臆造。
fn parse_release(body: &str) -> Option<AppUpdate> {
    let value: Value = serde_json::from_str(body).ok()?;
    let tag = value.get("tag_name")?.as_str()?;
    let url = value.get("html_url")?.as_str()?;
    let latest = tag.strip_prefix('v').unwrap_or(tag).trim();
    if latest.is_empty() || url.is_empty() {
        return None;
    }
    Some(AppUpdate {
        latest: latest.to_owned(),
        url: url.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tag_and_url_stripping_v() {
        let update = parse_release(
            r#"{"tag_name":"v0.5.0","html_url":"https://github.com/BenZinaDaze/amc/releases/tag/v0.5.0"}"#,
        )
        .expect("valid body parses");
        assert_eq!(update.latest, "0.5.0");
        assert_eq!(update.url, "https://github.com/BenZinaDaze/amc/releases/tag/v0.5.0");
    }

    #[test]
    fn rejects_bodies_missing_fields() {
        assert!(parse_release("{}").is_none());
        assert!(parse_release(r#"{"tag_name":123}"#).is_none());
        assert!(parse_release(r#"{"tag_name":"","html_url":"https://x"}"#).is_none());
        assert!(parse_release("not json").is_none());
    }
}
