use crate::platform::Result;
use serde::Serialize;
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

/// GitHub Releases 的 latest 端点总是返回语义上最新的正式 release。
const RELEASES_API: &str = "https://api.github.com/repos";
/// npm registry 的 latest 端点返回 latest dist-tag 指向的清单。
const NPM_REGISTRY_API: &str = "https://registry.npmjs.org";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BODY: u64 = 64 * 1024;

#[derive(Serialize)]
pub struct AppUpdate {
    /// 最新版本号，已去掉 `v` 前缀。
    pub latest: String,
    /// release 页面或项目主页链接（html_url / homepage）。
    pub url: String,
}

/// 查询 AMC 自己的最新 release。失败一律返回 Err，由前端静默忽略——
/// 版本提示是纯增益信息，不能打扰主流程。
pub fn check() -> Result<AppUpdate> {
    github_release("BenZinaDaze/amc")
}

/// 已接入 Agent 的更新来源（id 与 usage 模块一致）：OMP 走 GitHub
/// Releases，Claude Code 与 Codex CLI 的正式发布渠道是 npm。
pub fn check_agent(agent: &str) -> Result<AppUpdate> {
    match agent {
        "omp" => github_release("can1357/oh-my-pi"),
        "claude-code" => npm_latest("@anthropic-ai/claude-code"),
        "codex" => npm_latest("@openai/codex"),
        other => Err(format!("不支持的 Agent 更新来源: {other}")),
    }
}

fn http_get(url: &str, accept: &str) -> Result<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .into();
    // GitHub API 强制要求 User-Agent，缺省会 403。
    let response = agent
        .get(url)
        .header("User-Agent", "amc-desktop")
        .header("Accept", accept)
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
    Ok(text)
}

fn github_release(repo: &str) -> Result<AppUpdate> {
    let url = format!("{RELEASES_API}/{repo}/releases/latest");
    let body = http_get(&url, "application/vnd.github+json")?;
    parse_release(&body).ok_or_else(|| "版本更新响应缺少 tag_name 或 html_url".to_owned())
}

fn npm_latest(package: &str) -> Result<AppUpdate> {
    let url = format!("{NPM_REGISTRY_API}/{package}/latest");
    let body = http_get(&url, "application/json")?;
    parse_npm(&body, package).ok_or_else(|| "npm 版本响应缺少 version 字段".to_owned())
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

/// npm latest 清单解析：版本取 `version`，链接优先 `homepage`（须是
/// https），缺省回退 npm 包页面。
fn parse_npm(body: &str, package: &str) -> Option<AppUpdate> {
    let value: Value = serde_json::from_str(body).ok()?;
    let latest = value.get("version")?.as_str()?.trim();
    if latest.is_empty() {
        return None;
    }
    let url = match value.get("homepage").and_then(Value::as_str) {
        Some(homepage) if homepage.starts_with("https://") => homepage.to_owned(),
        _ => format!("https://www.npmjs.com/package/{package}"),
    };
    Some(AppUpdate {
        latest: latest.to_owned(),
        url,
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

    #[test]
    fn parses_npm_version_preferring_https_homepage() {
        let update = parse_npm(
            r#"{"version":"2.1.296","homepage":"https://github.com/anthropics/claude-code"}"#,
            "@anthropic-ai/claude-code",
        )
        .expect("valid body parses");
        assert_eq!(update.latest, "2.1.296");
        assert_eq!(update.url, "https://github.com/anthropics/claude-code");
    }

    #[test]
    fn npm_without_homepage_falls_back_to_package_page() {
        let update = parse_npm(r#"{"version":"0.162.1"}"#, "@openai/codex").expect("valid body parses");
        assert_eq!(update.latest, "0.162.1");
        assert_eq!(update.url, "https://www.npmjs.com/package/@openai/codex");
        // 非 https 的 homepage 不可信，同样回退。
        assert_eq!(
            parse_npm(r#"{"version":"1.0","homepage":"http://x"}"#, "pkg").unwrap().url,
            "https://www.npmjs.com/package/pkg"
        );
    }

    #[test]
    fn rejects_npm_bodies_missing_version() {
        assert!(parse_npm("Not Found", "pkg").is_none());
        assert!(parse_npm(r#"{"version":""}"#, "pkg").is_none());
        assert!(parse_npm(r#"{"version":42}"#, "pkg").is_none());
    }

    #[test]
    fn unknown_agent_sources_are_rejected() {
        assert!(check_agent("wget").is_err());
    }
}
