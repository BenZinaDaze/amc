//! Persisting subscription entries under the app data dir with owner-only
//! permissions. The frontend never receives keys; only masked hints cross the
//! boundary. Vendor-specific credential shaping lives in the vendor modules.

use crate::platform::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use super::{known_kind, validate_base_url, validate_platform};

/// Subscriptions the user saved inside AMC, persisted with owner-only
/// permissions under the app data dir. The frontend never receives keys.
const SUBSCRIPTIONS_FILE: &str = "subscriptions.json";

/// One subscription instance; a vendor may appear several times.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredSubscription {
    /// Unique among the current entries: `"1"`, `"2"`, …
    pub id: String,
    /// Provider kind, e.g. `glm`; see [`known_kind`].
    pub kind: String,
    /// User-defined card name.
    pub name: String,
    /// `zai` (api.z.ai) or `bigmodel` (open.bigmodel.cn); empty for kinds
    /// without platform choices.
    pub platform: String,
    /// Instance URL for kinds that need one (e.g. `sub2api`).
    #[serde(default)]
    pub base_url: Option<String>,
    pub key: String,
}

impl StoredSubscription {
    pub(super) fn hint(&self) -> String {
        let tail: String = self.key.chars().rev().take(4).collect();
        format!("…{}", tail.chars().rev().collect::<String>())
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredSubscriptions {
    /// Monotonic counter behind [`next_entry_id`]; ids are never reused.
    #[serde(default)]
    next_id: u64,
    #[serde(default)]
    pub(super) entries: Vec<StoredSubscription>,
}

pub(super) fn read_stored(data_dir: &Path) -> StoredSubscriptions {
    let mut stored: StoredSubscriptions = fs::read_to_string(data_dir.join(SUBSCRIPTIONS_FILE))
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default();
    // Counter-less files (hand-written) would otherwise fall back to
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
    let bytes = serde_json::to_vec_pretty(&stored).map_err(|e| format!("序列化失败: {e}"))?;
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

// ---------------------------------------------------------------- entry CRUD

pub fn add_plan(
    data_dir: &Path,
    kind: &str,
    name: &str,
    platform: &str,
    key: &str,
    base_url: Option<&str>,
) -> Result<()> {
    if !known_kind(kind) {
        return Err(format!("未知的订阅套餐: {kind}"));
    }
    let name = validate_name(name)?;
    let platform = validate_platform(kind, platform)?;
    let base_url = validate_base_url(kind, base_url)?;
    let key = validate_key(key)?;
    let mut stored = read_stored(data_dir);
    let id = next_entry_id(&mut stored);
    stored.entries.push(StoredSubscription {
        id,
        kind: kind.to_owned(),
        name,
        platform,
        base_url,
        key,
    });
    write_stored_entries(data_dir, stored)
}

/// A `None` key keeps the stored one, so edits never need the raw key; the
/// instance URL is not secret and is always replaced.
pub fn update_plan(
    data_dir: &Path,
    id: &str,
    name: &str,
    platform: &str,
    key: Option<&str>,
    base_url: Option<&str>,
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
    entry.base_url = validate_base_url(&entry.kind, base_url)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::fetch_all;
    use serde_json::json;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("amc-stored-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn entry_crud_and_validation() {
        let dir = temp_dir("crud");
        assert!(add_plan(&dir, "glm", "  ", "zai", "12345678", None).is_err());
        assert!(add_plan(&dir, "nope", "名", "zai", "12345678", None).is_err());
        assert!(add_plan(&dir, "glm", "名", "unknown", "12345678", None).is_err());
        assert!(add_plan(&dir, "glm", "名", "zai", "short", None).is_err());
        assert!(add_plan(&dir, "sub2api", "名", "", "sk-abcdef123456", None).is_err());
        assert!(add_plan(
            &dir,
            "sub2api",
            "名",
            "",
            "sk-abcdef123456",
            Some("ftp://x")
        )
        .is_err());
        assert!(add_plan(
            &dir,
            "sub2api",
            "名",
            "zai",
            "sk-abcdef123456",
            Some("https://x.y")
        )
        .is_err());
        assert!(fetch_all(&dir).is_empty());

        // The same vendor can be added twice with different keys.
        add_plan(&dir, "glm", "  主号  ", "zai", "  12345678abcdef  ", None).unwrap();
        add_plan(&dir, "glm", "备用", "bigmodel", "fedcba9876543210", None).unwrap();
        add_plan(
            &dir,
            "sub2api",
            "自建",
            "",
            "sk-abcdef123456",
            Some("  https://sub.example.com/  "),
        )
        .unwrap();
        let entries = read_stored(&dir).entries;
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id, "1");
        assert_eq!(entries[1].id, "2");
        assert_eq!(entries[2].id, "3");
        assert_eq!(entries[0].name, "主号");
        assert_eq!(entries[0].platform, "zai");
        assert_eq!(entries[0].key, "12345678abcdef");
        assert_eq!(entries[0].hint(), "…cdef");
        assert_eq!(entries[0].base_url, None);
        assert_eq!(entries[2].platform, "");
        assert_eq!(
            entries[2].base_url.as_deref(),
            Some("https://sub.example.com")
        );

        // A missing key keeps the stored one; a blank one is rejected.
        update_plan(&dir, "1", "主力", "bigmodel", None, None).unwrap();
        assert!(update_plan(&dir, "1", "主力", "bigmodel", Some("  "), None).is_err());
        let entry = &read_stored(&dir).entries[0];
        assert_eq!(entry.name, "主力");
        assert_eq!(entry.platform, "bigmodel");
        assert_eq!(entry.key, "12345678abcdef");
        update_plan(&dir, "1", "主力", "zai", Some("aaaa1234"), None).unwrap();
        assert_eq!(read_stored(&dir).entries[0].key, "aaaa1234");
        assert!(update_plan(&dir, "99", "x", "zai", None, None).is_err());
        // The instance URL is replaced on every edit.
        update_plan(&dir, "3", "自建", "", None, Some("http://127.0.0.1:9")).unwrap();
        assert_eq!(
            read_stored(&dir).entries[2].base_url.as_deref(),
            Some("http://127.0.0.1:9")
        );

        remove_plan(&dir, "1").unwrap();
        assert_eq!(read_stored(&dir).entries.len(), 2);
        remove_plan(&dir, "1").unwrap(); // removing again is a no-op

        // Each entry renders as its own card with its own name and masked key.
        let statuses = fetch_all(&dir);
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].id, "2");
        assert_eq!(statuses[0].title, "备用");
        assert_eq!(statuses[0].provider, "glm");
        assert_eq!(statuses[0].platform, "bigmodel");
        assert_eq!(statuses[0].key_hint.as_deref(), Some("…3210"));
        assert_eq!(statuses[1].provider, "sub2api");
        assert_eq!(statuses[1].base_url.as_deref(), Some("http://127.0.0.1:9"));
    }

