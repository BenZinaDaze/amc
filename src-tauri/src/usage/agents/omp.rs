// OMP usage: the stats.db reader plus the launch-context resolution that
// locates it (dotenv files, profiles, XDG data migration). OMP owns the
// per-request `messages` table with a `UNIQUE(session_file, entry_id)` key,
// which becomes the archive's dedup identity.
#[cfg(test)]
use crate::usage::prices;
use crate::usage::{AgentUsageAdapter, SourceScan, UsageRecord};
#[cfg(test)]
use crate::pricing::Pricing;
use crate::platform;
use rusqlite::{params, Connection, OpenFlags};
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

pub(crate) struct OmpUsageAdapter {
    stats_db: PathBuf,
    /// OMP 原生用户级 MCP 配置（agent 目录下的 mcp.json），供 MCP
    /// 管理模块定位写入目标；与 stats.db 走同一套目录解析。
    mcp_json: PathBuf,
    /// 生效的配置根（含 profile），用于判断 OMP 是否已初始化。
    config_root: PathBuf,
}

impl OmpUsageAdapter {
    pub(crate) fn mcp_json_path(&self) -> &Path {
        &self.mcp_json
    }

    pub(crate) fn config_root(&self) -> &Path {
        &self.config_root
    }
}

/// `messages` 表一行的原始列值（session_file 之后均为字符串/整数列）。
type OmpMessageRow = (String, String, String, i64, i64, i64, i64, i64, i64);

const OMP_EXECUTABLE: &str = if cfg!(windows) { "omp.exe" } else { "omp" };
/// OMP 经 bun/npm 安装，需要这些相对用户目录的附加搜索路径。
const OMP_HOME_EXTRA_DIRS: &[&str] = &[".bun/bin", ".local/bin"];

/// 在 PATH 与 OMP 安装目录中定位 `omp` 可执行文件。
pub(crate) fn executable() -> Option<PathBuf> {
    platform::cli_executable(OMP_EXECUTABLE, OMP_HOME_EXTRA_DIRS)
}

/// OMP 安装状态（Agents 页展示用）：是否找到文件 + `--version` 输出。
pub(crate) fn status() -> (bool, String) {
    let status = platform::cli_status(OMP_EXECUTABLE, OMP_HOME_EXTRA_DIRS);
    (status.installed, status.version)
}

// OMP loads dotenv files after Bun preloads the launch project's .env. Only
// directory keys are retained; credentials in these files never enter AMC state.
fn path_env() -> HashMap<String, String> {
    const KEYS: [&str; 5] = [
        "PI_CONFIG_DIR",
        "PI_CODING_AGENT_DIR",
        "XDG_DATA_HOME",
        "OMP_PROFILE",
        "PI_PROFILE",
    ];
    KEYS.into_iter()
        .filter_map(|key| env::var(key).ok().map(|value| (key.to_owned(), value)))
        .collect()
}

