//! zcode 远程设备的探测、凭据存储与用量快照同步。zcode 桌面端连接远程
//! 设备（SSH）时在远程机上跑独立的 zcode host，用量数据全部落在远程机
//! 自己的 `~/.zcode/cli/db/db.sqlite`，本机库零镜像——AMC 只能自己通过
//! SSH 拉取。设备清单探测自 `~/.zcode/v2/setting.json`（明文，密码除外：
//! 那是 ZCode 专属 Keychain 加密，AMC 无法解密，必须由用户录入）。

use crate::platform::{self, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DEVICES_FILE: &str = "remote_devices.json";
/// SSH 连接/认证的整体超时：设备常态离线，超时要短到不折磨人。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);

// ------------------------------------------------------------------ 探测

/// `~/.zcode/v2/setting.json` 里 AMC 关心的子集；zcode 未写过的键全部
/// 走 default，schema 演进最多让探测退化为空清单而不是报错。
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ZcodeV2Settings {
    #[serde(default)]
    source_servers: Vec<SourceServer>,
    #[serde(default)]
    connection_profiles: Vec<ConnectionProfile>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
struct SourceServer {
    #[serde(default)]
    source_server_id: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    os_user: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    install_state: String,
    #[serde(default)]
    last_seen_version: String,
    #[serde(default)]
    last_online_at: i64,
    #[serde(default)]
    preferred_profile_id: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
struct ConnectionProfile {
    #[serde(default)]
    profile_id: String,
    #[serde(default)]
    source_server_id: Option<String>,
    #[serde(default)]
    target: Option<ProfileTarget>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
struct ProfileTarget {
    #[serde(default)]
    host: String,
    #[serde(default)]
    port: u16,
    #[serde(default)]
    username: String,
}

/// 探测到的设备（连接参数来自 zcode 的连接 profile，可能为空）。
#[derive(Debug)]
struct DetectedDevice {
    id: String,
    display_name: String,
    os_user: String,
    hostname: String,
    install_state: String,
    last_seen_version: String,
    last_online_at: i64,
    host: String,
    port: u16,
    username: String,
}

fn detect_from_settings(home: &Path) -> Result<Vec<DetectedDevice>> {
    let file = home.join(".zcode").join("v2").join("setting.json");
    let contents = match fs::read_to_string(&file) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("读取 {} 失败: {e}", file.display())),
    };
    let settings: ZcodeV2Settings = serde_json::from_str(&contents)
        .map_err(|e| format!("解析 {} 失败: {e}", file.display()))?;
    Ok(settings
        .source_servers
        .into_iter()
        .filter(|server| !server.source_server_id.is_empty())
        .map(|server| {
            // host 漂移（换网络/VPN）会让探测值过时，所以探测只是默认值，
            // 用户在 AMC 里保存过的字段优先。
            let profile = settings
                .connection_profiles
                .iter()
                .find(|profile| {
                    server
                        .preferred_profile_id
                        .as_deref()
                        .is_some_and(|id| id == profile.profile_id)
                })
                .or_else(|| {
                    settings.connection_profiles.iter().find(|profile| {
                        profile
                            .source_server_id
                            .as_deref()
                            .is_some_and(|id| id == server.source_server_id)
                    })
                });
            let target = profile
                .and_then(|profile| profile.target.clone())
                .unwrap_or_default();
            DetectedDevice {
                id: server.source_server_id,
                display_name: server.display_name,
                os_user: server.os_user,
                hostname: server.hostname,
                install_state: server.install_state,
                last_seen_version: server.last_seen_version,
                last_online_at: server.last_online_at,
                host: target.host,
                port: target.port,
                username: target.username,
            }
        })
        .collect())
}

// ------------------------------------------------------------------ 存储

/// AMC 自己保存的远程设备凭据与状态；密码只进后端，跨 IPC 的永远是掩码
/// 提示。display_name 是探测快照——设备从 zcode 移除后列表仍能显示名字。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredDevice {
    id: String,
    display_name: String,
    host: String,
    port: u16,
    username: String,
    /// SSH 密码；None=尚未录入。
    #[serde(default)]
    password: Option<String>,
    /// TOFU 固定的主机密钥指纹（SHA256:…）；换 host/端口时清空重录。
    #[serde(default)]
    host_key: Option<String>,
    #[serde(default)]
    last_sync_ms: i64,
    #[serde(default)]
    last_inserted: i64,
}

impl StoredDevice {
    /// 密码提示：无论长短都不回显任何片段（短密码下掩码即全文），
    /// 统一只报「已保存」；None=尚未录入。
    fn password_hint(&self) -> Option<String> {
        self.password
            .as_deref()
            .map(str::trim)
            .filter(|password| !password.is_empty())
            .map(|_| "已保存".to_owned())
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StoredDevices {
    #[serde(default)]
    devices: Vec<StoredDevice>,
}

/// 读取已存设备。文件缺失=首次运行返回空；存在但损坏必须报错，静默清零
/// 会让下一次保存直接覆盖掉（可能是半截的）凭据文件。
fn read_stored(data_dir: &Path) -> Result<StoredDevices> {
    let path = data_dir.join(DEVICES_FILE);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(StoredDevices::default()),
        Err(e) => return Err(format!("读取远程设备数据 {} 失败: {e}", path.display())),
    };
    serde_json::from_str(&contents).map_err(|e| {
        format!(
            "远程设备数据 {} 已损坏（{e}）；请修复或删除该文件后重试",
            path.display()
        )
    })
}

fn write_stored(data_dir: &Path, stored: &StoredDevices) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(stored).map_err(|e| format!("序列化失败: {e}"))?;
    // 原子替换 + 0600：写中途崩溃最多留下 .tmp 暂存文件，正式文件始终
    // 完整，且密码不会经手世界可读的中间态。
    platform::write_atomic_private(&data_dir.join(DEVICES_FILE), &bytes)
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ------------------------------------------------------------------ 视图与 CRUD

/// 列表页的一条设备：探测信息 + 存储覆盖后的连接参数 + 掩码提示。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDeviceView {
    pub id: String,
    pub display_name: String,
    pub os_user: String,
    pub hostname: String,
    pub install_state: String,
    pub last_seen_version: String,
    pub last_online_at: i64,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password_hint: Option<String>,
    pub host_key_pinned: bool,
    pub last_sync_ms: i64,
    pub last_inserted: i64,
    /// 设备已从 zcode 配置中消失（凭据保留，通常意味着 zcode 卸载了它）。
    pub gone: bool,
}

/// 探测 + 存储合并成列表；探测不到但已保存凭据的设备也要保留，否则已
/// 录入的密码会凭空消失。
pub fn list(data_dir: &Path) -> Result<Vec<RemoteDeviceView>> {
    let detected = detect_from_settings(&platform::home()?)?;
    let stored = read_stored(data_dir)?;
    let mut views = Vec::new();
    for device in &detected {
        let saved = stored.devices.iter().find(|entry| entry.id == device.id);
        let (host, port, username, hint, pinned, last_sync_ms, last_inserted) = match saved {
            Some(entry) => (
                entry.host.clone(),
                entry.port,
                entry.username.clone(),
                entry.password_hint(),
                entry.host_key.is_some(),
                entry.last_sync_ms,
                entry.last_inserted,
            ),
            None => (
                device.host.clone(),
                device.port,
                device.username.clone(),
                None,
                false,
                0,
                0,
            ),
        };
        views.push(RemoteDeviceView {
            id: device.id.clone(),
            display_name: device.display_name.clone(),
            os_user: device.os_user.clone(),
            hostname: device.hostname.clone(),
            install_state: device.install_state.clone(),
            last_seen_version: device.last_seen_version.clone(),
            last_online_at: device.last_online_at,
            host,
            port,
            username,
            password_hint: hint,
            host_key_pinned: pinned,
            last_sync_ms,
            last_inserted,
            gone: false,
        });
    }
    for entry in &stored.devices {
        if detected.iter().any(|device| device.id == entry.id) {
            continue;
        }
        views.push(RemoteDeviceView {
            id: entry.id.clone(),
            display_name: entry.display_name.clone(),
            os_user: String::new(),
            hostname: String::new(),
            install_state: String::new(),
            last_seen_version: String::new(),
            last_online_at: 0,
            host: entry.host.clone(),
            port: entry.port,
            username: entry.username.clone(),
            password_hint: entry.password_hint(),
            host_key_pinned: entry.host_key.is_some(),
            last_sync_ms: entry.last_sync_ms,
            last_inserted: entry.last_inserted,
            gone: true,
        });
    }
    Ok(views)
}

/// 保存/更新一台设备的连接参数；`password` 为 None 保留已存密码（前端
/// 留空即不改）。host 或端口变化时清空 TOFU 指纹——新地址本来就该重新
/// 确认主机密钥。首次保存会以探测到的显示名创建条目。
pub fn save(
    data_dir: &Path,
    id: &str,
    host: &str,
    port: u16,
    username: &str,
    password: Option<&str>,
) -> Result<()> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!("无效的远程设备标识: {id}"));
    }
    let host = host.trim();
    if host.is_empty() {
        return Err("主机地址不能为空".to_owned());
    }
    if port == 0 {
        return Err("端口无效".to_owned());
    }
    let username = username.trim();
    if username.is_empty() {
        return Err("登录用户名不能为空".to_owned());
    }
    let password = password
        .map(str::trim)
        .filter(|password| !password.is_empty())
        .map(str::to_owned);
    let detected = detect_from_settings(&platform::home()?)?;
    let display_name = detected
        .iter()
        .find(|device| device.id == id)
        .map(|device| device.display_name.clone())
        .unwrap_or_else(|| id.chars().take(8).collect());
    let mut stored = read_stored(data_dir)?;
    let entry = match stored.devices.iter_mut().find(|entry| entry.id == id) {
        Some(entry) => entry,
        None => {
            stored.devices.push(StoredDevice {
                id: id.to_owned(),
                display_name: display_name.clone(),
                host: String::new(),
                port: 0,
                username: String::new(),
                password: None,
                host_key: None,
                last_sync_ms: 0,
                last_inserted: 0,
            });
            stored.devices.last_mut().expect("just pushed")
        }
    };
    // host/端口变化 → 主机密钥指纹作废，重新走一次首连确认。
    if entry.host != host || entry.port != port {
        entry.host_key = None;
        entry.last_sync_ms = 0;
        entry.last_inserted = 0;
    }
    entry.display_name = display_name;
    entry.host = host.to_owned();
    entry.port = port;
    entry.username = username.to_owned();
    if let Some(password) = password {
        entry.password = Some(password);
    }
    write_stored(data_dir, &stored)
}