    #[test]
    fn entry_ids_are_never_reused() {
        let dir = temp_dir("ids");
        add_plan(&dir, "glm", "甲", "zai", "12345678abcdef", None).unwrap();
        add_plan(&dir, "glm", "乙", "zai", "12345678abcdef", None).unwrap();
        remove_plan(&dir, "2").unwrap();
        add_plan(&dir, "glm", "丙", "zai", "12345678abcdef", None).unwrap();
        let stored = read_stored(&dir);
        let ids: Vec<&str> = stored.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["1", "3"]);
        // A stale view of the deleted entry cannot touch the new one.
        assert!(update_plan(&dir, "2", "幽灵", "zai", None, None).is_err());
        remove_plan(&dir, "2").unwrap(); // removing a stale id is a no-op
        assert_eq!(read_stored(&dir).entries.len(), 2);

        remove_plan(&dir, "1").unwrap();
        add_plan(&dir, "glm", "丁", "zai", "12345678abcdef", None).unwrap();
        let stored = read_stored(&dir);
        let ids: Vec<&str> = stored.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["3", "4"]);
    }

    #[test]
    fn delete_before_add_never_reuses_id() {
        // A counter-less file (hand-written) must not let the next add claim
        // an id that a stored entry already used.
        let dir = temp_dir("counterless");
        let file = json!({ "entries": [
            { "id": "5", "kind": "glm", "name": "手写", "platform": "zai", "key": "12345678abcdef" }
        ] });
        fs::write(
            dir.join(SUBSCRIPTIONS_FILE),
            serde_json::to_vec(&file).unwrap(),
        )
        .unwrap();
        remove_plan(&dir, "5").unwrap();
        add_plan(&dir, "glm", "新的", "zai", "12345678abcdef", None).unwrap();
        let stored = read_stored(&dir);
        assert_eq!(stored.entries[0].id, "6");
    }
}
