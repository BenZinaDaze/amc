use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

pub type Result<T, E = String> = std::result::Result<T, E>;

pub fn home() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        home_from_values(env::var_os("USERPROFILE"), env::var_os("HOME"))
    }
    #[cfg(not(windows))]
    {
        home_from_values(env::var_os("HOME"), env::var_os("USERPROFILE"))
    }
}

fn home_from_values(
    primary: Option<std::ffi::OsString>,
    fallback: Option<std::ffi::OsString>,
) -> Result<PathBuf> {
    primary
        .filter(|value| !value.is_empty())
        .or_else(|| fallback.filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .ok_or_else(|| "无法定位用户目录，无法扫描通用来源".into())
}

pub fn valid_mcp_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b':'))
}

pub fn valid_skill_name(value: &str) -> bool {
    let mut count = 0;
    let mut previous_hyphen = false;
    for character in value.chars() {
        if count == 64 {
            return false;
        }
        if character == '-' {
            if count == 0 || previous_hyphen {
                return false;
            }
            previous_hyphen = true;
        } else if !character.is_ascii_lowercase() && !character.is_ascii_digit() {
            return false;
        } else {
            previous_hyphen = false;
        }
        count += 1;
    }
    count > 0 && !previous_hyphen
}

pub fn no_links(path: &Path, base: &Path) -> Result<()> {
    let relative = path
        .strip_prefix(base)
        .map_err(|_| format!("路径超出管理范围: {}", path.display()))?;
    let mut current = base.to_path_buf();
    for part in std::iter::once(Component::CurDir).chain(relative.components()) {
        if part != Component::CurDir {
            if !matches!(part, Component::Normal(_)) {
                return Err("路径包含非法分量".into());
            }
            current.push(part);
        }
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!("拒绝符号链接: {}", current.display()))
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(format!("检查 {} 失败: {e}", current.display())),
        }
    }
    Ok(())
}
pub fn skill_metadata(file: &Path) -> Result<(String, String)> {
    let text =
        fs::read_to_string(file).map_err(|e| format!("读取 {} 失败: {e}", file.display()))?;
    skill_metadata_text(&text, file)
}

fn skill_metadata_text(text: &str, file: &Path) -> Result<(String, String)> {
    let front = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---"))
        .ok_or_else(|| format!("{} 缺少 YAML frontmatter", file.display()))?
        .0;
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(front).map_err(|e| format!("SKILL.md 元数据无效: {e}"))?;
    let name = yaml
        .get("name")
        .and_then(serde_yaml::Value::as_str)
        .unwrap_or("")
        .trim();
    let description = yaml
        .get("description")
        .and_then(serde_yaml::Value::as_str)
        .unwrap_or("")
        .trim();
    if !valid_skill_name(name) || description.is_empty() {
        return Err(format!(
            "{} 必须包含符合 Agent Skills 规范的 name 和 description",
            file.display()
        ));
    }
    Ok((name.to_owned(), description.to_owned()))
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn snapshot(directory: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    snapshot_impl(directory, false)
}

pub fn source_snapshot(
    directory: &Path,
    repository_root: &Path,
) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    snapshot_impl(
        directory,
        directory == repository_root || directory == repository_root.join("."),
    )
}

fn snapshot_impl(directory: &Path, skip_root_git: bool) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    fn visit(
        path: &Path,
        relative: &Path,
        files: &mut Vec<(PathBuf, Vec<u8>)>,
        total_bytes: &mut u64,
        skip_root_git: bool,
    ) -> Result<()> {
        let mut entries = fs::read_dir(path)
            .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?
            .map(|entry| entry.map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name();
            if skip_root_git && relative.as_os_str().is_empty() && name == ".git" {
                continue;
            }
            let rel = relative.join(name);
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                visit(&entry.path(), &rel, files, total_bytes, false)?;
            } else if kind.is_file() {
                if files.len() >= 4096
                    || entry.metadata().map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024
                {
                    return Err("技能文件超出安全大小限制（4096 个文件 / 单文件 64 MiB）".into());
                }
                let data = fs::read(entry.path()).map_err(|e| e.to_string())?;
                *total_bytes = total_bytes
                    .checked_add(data.len() as u64)
                    .ok_or("技能大小溢出")?;
                if *total_bytes > 128 * 1024 * 1024 {
                    return Err("技能总大小超过 128 MiB".into());
                }
                files.push((rel, data));
            } else {
                return Err(format!(
                    "技能目录含符号链接或特殊文件: {}",
                    entry.path().display()
                ));
            }
        }
        Ok(())
    }
    if directory.exists() {
        if !directory.is_dir()
            || fs::symlink_metadata(directory)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
        {
            return Err(format!("技能目录不安全: {}", directory.display()));
        }
        visit(
            directory,
            Path::new(""),
            &mut files,
            &mut total_bytes,
            skip_root_git,
        )?;
    }
    Ok(files)
}