/// 移除一台已从 zcode 消失的设备：删掉 AMC 保存的凭据、主机密钥指纹
/// 与本地快照。已归档的用量（`zcode@<id>`）保留——设备消失后统计仍应
/// 可读（与本地 agent 卸载后 archive 存活的哲学一致）。对探测仍在列的
/// 设备移除没有意义（刷新即回来），前端只对 gone 设备露出入口。
pub fn remove(data_dir: &Path, id: &str) -> Result<()> {
    let mut stored = read_stored(data_dir)?;
    let before = stored.devices.len();
    stored.devices.retain(|entry| entry.id != id);
    if stored.devices.len() == before {
        return Err("该设备没有已保存的数据".to_owned());
    }
    write_stored(data_dir, &stored)?;
    let _ = fs::remove_dir_all(data_dir.join("remote-snapshots").join(id));
    Ok(())
}

// ------------------------------------------------------------------ 同步

/// 一次远程同步的结果。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSyncResult {
    pub inserted: i64,
    pub last_sync_ms: i64,
}

/// TOFU 主机密钥校验：首连接受并记下指纹，之后不匹配即拒绝（防中间
/// 人）。指纹通过共享的 slot 带回——handler 被 connect 消费，无法从返
/// 回值取回。
struct HostKeyHandler {
    expected: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
}