fn apply_dotenv_paths(
    file: &Path,
    values: &mut HashMap<String, String>,
    mirror_omp: bool,
) -> platform::Result<()> {
    let contents = match fs::read_to_string(file) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "读取 OMP 环境文件 {} 失败: {error}",
                file.display()
            ))
        }
    };
    let mut parsed = HashMap::new();
    let mut remaining = contents.as_str();
    while !remaining.is_empty() {
        let (line, rest) = remaining.split_once('\n').unwrap_or((remaining, ""));
        remaining = rest;
        let line = line.trim_start();
        let line = line
            .strip_prefix("export")
            .and_then(|tail| {
                tail.starts_with(char::is_whitespace)
                    .then_some(tail.trim_start())
            })
            .unwrap_or(line);
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim_end();
        if !key
            .bytes()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
            || !key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            continue;
        }
        let relevant = matches!(
            key,
            "PI_CONFIG_DIR"
                | "OMP_CONFIG_DIR"
                | "PI_CODING_AGENT_DIR"
                | "OMP_CODING_AGENT_DIR"
                | "XDG_DATA_HOME"
                | "OMP_PROFILE"
                | "PI_PROFILE"
        );
        if !relevant {
            // Skip unrelated values, including multiline secrets, without
            // retaining them or mistaking lines inside them for path keys.
            let raw = raw.trim_start();
            if let Some(quote @ ('"' | '\'' | '`')) = raw.chars().next() {
                if raw[quote.len_utf8()..].find(quote).is_none() {
                    let Some(end) = remaining.find(quote) else {
                        return Err(format!("OMP 环境文件 {} 的值缺少结束引号", file.display()));
                    };
                    remaining = remaining[end + quote.len_utf8()..]
                        .split_once('\n')
                        .map_or("", |(_, rest)| rest);
                }
            }
            continue;
        }
        let raw = raw.trim_start();
        let value = if let Some(quote @ ('"' | '\'' | '`')) = raw.chars().next() {
            let quoted = &raw[quote.len_utf8()..];
            let value = if let Some(end) = quoted.find(quote) {
                quoted[..end].to_owned()
            } else if let Some(end) = remaining.find(quote) {
                let value = format!("{quoted}\n{}", &remaining[..end]);
                remaining = remaining[end + quote.len_utf8()..]
                    .split_once('\n')
                    .map_or("", |(_, rest)| rest);
                value
            } else {
                return Err(format!("OMP 环境文件 {} 的值缺少结束引号", file.display()));
            };
            if quote == '"' {
                value.replace("\\n", "\n").replace("\\r", "\r")
            } else {
                value
            }
        } else {
            raw.split('#').next().unwrap_or("").trim_end().to_owned()
        };
        if !value.contains('\0') {
            parsed.insert(key.to_owned(), value);
        }
    }
    // The pre-import Bun autoload does not perform OMP_ -> PI_ mirroring.
    if mirror_omp {
        for (source, target) in [
            ("OMP_CONFIG_DIR", "PI_CONFIG_DIR"),
            ("OMP_CODING_AGENT_DIR", "PI_CODING_AGENT_DIR"),
            ("OMP_PROFILE", "PI_PROFILE"),
        ] {
            if let Some(value) = parsed.get(source).cloned() {
                parsed.insert(target.to_owned(), value);
            }
        }
    }
    for (key, value) in parsed {
        if matches!(
            key.as_str(),
            "PI_CONFIG_DIR"
                | "PI_CODING_AGENT_DIR"
                | "XDG_DATA_HOME"
                | "OMP_PROFILE"
                | "PI_PROFILE"
        ) && values.get(&key).is_none_or(|current| current.is_empty())
        {
            values.insert(key, value);
        }
    }
    Ok(())
}

fn config_dirs(
    home: &Path,
    config_name: &Path,
    profile_name: &str,
    pi_profile: Option<&str>,
    agent_override: Option<&Path>,
    cwd: &Path,
) -> platform::Result<(PathBuf, PathBuf)> {
    // Node's path.join(home, configName) retains home for leading slashes.
    #[cfg(not(windows))]
    let config_name = config_name.strip_prefix("/").unwrap_or(config_name);
    let base = home.join(config_name);
    let profile = normalize_profile(profile_name)?;
    let root = profile
        .as_ref()
        .map_or_else(|| base.clone(), |name| base.join("profiles").join(name));
    let derived_pi_dir = pi_profile
        .and_then(|name| normalize_profile(name).ok().flatten())
        .map(|name| base.join("profiles").join(name).join("agent"));
    let agent_override = agent_override.filter(|path| derived_pi_dir.as_deref() != Some(*path));
    let agent_dir = if profile.is_some() {
        root.join("agent")
    } else {
        agent_override.map_or_else(
            || root.join("agent"),
            |path| {
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    cwd.join(path)
                }
            },
        )
    };
    Ok((root, agent_dir))
}