pub fn tree_hash(files: &[(PathBuf, Vec<u8>)]) -> String {
    let mut hasher = Sha256::new();
    for (name, data) in files {
        hasher.update(name.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update((data.len() as u64).to_be_bytes());
        hasher.update(data);
    }
    format!("{:x}", hasher.finalize())
}

pub fn omp_executable() -> Option<PathBuf> {
    omp_candidates().find(|program| program.is_file())
}

fn omp_candidates() -> impl Iterator<Item = PathBuf> {
    let executable = if cfg!(windows) { "omp.exe" } else { "omp" };
    // Finder does not inherit the user's interactive shell PATH.
    env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .chain(
            home()
                .ok()
                .into_iter()
                .flat_map(|home| [home.join(".bun/bin"), home.join(".local/bin")]),
        )
        .chain(
            ["/opt/homebrew/bin", "/usr/local/bin"]
                .into_iter()
                .map(PathBuf::from),
        )
        .map(move |directory| directory.join(executable))
}

pub fn omp_command(program: &Path) -> Result<Command> {
    // npm/bun shims commonly use `#!/usr/bin/env bun`; a Finder-launched
    // process cannot resolve that interpreter from its minimal PATH.
    let home = home()?;
    let mut paths = vec![
        program
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf(),
        home.join(".bun/bin"),
        home.join(".local/bin"),
    ];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    let path = env::join_paths(paths).map_err(|e| format!("构造 OMP 执行路径失败: {e}"))?;
    let mut command = Command::new(program);
    command.env("PATH", path);
    Ok(command)
}

pub fn omp_version() -> (bool, String) {
    let mut found = false;
    for program in omp_candidates() {
        if !program.is_file() {
            continue;
        }
        found = true;
        if let Ok(mut command) = omp_command(&program) {
            if let Ok(output) = command.arg("--version").output() {
                if output.status.success() {
                    return (
                        true,
                        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
                    );
                }
            }
        }
    }
    (found, String::new())
}

pub const HIDDEN_MCP_VALUE: &str = "[已隐藏]";

pub fn redact_mcp_in_place(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                let name = key.to_ascii_lowercase();
                if name == "env" || name == "headers" {
                    if let Value::Object(entries) = child {
                        for entry in entries.values_mut() {
                            *entry = Value::String(HIDDEN_MCP_VALUE.into());
                        }
                    } else {
                        *child = Value::String(HIDDEN_MCP_VALUE.into());
                    }
                } else if matches!(
                    name.as_str(),
                    "authorization"
                        | "token"
                        | "accesstoken"
                        | "refreshtoken"
                        | "clientsecret"
                        | "credentialid"
                        | "apikey"
                        | "password"
                        | "secret"
                ) {
                    *child = Value::String(HIDDEN_MCP_VALUE.into());
                } else {
                    redact_mcp_in_place(child);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_mcp_in_place(item);
            }
        }
        _ => (),
    }
}

pub fn redacted_mcp(value: &Value) -> Value {
    let mut result = value.clone();
    redact_mcp_in_place(&mut result);
    result
}

pub fn restore_mcp_secrets(config: &mut Value, original: Option<&Value>) -> Result<()> {
    fn restore(value: &mut Value, original: Option<&Value>, masked: Option<&Value>) -> Result<()> {
        if value.as_str() == Some(HIDDEN_MCP_VALUE) {
            if masked.and_then(Value::as_str) != Some(HIDDEN_MCP_VALUE) {
                return Err("新的 MCP 凭据不能使用隐藏占位符；请输入实际值".into());
            }
            *value = original
                .ok_or("MCP 凭据原值已不存在，请重新加载配置")?
                .clone();
            return Ok(());
        }
        match value {
            Value::Object(fields) => {
                for (name, child) in fields {
                    restore(
                        child,
                        original.and_then(|value| value.get(name)),
                        masked.and_then(|value| value.get(name)),
                    )?;
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter_mut().enumerate() {
                    restore(
                        child,
                        original.and_then(|value| value.get(index)),
                        masked.and_then(|value| value.get(index)),
                    )?;
                }
            }
            _ => (),
        }
        Ok(())
    }
    let masked = original.map(redacted_mcp);
    restore(config, original, masked.as_ref())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpView {
    pub name: String,
    pub config: Value,
    pub source: String,
    pub enabled: bool,
    pub managed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillView {
    pub name: String,
    pub path: String,
    pub source: String,
    pub managed: bool,
    pub shadowed: bool,
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn home_values_prefer_primary_and_fallback_when_primary_is_empty() {
        assert_eq!(
            home_from_values(
                Some(OsString::from("/primary")),
                Some(OsString::from("/fallback"))
            )
            .unwrap(),
            PathBuf::from("/primary")
        );
        assert_eq!(
            home_from_values(Some(OsString::new()), Some(OsString::from("/fallback"))).unwrap(),
            PathBuf::from("/fallback")
        );
        assert!(home_from_values(None, None).is_err());
    }

    #[test]
    fn skill_names_follow_agent_skills_constraints() {
        assert!(valid_skill_name("pdf-processing"));
        assert!(valid_skill_name("skill2"));
        assert!(!valid_skill_name("PDF-processing"));
        assert!(!valid_skill_name("-pdf"));
        assert!(!valid_skill_name("pdf-"));
        assert!(!valid_skill_name("pdf--processing"));
        assert!(!valid_skill_name("pdf_processing"));
        assert!(!valid_skill_name(&"a".repeat(65)));
    }

    #[test]
    fn mcp_names_keep_mcp_compatibility_constraints() {
        assert!(valid_mcp_name("server_name:v1"));
        assert!(!valid_mcp_name("server/name"));
        assert!(!valid_mcp_name(&"a".repeat(101)));
    }
}