impl russh::client::Handler for HostKeyHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fingerprint = server_public_key
            .public_key()
            .fingerprint(russh::keys::HashAlg::Sha256)
            .to_string();
        *self.seen.lock().expect("指纹锁损坏") = Some(fingerprint.clone());
        Ok(match &self.expected {
            Some(expected) => *expected == fingerprint,
            None => true,
        })
    }
}

/// 从 SFTP 流式下载一个远程文件到本地。
async fn download_file(
    sftp: &russh_sftp::client::SftpSession,
    remote: &str,
    local: &Path,
) -> Result<()> {
    let mut remote_file = sftp
        .open(remote.to_owned())
        .await
        .map_err(|e| format!("打开远程文件 {remote} 失败: {e}"))?;
    if let Some(parent) = local.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("创建快照目录 {} 失败: {e}", parent.display()))?;
    }
    let mut out = tokio::fs::File::create(local)
        .await
        .map_err(|e| format!("创建快照文件 {} 失败: {e}", local.display()))?;
    tokio::io::copy(&mut remote_file, &mut out)
        .await
        .map_err(|e| format!("下载 {remote} 失败: {e}"))?;
    Ok(())
}

/// 同步一台设备：SSH 拉取远程 ZCode 数据库快照（db + WAL + SHM 三件套，
/// WAL 模式下本地打开会自行重放），随后走本地归档入库 `zcode@<id>`。
/// 设备离线、认证失败、远程无数据均返回可读的错误串。
pub async fn sync_device(data_dir: &Path, id: &str) -> Result<RemoteSyncResult> {
    let entry = {
        let stored = read_stored(data_dir)?;
        stored
            .devices
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
            .ok_or_else(|| "远程设备不存在，请刷新列表后重试".to_owned())?
    };
    let Some(password) = entry
        .password
        .as_deref()
        .map(str::trim)
        .filter(|password| !password.is_empty())
    else {
        return Err("请先保存该设备的 SSH 密码".to_owned());
    };
    let address = format!("{}:{}", entry.host, entry.port);
    let seen_fingerprint: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let config = Arc::new(russh::client::Config {
        inactivity_timeout: Some(Duration::from_secs(30)),
        keepalive_interval: Some(Duration::from_secs(15)),
        ..Default::default()
    });
    let handler = HostKeyHandler {
        expected: entry.host_key.clone(),
        seen: seen_fingerprint.clone(),
    };
    let mut session = tokio::time::timeout(
        CONNECT_TIMEOUT,
        russh::client::connect(config, (entry.host.clone(), entry.port), handler),
    )
    .await
    .map_err(|_| format!("连接 {address} 超时，设备可能离线"))?
    .map_err(|error| {
        // 指纹不匹配时 connect 只报泛化错误，对照 seen 给出准确原因。
        if entry.host_key.is_some() {
            if let Some(seen) = seen_fingerprint.lock().expect("指纹锁损坏").as_deref() {
                if Some(seen.to_owned()) != entry.host_key {
                    return format!(
                        "{address} 的主机密钥与上次记录不一致（可能遭遇中间人攻击，或设备重装/换机）；\
                         确认安全后可在保存设备时修改主机或端口以重置记录"
                    );
                }
            }
        }
        format!("连接 {address} 失败: {error}")
    })?;
    let auth = tokio::time::timeout(
        CONNECT_TIMEOUT,
        session.authenticate_password(entry.username.clone(), password.to_owned()),
    )
    .await
    .map_err(|_| format!("登录 {address} 超时"))?
    .map_err(|error| format!("登录 {address} 失败: {error}"))?;
    if !auth.success() {
        return Err("SSH 认证失败，请检查用户名与密码".to_owned());
    }
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| format!("打开 SSH 通道失败: {e}"))?;
    // SftpSession::new 只发 SSH_FXP_INIT，不会自己启动 sftp 子系统；
    // 不先 request_subsystem 的话服务器永远不回版本包，直到超时。
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("请求 SFTP 子系统失败: {e}"))?;
    let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| format!("建立 SFTP 会话失败: {e}"))?;
    let home = sftp
        .canonicalize(".")
        .await
        .map_err(|e| format!("解析远程用户目录失败: {e}"))?;
    let remote_db = format!("{home}/.zcode/cli/db/db.sqlite");
    let snapshot_dir = data_dir.join("remote-snapshots").join(id);
    let local_db = snapshot_dir.join("db.sqlite");
    // 主库缺失是明确的失败；WAL/SHM 是可选件（库已 checkpoint 或干净
    // 关闭时不存在），拉取失败一律跳过，残留的旧快照要清掉以免本地把
    // 上一轮的 WAL 套在新的 db 上。
    download_file(&sftp, &remote_db, &local_db)
        .await
        .map_err(|error| {
            format!("远程设备上未找到 ~/.zcode 数据库（{error}）；请确认远程已安装并使用过 ZCode")
        })?;
    for suffix in ["-wal", "-shm"] {
        let local_side = snapshot_dir.join(format!("db.sqlite{suffix}"));
        let _ = fs::remove_file(&local_side);
        let _ = download_file(&sftp, &format!("{remote_db}{suffix}"), &local_side).await;
    }
    let _ = sftp.close().await;
    let _ = session
        .disconnect(russh::Disconnect::ByApplication, "done", "en")
        .await;
    // 归档是 sqlite + 文件 IO，放回阻塞线程池；数据的含缓存换算、去重
    // 全部复用本地 ZCode 适配器那一套。
    let archive = tauri::async_runtime::spawn_blocking({
        let data_dir = data_dir.to_path_buf();
        let id = id.to_owned();
        let local_db = local_db.clone();
        move || crate::usage::archive_remote_zcode_snapshot(&data_dir, &id, &local_db)
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))??;
    // TOFU 落库 + 同步状态更新。首连（无 expected）时才需要写指纹。
    let pinned = seen_fingerprint.lock().expect("指纹锁损坏").clone();
    let now = now_millis();
    let mut stored = read_stored(data_dir)?;
    if let Some(entry) = stored.devices.iter_mut().find(|entry| entry.id == id) {
        if entry.host_key.is_none() {
            entry.host_key = pinned;
        }
        entry.last_sync_ms = now;
        entry.last_inserted = archive.inserted as i64;
    }
    write_stored(data_dir, &stored)?;
    Ok(RemoteSyncResult {
        inserted: archive.inserted as i64,
        last_sync_ms: now,
    })
}