fn effective_path_env(
    home: &Path,
    cwd: &Path,
    mut values: HashMap<String, String>,
) -> platform::Result<(HashMap<String, String>, String)> {
    // Bun preloads the launch project's .env before importing OMP's dirs.ts.
    apply_dotenv_paths(&cwd.join(".env"), &mut values, false)?;
    let profile = values
        .get("OMP_PROFILE")
        .or_else(|| values.get("PI_PROFILE"))
        .cloned()
        .unwrap_or_default();
    let config_name = values
        .get("PI_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map_or(".omp", String::as_str);
    let (root, agent) = config_dirs(
        home,
        Path::new(config_name),
        &profile,
        values.get("PI_PROFILE").map(String::as_str),
        values
            .get("PI_CODING_AGENT_DIR")
            .filter(|v| !v.is_empty())
            .map(Path::new),
        cwd,
    )?;
    // The initial dirs resolver is already fixed when env.ts mirrors OMP_ keys.
    apply_dotenv_paths(&cwd.join(".env"), &mut values, true)?;
    for file in [agent.join(".env"), root.join(".env"), home.join(".env")] {
        apply_dotenv_paths(&file, &mut values, true)?;
    }
    Ok((values, profile))
}

impl OmpUsageAdapter {
    pub(crate) fn from_environment() -> platform::Result<Self> {
        let home = platform::home()?;
        let cwd = env::current_dir().map_err(|e| format!("读取 OMP 启动目录失败: {e}"))?;
        Self::from_launch_context(home, &cwd, path_env())
    }

    fn from_launch_context(
        home: PathBuf,
        cwd: &Path,
        launch_env: HashMap<String, String>,
    ) -> platform::Result<Self> {
        let (values, profile) = effective_path_env(&home, cwd, launch_env)?;
        Self::from_configuration(
            home,
            values
                .get("PI_CONFIG_DIR")
                .filter(|v| !v.is_empty())
                .map_or_else(|| PathBuf::from(".omp"), PathBuf::from),
            &profile,
            values.get("PI_PROFILE").map(String::as_str),
            values
                .get("PI_CODING_AGENT_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            values
                .get("XDG_DATA_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            cwd,
        )
    }

    fn from_configuration(
        home: PathBuf,
        config_name: PathBuf,
        profile_name: &str,
        pi_profile: Option<&str>,
        agent_override: Option<PathBuf>,
        xdg_data_home: Option<PathBuf>,
        cwd: &Path,
    ) -> platform::Result<Self> {
        let (root, agent_dir) = config_dirs(
            &home,
            &config_name,
            profile_name,
            pi_profile,
            agent_override.as_deref(),
            cwd,
        )?;
        let profile = normalize_profile(profile_name)?;
        let data_root = if agent_dir == root.join("agent")
            && cfg!(any(target_os = "linux", target_os = "macos"))
        {
            xdg_data_home.and_then(|value| {
                let mut path = value.join("omp");
                if let Some(name) = &profile {
                    path = path.join("profiles").join(name);
                }
                path.exists().then_some(path)
            })
        } else {
            None
        };
        Ok(Self {
            stats_db: data_root.as_ref().unwrap_or(&root).join("stats.db"),
            mcp_json: agent_dir.join("mcp.json"),
            config_root: root,
        })
    }
    fn sync_cli(&self) -> platform::Result<()> {
        let program = executable().ok_or("找不到 OMP 可执行文件；请安装 omp 后重试")?;
        let output = platform::cli_command(&program)?
            .args(["stats", "--summary"])
            .output()
            .map_err(|e| format!("执行 {} 失败: {e}", program.display()))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            let detail = detail.trim();
            return Err(format!(
                "OMP 用量同步失败（{}）{}",
                output.status,
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            ));
        }
        Ok(())
    }

    fn scan_db(&self, since: i64) -> platform::Result<SourceScan> {
        let db = Connection::open_with_flags(&self.stats_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| {
                format!(
                    "只读打开 OMP 用量数据库 {} 失败: {e}",
                    self.stats_db.display()
                )
            })?;
        // Failed requests carry no usage — OMP records `error`/`aborted`
        // rows with zero tokens — so they stay out of request counts; a
        // failure row that did record tokens was partially streamed and
        // billed, and keeps counting.
        let mut query = db
            .prepare(
                "SELECT session_file, entry_id, model, timestamp, input_tokens, \
                 output_tokens, cache_read_tokens, cache_write_tokens, total_tokens \
                 FROM messages WHERE timestamp >= ?1 \
                 AND (stop_reason NOT IN ('error', 'aborted') OR total_tokens > 0)",
            )
            .map_err(|e| format!("OMP 用量表无效: {e}"))?;
        let mut rows = query
            .query(params![since])
            .map_err(|e| format!("读取 OMP 用量失败: {e}"))?;
        let mut records = Vec::new();
        while let Some(row) = rows.next().map_err(|e| format!("读取 OMP 用量失败: {e}"))? {
            let record: rusqlite::Result<OmpMessageRow> = (|| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })();
            let (
                session_file,
                entry_id,
                model,
                timestamp,
                input,
                output,
                read,
                write,
                tokens,
            ) = record.map_err(|e| format!("OMP 用量记录无效: {e}"))?;
            // The source's own UNIQUE(session_file, entry_id) stays stable
            // when `omp stats --summary` rebuilds the database, so repeated
            // scans archive each request exactly once.
            records.push(UsageRecord {
                external_id: format!("{session_file}\u{1}{entry_id}"),
                model,
                timestamp,
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: read,
                cache_write_tokens: write,
                total_tokens: tokens,
            });
        }
        let synced_at = [
            self.stats_db.clone(),
            self.stats_db.with_extension("db-wal"),
        ]
        .iter()
        .filter_map(|path| {
            fs::metadata(path)
                .ok()?
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()
        })
        .filter_map(|time| i64::try_from(time.as_millis()).ok())
        .max()
        .unwrap_or(0);
        Ok(SourceScan { records, synced_at })
    }
}

impl AgentUsageAdapter for OmpUsageAdapter {
    fn sync_source(&self) -> platform::Result<()> {
        self.sync_cli()
    }

    fn scan(&self, since: i64) -> platform::Result<SourceScan> {
        self.scan_db(since)
    }
}

fn normalize_profile(value: &str) -> platform::Result<Option<String>> {
    let value = value.trim();
    if value.is_empty() || value == "default" {
        return Ok(None);
    }
    let valid = value.len() <= 64
        && value
            .bytes()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && value.bytes().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
        })
        && !value.ends_with('.');
    let basename = value.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(basename.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (basename.len() == 4
            && (basename.starts_with("COM") || basename.starts_with("LPT"))
            && basename.as_bytes()[3].is_ascii_digit());
    if !valid || reserved {
        return Err(format!("OMP profile 名称无效: {value}"));
    }
    Ok(Some(value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Scans the adapter's source into a fresh archive under `root` and
    // aggregates the requested range — the exact production flow.
    fn archive_and_query(
        adapter: &OmpUsageAdapter,
        root: &Path,
        range: &str,
        prices: &Pricing,
    ) -> crate::usage::UsageStats {
        let store = crate::usage::store::UsageStore::new(root).unwrap();
        let scan = adapter.scan(0).unwrap();
        store.ingest("omp", &scan.records).unwrap();
        store
            .query(
                Some("omp"),
                crate::usage::UsageRange::parse(range, crate::usage::now_millis().unwrap())
                    .unwrap(),
                prices,
            )
            .unwrap()
            .finish()
    }

    #[test]
    fn custom_range_excludes_end_boundary_from_every_aggregate() {
        let root = env::temp_dir().join(format!("amc-omp-range-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let stats_db = root.join("stats.db");
        let db = Connection::open(&stats_db).unwrap();
        db.execute_batch(
            "CREATE TABLE messages (
                session_file TEXT NOT NULL, entry_id TEXT NOT NULL,
                provider TEXT NOT NULL, model TEXT NOT NULL,
                timestamp INTEGER NOT NULL, input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL, cache_read_tokens INTEGER NOT NULL,
                cache_write_tokens INTEGER NOT NULL, total_tokens INTEGER NOT NULL,
                stop_reason TEXT NOT NULL DEFAULT 'stop'
            )",
        )
        .unwrap();
        let start = 1_700_000_000_000_i64;
        let end = start + 86_400_000;
        for (entry, timestamp, model, tokens) in [
            ("before", start - 1, "excluded", 100),
            ("first", start, "gpt-6-sol", 11),
            ("last", end - 1, "gpt-6-sol", 13),
            ("after", end, "excluded", 200),
        ] {
            db.execute(
                "INSERT INTO messages VALUES ('session', ?1, 'openai', ?2, ?3, 1, 2, 3, 0, ?4, 'stop')",
                params![entry, model, timestamp, tokens],
            )
            .unwrap();
        }
        drop(db);
        let adapter = OmpUsageAdapter {
            stats_db,
            mcp_json: root.join("agent/mcp.json"),
            config_root: root.clone(),
        };
        let stats = archive_and_query(
            &adapter,
            &root,
            &format!("custom:{start}:{end}"),
            &prices(),
        );
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.total_tokens, 24);
        assert_eq!(stats.input_tokens, 2);
        assert_eq!(stats.output_tokens, 4);
        assert_eq!(stats.cache_read_tokens, 6);
        assert_eq!(
            stats.total_cost,
            Some(2. * (2. + 0.2 * 3. + 10. * 2.) / 1_000_000.)
        );
        assert_eq!(stats.unpriced_requests, 0);
        assert_eq!(stats.by_model.len(), 1);
        assert_eq!(stats.by_model[0].model, "gpt-6-sol");
        assert_eq!(stats.by_model[0].requests, 2);
        assert_eq!(stats.by_model[0].total_tokens, 24);
        assert_eq!(stats.by_model[0].cost, stats.total_cost);
        assert_eq!(
            stats.trend.iter().map(|point| point.requests).sum::<i64>(),
            2
        );
        assert_eq!(
            stats
                .trend
                .iter()
                .map(|point| point.total_tokens)
                .sum::<i64>(),
            24
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mixed_models_show_only_priced_subtotal() {
        let root = env::temp_dir().join(format!("amc-omp-pricing-{}", uuid::Uuid::new_v4()));
        let stats_db = root.join("stats.db");
        fixture_stats(&stats_db, "fixture-entry", 6);
        let db = Connection::open(&stats_db).unwrap();
        db.execute(
            "INSERT INTO messages VALUES ('session', 'openai-entry', 'openai', 'gpt-6-sol', 1700000000000, 1000000, 0, 0, 0, 1000000, 'stop')",
            [],
        )
        .unwrap();
        drop(db);
        let adapter = OmpUsageAdapter {
            stats_db,
            mcp_json: root.join("agent/mcp.json"),
            config_root: root.clone(),
        };
        let stats = archive_and_query(&adapter, &root, "all", &prices());
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.unpriced_requests, 1);
        assert_eq!(stats.total_cost, Some(4.0));
        assert_eq!(
            stats
                .by_model
                .iter()
                .find(|m| m.model == "model")
                .unwrap()
                .cost,
            None
        );
        assert_eq!(
            stats
                .by_model
                .iter()
                .find(|m| m.model == "gpt-6-sol")
                .unwrap()
                .cost,
            Some(4.0)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_requests_without_usage_are_excluded_from_stats() {
        let root = env::temp_dir().join(format!("amc-omp-failed-{}", uuid::Uuid::new_v4()));
        let stats_db = root.join("stats.db");
        fixture_stats(&stats_db, "success", 6);
        let db = Connection::open(&stats_db).unwrap();
        // Failed rows carry no usage at all (error, user abort); a failure
        // that streamed partial output keeps its billed tokens, with the
        // buckets summing to the row's total.
        for (entry, stop_reason, model, input, output, read, write, total) in [
            ("error-empty", "error", "model", 0, 0, 0, 0, 0),
            ("aborted-empty", "aborted", "model", 0, 0, 0, 0, 0),
            ("error-partial", "error", "gpt-6-sol", 2, 4, 3, 0, 9),
        ] {
            db.execute(
                "INSERT INTO messages (session_file, entry_id, provider, model, \
                 timestamp, input_tokens, output_tokens, cache_read_tokens, \
                 cache_write_tokens, total_tokens, stop_reason) \
                 VALUES ('session', ?1, 'fixture', ?2, 1700000000000, \
                 ?3, ?4, ?5, ?6, ?7, ?8)",
                params![entry, model, input, output, read, write, total, stop_reason],
            )
            .unwrap();
        }
        drop(db);
        let adapter = OmpUsageAdapter {
            stats_db,
            mcp_json: root.join("agent/mcp.json"),
            config_root: root.clone(),
        };
        // The scan itself drops usage-less failures; the partial billed
        // failure survives alongside the success.
        assert_eq!(adapter.scan(0).unwrap().records.len(), 2);
        let stats = archive_and_query(&adapter, &root, "all", &prices());
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.total_tokens, 15);
        assert_eq!(stats.input_tokens, 3);
        assert_eq!(stats.output_tokens, 6);
        assert_eq!(stats.cache_read_tokens, 6);
        assert_eq!(stats.unpriced_requests, 1);
        // Dropping empty failures changes request counts only; the priced
        // partial row's cost is untouched.
        let expected = (2. * 2. + 0.2 * 3. + 10. * 4.) / 1_000_000.;
        assert_eq!(stats.total_cost, Some(expected));
        assert_eq!(stats.by_model.len(), 2);
        assert_eq!(
            stats
                .by_model
                .iter()
                .find(|m| m.model == "gpt-6-sol")
                .unwrap()
                .cost,
            Some(expected)
        );
        fs::remove_dir_all(root).unwrap();
    }

    // Records are archived once as raw tokens; editing the catalog reprices
    // the same rows without re-reading the source.
    #[test]
    fn editing_pricing_json_reprices_saved_usage_without_resync() {
        let root = env::temp_dir().join(format!("amc-usage-reprice-{}", uuid::Uuid::new_v4()));
        let stats_db = root.join("stats.db");
        fixture_stats(&stats_db, "entry", 6);
        let price_file = root.join(crate::pricing::FILE_NAME);
        let adapter = OmpUsageAdapter {
            stats_db,
            mcp_json: root.join("agent/mcp.json"),
            config_root: root.clone(),
        };
        let store = crate::usage::store::UsageStore::new(&root).unwrap();
        let scan = adapter.scan(0).unwrap();
        store.ingest("omp", &scan.records).unwrap();
        let range = crate::usage::UsageRange::parse(
            "all",
            crate::usage::now_millis().unwrap(),
        )
        .unwrap();
        fs::write(&price_file, r#"{"models":{"model":{"input":10000,"output":20000,"cache_read":30000}}}"#).unwrap();
        let first = store
            .query(Some("omp"), range, &Pricing::load(&price_file).unwrap())
            .unwrap()
            .finish();
        assert!((first.total_cost.unwrap() - 0.14).abs() < 1e-12);
        assert_eq!(first.unpriced_requests, 0);
        fs::write(&price_file, r#"{"models":{"model":{"input":20000,"output":30000,"cache_read":40000}}}"#).unwrap();
        let second = store
            .query(Some("omp"), range, &Pricing::load(&price_file).unwrap())
            .unwrap()
            .finish();
        assert!((second.total_cost.unwrap() - 0.20).abs() < 1e-12);
        assert!((second.by_model[0].cost.unwrap() - 0.20).abs() < 1e-12);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn profiles_validate_and_normalize_before_constructing_paths() {
        assert_eq!(normalize_profile(" default ").unwrap(), None);
        assert_eq!(
            normalize_profile(" team_a ").unwrap(),
            Some("team_a".into())
        );
        assert!(normalize_profile("../other").is_err());
        assert!(normalize_profile("con.foo").is_err());
    }
    #[test]
    fn profile_and_xdg_paths_follow_omp_migration_rules() {
        let root = env::temp_dir().join(format!("amc-omp-paths-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let xdg = root.join("xdg");
        let config = PathBuf::from("custom-omp");
        fs::create_dir_all(xdg.join("omp")).unwrap();
        let fallback = OmpUsageAdapter::from_configuration(
            home.clone(),
            config.clone(),
            "team",
            None,
            None,
            Some(xdg.clone()),
            &root,
        )
        .unwrap();
        assert_eq!(
            fallback.stats_db,
            home.join("custom-omp/profiles/team/stats.db")
        );

        fs::create_dir_all(xdg.join("omp/profiles/team")).unwrap();
        let migrated = OmpUsageAdapter::from_configuration(
            home.clone(),
            config.clone(),
            "team",
            None,
            None,
            Some(xdg.clone()),
            &root,
        )
        .unwrap();
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert_eq!(migrated.stats_db, xdg.join("omp/profiles/team/stats.db"));
        } else {
            assert_eq!(migrated.stats_db, fallback.stats_db);
        }
        let custom_agent = root.join("agent-other");
        let overridden = OmpUsageAdapter::from_configuration(
            home.clone(),
            config.clone(),
            "",
            None,
            Some(custom_agent.clone()),
            Some(xdg.clone()),
            &root,
        )
        .unwrap();
        assert_eq!(overridden.stats_db, home.join("custom-omp/stats.db"));

        let derived = home.join("custom-omp/profiles/team/agent");
        let reset = OmpUsageAdapter::from_configuration(
            home,
            config,
            "default",
            Some("team"),
            Some(derived),
            Some(xdg.clone()),
            &root,
        )
        .unwrap();
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert_eq!(reset.stats_db, xdg.join("omp/stats.db"));
        }
        fs::remove_dir_all(root).unwrap();
    }
    fn fixture_stats(path: &Path, entry_id: &str, tokens: i64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE messages (
                session_file TEXT NOT NULL, entry_id TEXT NOT NULL,
                provider TEXT NOT NULL, model TEXT NOT NULL,
                timestamp INTEGER NOT NULL, input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL, cache_read_tokens INTEGER NOT NULL,
                cache_write_tokens INTEGER NOT NULL, total_tokens INTEGER NOT NULL,
                stop_reason TEXT NOT NULL DEFAULT 'stop'
            )",
        )
        .unwrap();
        db.execute(
            "INSERT INTO messages VALUES ('session', ?1, 'fixture', 'model', 1700000000000, 1, 2, 3, 0, ?2, 'stop')",
            params![entry_id, tokens],
        )
        .unwrap();
    }

    #[test]
    fn home_dotenv_redirects_actual_usage_database_without_inheriting_amc_process_env() {
        let root = env::temp_dir().join(format!("amc-omp-dotenv-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let cwd = root.join("project");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            home.join(".env"),
            "PI_CONFIG_DIR='from-home'\nOMP_PROFILE=late-profile\n",
        )
        .unwrap();
        let db = home.join("from-home/stats.db");
        fixture_stats(&db, "entry", 321);
        let adapter = OmpUsageAdapter::from_launch_context(home, &cwd, HashMap::new()).unwrap();
        let stats = archive_and_query(&adapter, &root, "all", &prices());
        assert_eq!(adapter.stats_db, db);
        assert_eq!(stats.total_tokens, 321);
        assert_eq!(stats.total_requests, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_and_agent_dotenv_override_lower_priority_paths_and_mirror_omp_aliases() {
        let root = env::temp_dir().join(format!("amc-omp-dotenv-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let cwd = root.join("project");
        let xdg = root.join("xdg");
        fs::create_dir_all(home.join(".omp/agent")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(cwd.join(".env"), "OMP_CONFIG_DIR=project-root\n").unwrap();
        fs::write(
            home.join(".omp/agent/.env"),
            format!("XDG_DATA_HOME=\"{}\"\n", xdg.display(),),
        )
        .unwrap();
        fs::write(home.join(".omp/.env"), "XDG_DATA_HOME=/wrong/root\n").unwrap();
        fs::write(
            home.join(".env"),
            "PI_CONFIG_DIR=home-root\nXDG_DATA_HOME=/wrong/home\n",
        )
        .unwrap();
        let db = xdg.join("omp/stats.db");
        fixture_stats(&db, "entry", 654);
        let adapter = OmpUsageAdapter::from_launch_context(home, &cwd, HashMap::new()).unwrap();
        let expected = if cfg!(any(target_os = "macos", target_os = "linux")) {
            db
        } else {
            root.join("home/project-root/stats.db")
        };
        if !cfg!(any(target_os = "macos", target_os = "linux")) {
            fixture_stats(&expected, "entry", 654);
        }
        assert_eq!(adapter.stats_db, expected);
        let stats = archive_and_query(&adapter, &root, "all", &prices());
        assert_eq!(stats.total_tokens, 654);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn profile_agent_dotenv_relocates_usage_to_profile_xdg_data() {
        let root = env::temp_dir().join(format!("amc-omp-dotenv-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let cwd = root.join("project");
        let xdg = root.join("xdg");
        fs::create_dir_all(home.join(".omp/profiles/team/agent")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            home.join(".omp/profiles/team/agent/.env"),
            format!("XDG_DATA_HOME='{}'\n", xdg.display(),),
        )
        .unwrap();
        let db = xdg.join("omp/profiles/team/stats.db");
        fixture_stats(&db, "entry", 987);
        let adapter = OmpUsageAdapter::from_launch_context(
            home.clone(),
            &cwd,
            HashMap::from([("OMP_PROFILE".into(), "team".into())]),
        )
        .unwrap();
        let expected = if cfg!(any(target_os = "macos", target_os = "linux")) {
            db
        } else {
            home.join(".omp/profiles/team/stats.db")
        };
        if !cfg!(any(target_os = "macos", target_os = "linux")) {
            fixture_stats(&expected, "entry", 987);
        }
        assert_eq!(adapter.stats_db, expected);
        assert_eq!(
            archive_and_query(&adapter, &root, "all", &prices()).total_tokens,
            987,
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn installed_omp_stats_db_matches_project_dotenv_resolution_with_mode_files() {
        let Some(program) = executable() else {
            // OMP is optional in cross-platform CI; the Rust-only path tests
            // above remain deterministic without a locally installed CLI.
            return;
        };
        let root = env::temp_dir().join(format!("amc-omp-cli-env-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let project = root.join("project");
        let xdg = root.join("xdg");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(xdg.join("omp")).unwrap();
        fs::write(project.join(".env"), "PI_CONFIG_DIR=base\n").unwrap();
        fs::write(project.join(".env.production"), "PI_CONFIG_DIR=mode\n").unwrap();
        fs::write(project.join(".env.local"), "PI_CONFIG_DIR=local\n").unwrap();
        fs::write(
            project.join(".env.production.local"),
            format!(
                "PI_CONFIG_DIR=mode-local\nXDG_DATA_HOME={}\n",
                xdg.display()
            ),
        )
        .unwrap();
        let output = platform::cli_command(&program)
            .unwrap()
            .current_dir(&project)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("NODE_ENV", "production")
            .env_remove("PI_CONFIG_DIR")
            .env_remove("PI_CODING_AGENT_DIR")
            .env_remove("OMP_PROFILE")
            .env_remove("PI_PROFILE")
            .env_remove("XDG_DATA_HOME")
            .args(["stats", "--summary"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "真实 OMP 同步失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let candidates = [
            home.join("base/stats.db"),
            home.join("mode/stats.db"),
            home.join("local/stats.db"),
            home.join("mode-local/stats.db"),
            xdg.join("omp/stats.db"),
        ];
        let created: Vec<_> = candidates
            .into_iter()
            .filter(|path| path.is_file())
            .collect();
        assert_eq!(created.len(), 1, "OMP 应只写入一个统计库: {created:?}");
        let adapter = OmpUsageAdapter::from_launch_context(
            home,
            &project,
            HashMap::from([("NODE_ENV".into(), "production".into())]),
        )
        .unwrap();
        assert_eq!(
            adapter.stats_db, created[0],
            "AMC 必须只读 OMP 此次实际写入的 stats.db"
        );
        assert_eq!(
            archive_and_query(&adapter, &root, "all", &prices()).total_requests,
            0
        );
        fs::remove_dir_all(root).unwrap();
    }
}