// ------------------------------------------------------------------ 测试

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("amc-remote-devices-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 一份贴近真实结构的 setting.json fixture：两台设备（其一 profile
    /// 指向 preferred，另一台靠 sourceServerId 关联）加一条无 source-
    /// Server 的孤儿 profile。
    fn settings_fixture() -> String {
        serde_json::json!({
            "sourceServers": [
                {
                    "sourceServerId": "srv-1",
                    "displayName": "testbox · builder",
                    "osUser": "builder",
                    "hostname": "testbox",
                    "installState": "installed",
                    "lastSeenVersion": "3.18.2",
                    "lastOnlineAt": 1794965138103_i64,
                    "preferredProfileId": "prof-1"
                },
                {
                    "sourceServerId": "srv-2",
                    "displayName": "clouddev",
                    "osUser": "root",
                    "hostname": "cloud",
                    "installState": "installed",
                    "lastSeenVersion": "3.17.2",
                    "lastOnlineAt": 100,
                    "preferredProfileId": "missing-profile"
                }
            ],
            "connectionProfiles": [
                {
                    "profileId": "prof-1",
                    "sourceServerId": "srv-1",
                    "displayName": "testbox",
                    "target": { "kind": "ssh", "host": "198.51.100.10", "port": 22, "username": "builder" }
                },
                {
                    "profileId": "prof-orphan",
                    "displayName": "198.51.100.30",
                    "target": { "kind": "ssh", "host": "198.51.100.30", "port": 22, "username": "builder" }
                },
                {
                    "profileId": "prof-2",
                    "sourceServerId": "srv-2",
                    "target": { "kind": "ssh", "host": "build-box.example.com", "port": 22022, "username": "root" }
                }
            ],
            "recentProjects": ["unrelated"],
            "locale": "zh-CN"
        })
        .to_string()
    }

    #[test]
    fn detect_joins_servers_with_profiles_and_tolerates_missing() {
        let home = temp_dir("detect");
        fs::create_dir_all(home.join(".zcode/v2")).unwrap();
        fs::write(home.join(".zcode/v2/setting.json"), settings_fixture()).unwrap();
        let devices = detect_from_settings(&home).unwrap();
        assert_eq!(devices.len(), 2);
        // preferred profile 提供连接参数。
        assert_eq!(devices[0].id, "srv-1");
        assert_eq!(devices[0].host, "198.51.100.10");
        assert_eq!(devices[0].port, 22);
        assert_eq!(devices[0].username, "builder");
        assert_eq!(devices[0].install_state, "installed");
        // preferred 缺失时回退到同 sourceServerId 的 profile。
        assert_eq!(devices[1].id, "srv-2");
        assert_eq!(devices[1].host, "build-box.example.com");
        assert_eq!(devices[1].port, 22022);
        // 孤儿 profile（无 sourceServer）不产生设备。
        assert!(!devices.iter().any(|device| device.host == "198.51.100.30"));

        // 未知/新增的顶层键不阻碍解析；文件缺失是空清单。
        let mut settings: serde_json::Value = serde_json::from_str(&settings_fixture()).unwrap();
        settings["brandNewKey"] = serde_json::json!({ "nested": true });
        fs::write(
            home.join(".zcode/v2/setting.json"),
            serde_json::to_string(&settings).unwrap(),
        )
        .unwrap();
        assert_eq!(detect_from_settings(&home).unwrap().len(), 2);
        fs::remove_file(home.join(".zcode/v2/setting.json")).unwrap();
        assert!(detect_from_settings(&home).unwrap().is_empty());
        // 完全没有 .zcode 目录（zcode 未安装）也是空清单。
        fs::remove_dir_all(home.join(".zcode")).unwrap();
        assert!(detect_from_settings(&home).unwrap().is_empty());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn list_merges_detected_with_stored_and_keeps_gone_devices() {
        let home = temp_dir("list-home");
        let dir = temp_dir("list-data");
        fs::create_dir_all(home.join(".zcode/v2")).unwrap();
        fs::write(home.join(".zcode/v2/setting.json"), settings_fixture()).unwrap();
        // env_lock 只串行化；HOME 的保存/恢复自己负责，否则并行测试会
        // 读到被删掉的变量（或反过来把删除状态漏给后续测试）。
        let _guard = crate::test_support::env_lock();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &home);

        let views = list(&dir).unwrap();
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].host, "198.51.100.10");
        assert_eq!(views[0].password_hint, None);
        assert!(!views[0].host_key_pinned);

        // 保存凭据后：字段覆盖探测值，密码只回掩码。
        save(
            &dir,
            "srv-1",
            " 198.51.100.20 ",
            22,
            " builder ",
            Some(" pass1234 "),
        )
        .unwrap();
        let views = list(&dir).unwrap();
        assert_eq!(views[0].host, "198.51.100.20");
        assert_eq!(views[0].username, "builder");
        // 密码提示不回显任何片段（含长密码），统一只报「已保存」。
        assert_eq!(views[0].password_hint.as_deref(), Some("已保存"));

        // 设备从 zcode 消失（setting.json 清空）→ gone=true 仍列出。
        fs::write(home.join(".zcode/v2/setting.json"), "{}").unwrap();
        let views = list(&dir).unwrap();
        assert_eq!(views.len(), 1);
        assert!(views[0].gone);
        assert_eq!(views[0].display_name, "testbox · builder");
        assert_eq!(views[0].password_hint.as_deref(), Some("已保存"));

        match original_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn save_validates_and_keeps_password_on_none() {
        let dir = temp_dir("save");
        assert!(save(&dir, "srv-1", " ", 22, "u", None).is_err());
        assert!(save(&dir, "srv-1", "h", 0, "u", None).is_err());
        assert!(save(&dir, "srv-1", "h", 22, " ", None).is_err());
        assert!(save(&dir, "../escape", "h", 22, "u", None).is_err());
        save(&dir, "srv-1", "hosta", 22, "user", Some(" secret1 ")).unwrap();
        // None 保留旧密码；Some 才覆盖。
        save(&dir, "srv-1", "hosta", 22, "user", None).unwrap();
        let stored = read_stored(&dir).unwrap();
        assert_eq!(stored.devices[0].password.as_deref(), Some("secret1"));
        save(&dir, "srv-1", "hosta", 22, "user", Some("secret2")).unwrap();
        assert_eq!(
            read_stored(&dir).unwrap().devices[0].password.as_deref(),
            Some("secret2")
        );

        // host 变化清空 TOFU 指纹；host 与端口都不变时保留。
        let mut stored = read_stored(&dir).unwrap();
        stored.devices[0].host_key = Some("SHA256:pin".to_owned());
        write_stored(&dir, &stored).unwrap();
        save(&dir, "srv-1", "hostb", 22, "user", None).unwrap();
        assert_eq!(read_stored(&dir).unwrap().devices[0].host_key, None);
        let mut stored = read_stored(&dir).unwrap();
        stored.devices[0].host_key = Some("SHA256:pin".to_owned());
        write_stored(&dir, &stored).unwrap();
        save(&dir, "srv-1", "hostb", 22, "user", None).unwrap();
        assert_eq!(
            read_stored(&dir).unwrap().devices[0].host_key.as_deref(),
            Some("SHA256:pin")
        );

        // 损坏文件必须报错而不是被覆盖；写入口不留 .tmp。
        fs::write(dir.join(DEVICES_FILE), "{ not json").unwrap();
        assert!(save(&dir, "srv-1", "h", 22, "u", None).is_err());
        assert!(list(&dir).is_err());
        assert_eq!(
            fs::read_to_string(dir.join(DEVICES_FILE)).unwrap(),
            "{ not json"
        );
        fs::remove_file(dir.join(DEVICES_FILE)).unwrap();
        save(&dir, "srv-1", "h", 22, "u", Some("password")).unwrap();
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "残留暂存文件: {leftovers:?}");

        // 移除：删存储条目与本地快照；未保存过的设备报错；再移除同 id 也报错。
        fs::create_dir_all(dir.join("remote-snapshots/srv-1")).unwrap();
        fs::write(dir.join("remote-snapshots/srv-1/db.sqlite"), "x").unwrap();
        assert!(remove(&dir, "srv-2").is_err());
        remove(&dir, "srv-1").unwrap();
        assert!(read_stored(&dir).unwrap().devices.is_empty());
        assert!(!dir.join("remote-snapshots/srv-1").exists());
        assert!(remove(&dir, "srv-1").is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sync_requires_stored_device_and_password() {
        let dir = temp_dir("sync");
        let error = futures_block_on(sync_device(&dir, "nope")).unwrap_err();
        assert!(error.contains("不存在"), "{error}");
        save(&dir, "srv-1", "h", 22, "u", None).unwrap();
        let error = futures_block_on(sync_device(&dir, "srv-1")).unwrap_err();
        assert!(error.contains("密码"), "{error}");
        fs::remove_dir_all(dir).unwrap();
    }

    /// 测试里没有 tokio runtime，用极简的 block_on 驱动同步函数到第一
    /// 个 await 为止（两条失败路径都在任何 .await 之前返回）。
    fn futures_block_on<F: std::future::Future>(future: F) -> F::Output {
        // 简单轮询：这两条错误路径不依赖 IO 完成，首次 poll 即 Ready。
        let mut future = Box::pin(future);
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(output) => output,
            std::task::Poll::Pending => panic!("失败路径不应挂起"),
        }
    }

    /// 真机冒烟：读取当前机器上真实的 zcode setting.json（未安装 zcode
    /// 时探测结果为空，同样算通过）。`--ignored` 手动运行看清单。
    #[test]
    #[ignore = "真机探测冒烟，验证时手动运行"]
    fn detect_real_machine_smoke() {
        let home = platform::home().unwrap();
        let devices = detect_from_settings(&home).unwrap();
        for device in &devices {
            assert!(!device.id.is_empty(), "设备 id 不能为空: {device:?}");
        }
        eprintln!(
            "探测到 {} 台设备: {:?}",
            devices.len(),
            devices
                .iter()
                .map(|device| (device.id.as_str(), device.host.as_str(), device.port))
                .collect::<Vec<_>>()
        );
    }

    /// 真连 e2e：走完整的 save → sync_device（SSH+SFTP+归档）链路，凭据
    /// 从环境变量读取（AMC_SSH_HOST/PORT/USER/PASSWORD），缺任何一个
    /// 即跳过——密码绝不进仓库。
    #[test]
    #[ignore = "需要真实远程设备，凭据经环境变量传入后手动运行"]
    fn sync_real_device_e2e() {
        let Ok(host) = std::env::var("AMC_SSH_HOST") else {
            return;
        };
        let Ok(user) = std::env::var("AMC_SSH_USER") else {
            return;
        };
        let Ok(password) = std::env::var("AMC_SSH_PASSWORD") else {
            return;
        };
        let port: u16 = std::env::var("AMC_SSH_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(22);
        let dir = temp_dir("e2e");
        save(&dir, "srv-e2e", &host, port, &user, Some(&password)).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let first = runtime
            .block_on(async { sync_device(&dir, "srv-e2e").await })
            .expect("首次同步应成功");
        eprintln!("首次同步: 新入库 {inserted} 条", inserted = first.inserted);
        assert!(first.inserted > 0, "真机上应有用量数据可入库");
        assert!(dir.join("remote-snapshots/srv-e2e/db.sqlite").is_file());
        // 再同步一次：幂等，零新增。
        let second = runtime
            .block_on(async { sync_device(&dir, "srv-e2e").await })
            .expect("二次同步应成功");
        assert_eq!(second.inserted, 0, "重复同步不应重复入库");
        eprintln!(
            "二次同步: 新入库 {inserted} 条（幂等）",
            inserted = second.inserted
        );
        // TOFU 指纹已落库。
        assert!(read_stored(&dir).unwrap().devices[0]
            .host_key
            .as_deref()
            .is_some_and(|key| key.starts_with("SHA256:")));
        fs::remove_dir_all(dir).unwrap();
    }
}
