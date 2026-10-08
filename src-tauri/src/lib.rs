mod mcp;
mod operations;
mod platform;
mod pricing;
mod skills;
mod store;
#[cfg(test)]
mod test_support;
mod subscription;
mod usage;
mod update;
mod workspace;

use std::sync::{Arc, Mutex};
use serde::Serialize;
use tauri::{Emitter, Manager, State};

type Shared = Arc<Mutex<operations::Core>>;

async fn dispatch<T: Send + 'static>(
    state: State<'_, Shared>,
    task: impl FnOnce(&mut operations::Core) -> platform::Result<T> + Send + 'static,
) -> platform::Result<T> {
    let core = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = core.lock().map_err(|_| "AMC 状态锁已损坏".to_string())?;
        task(&mut guard)
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn get_default_workspace() -> platform::Result<workspace::Workspace> {
    workspace::default_workspace()
}

#[tauri::command]
async fn get_state(
    state: State<'_, Shared>,
    workspace: workspace::Workspace,
) -> platform::Result<operations::State> {
    dispatch(state, move |core| core.state(workspace)).await
}

#[tauri::command]
async fn plan_mcp(
    state: State<'_, Shared>,
    name: String,
    config: Option<serde_json::Value>,
    agents: Vec<mcp::Agent>,
) -> platform::Result<operations::Plan> {
    dispatch(
        state,
        move |core| core.plan_mcp(name, config, agents),
    )
    .await
}

#[tauri::command]
async fn plan_mcp_toggle(
    state: State<'_, Shared>,
    name: String,
    agent: mcp::Agent,
    enabled: bool,
) -> platform::Result<operations::Plan> {
    dispatch(
        state,
        move |core| core.plan_mcp_toggle(name, agent, enabled),
    )
    .await
}

#[tauri::command]
async fn apply_plan(state: State<'_, Shared>, id: String) -> platform::Result<operations::Message> {
    dispatch(state, move |core| core.apply(id)).await
}

#[tauri::command]
async fn add_repository(
    state: State<'_, Shared>,
    url: String,
    reference: String,
) -> platform::Result<store::Repository> {
    dispatch(state, move |core| core.add_repository(url, reference)).await
}

#[tauri::command]
async fn remove_repository(
    state: State<'_, Shared>,
    repository_id: i64,
) -> platform::Result<operations::Message> {
    dispatch(state, move |core| core.remove_repository(repository_id)).await
}

#[tauri::command]
async fn list_repository_skills(
    state: State<'_, Shared>,
    repository_id: i64,
) -> platform::Result<Vec<operations::RepositorySkill>> {
    dispatch(state, move |core| core.repository_skills(repository_id)).await
}

#[tauri::command]
async fn check_updates(
    state: State<'_, Shared>,
    repository_id: i64,
) -> platform::Result<operations::Message> {
    dispatch(state, move |core| core.check_updates(repository_id)).await
}

#[tauri::command]
async fn check_all_updates(state: State<'_, Shared>) -> platform::Result<operations::Message> {
    dispatch(state, |core| core.check_all_updates()).await
}

#[tauri::command]
async fn plan_skill(
    state: State<'_, Shared>,
    repository_id: i64,
    skill_path: String,
) -> platform::Result<operations::Plan> {
    dispatch(state, move |core| core.plan_skill(repository_id, skill_path)).await
}

#[tauri::command]
async fn plan_skill_toggle(
    state: State<'_, Shared>,
    name: String,
    target: skills::SkillTarget,
    enabled: bool,
) -> platform::Result<operations::Plan> {
    dispatch(state, move |core| {
        core.plan_skill_toggle(name, target, enabled)
    })
    .await
}

#[tauri::command]
async fn plan_skill_remove(
    state: State<'_, Shared>,
    name: String,
) -> platform::Result<operations::Plan> {
    dispatch(state, move |core| core.plan_skill_remove(name)).await
}

#[tauri::command]
async fn sync_agent_usage(
    app: tauri::AppHandle,
    agent_id: String,
    range: String,
) -> platform::Result<usage::UsageStats> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let price_file = data_dir.join(pricing::FILE_NAME);
    tauri::async_runtime::spawn_blocking(move || {
        usage::sync_agent_usage(&agent_id, &range, &data_dir, &price_file)
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn get_agent_usage(
    app: tauri::AppHandle,
    agent_id: String,
    range: String,
) -> platform::Result<usage::UsageStats> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let price_file = data_dir.join(pricing::FILE_NAME);
    tauri::async_runtime::spawn_blocking(move || {
        usage::get_agent_usage(&agent_id, &range, &data_dir, &price_file)
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn sync_agents_usage(
    app: tauri::AppHandle,
    range: String,
) -> platform::Result<usage::UsageStats> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let price_file = data_dir.join(pricing::FILE_NAME);
    tauri::async_runtime::spawn_blocking(move || {
        usage::sync_agents_usage(&range, &data_dir, &price_file)
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn get_agents_usage(
    app: tauri::AppHandle,
    range: String,
) -> platform::Result<usage::UsageStats> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let price_file = data_dir.join(pricing::FILE_NAME);
    tauri::async_runtime::spawn_blocking(move || {
        usage::get_agents_usage(&range, &data_dir, &price_file)
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn get_claude_code_status() -> platform::Result<usage::ClaudeCodeStatus> {
    tauri::async_runtime::spawn_blocking(usage::claude_code_status)
        .await
        .map_err(|e| format!("后台操作失败: {e}"))
}

#[tauri::command]
async fn get_codex_status() -> platform::Result<usage::CodexStatus> {
    tauri::async_runtime::spawn_blocking(usage::codex_status)
        .await
        .map_err(|e| format!("后台操作失败: {e}"))
}

#[tauri::command]
async fn refresh_pricing(app: tauri::AppHandle) -> platform::Result<String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || pricing::refresh(&data_dir))
        .await
        .map_err(|e| format!("后台操作失败: {e}"))?
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SubscriptionLoadEvent<'a> {
    nonce: u64,
    statuses: &'a [subscription::SubscriptionStatus],
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SubscriptionStatusEvent<'a> {
    nonce: u64,
    status: &'a subscription::SubscriptionStatus,
}

#[tauri::command]
async fn fetch_subscriptions(
    app: tauri::AppHandle,
    nonce: u64,
) -> platform::Result<Vec<subscription::SubscriptionStatus>> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    // Cards stream in one by one (`subscription-load` placeholders, then one
    // `subscription-status` per finished query) so the UI never waits for the
    // slowest vendor; `nonce` lets the frontend drop stale requests' events.
    tauri::async_runtime::spawn_blocking(move || {
        Ok(subscription::fetch_all_streaming(&data_dir, |event| match &event {
            subscription::FetchEvent::Begin(statuses) => {
                let _ = app.emit(
                    "subscription-load",
                    SubscriptionLoadEvent {
                        nonce,
                        statuses: statuses.as_slice(),
                    },
                );
            }
            subscription::FetchEvent::Ready(status) => {
                let _ = app.emit(
                    "subscription-status",
                    SubscriptionStatusEvent { nonce, status },
                );
            }
        }))
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn list_subscription_kinds() -> platform::Result<Vec<subscription::SubscriptionKind>> {
    Ok(subscription::list_kinds())
}

#[tauri::command]
async fn add_subscription_plan(
    app: tauri::AppHandle,
    kind: String,
    name: String,
    platform: String,
    key: String,
    base_url: Option<String>,
) -> platform::Result<subscription::Message> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        subscription::add_plan(
            &data_dir,
            &kind,
            &name,
            &platform,
            &key,
            base_url.as_deref(),
            None,
        )?;
        Ok(subscription::Message {
            message: format!("已添加订阅套餐 {name}"),
        })
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn update_subscription_plan(
    app: tauri::AppHandle,
    id: String,
    name: String,
    platform: String,
    key: Option<String>,
    base_url: Option<String>,
) -> platform::Result<subscription::Message> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        subscription::update_plan(
            &data_dir,
            &id,
            &name,
            &platform,
            key.as_deref(),
            base_url.as_deref(),
        )?;
        Ok(subscription::Message {
            message: format!("已更新订阅套餐 {name}"),
        })
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn remove_subscription_plan(
    app: tauri::AppHandle,
    id: String,
) -> platform::Result<subscription::Message> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        subscription::remove_plan(&data_dir, &id)?;
        Ok(subscription::Message {
            message: "已移除订阅套餐".to_owned(),
        })
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn antigravity_login_and_add(
    app: tauri::AppHandle,
    name: String,
) -> platform::Result<subscription::Message> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        subscription::antigravity_login_and_add(&data_dir, &name)?;
        Ok(subscription::Message {
            message: format!("已添加订阅套餐 {name}"),
        })
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn check_app_update() -> platform::Result<update::AppUpdate> {
    tauri::async_runtime::spawn_blocking(update::check)
        .await
        .map_err(|e| format!("后台操作失败: {e}"))?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            pricing::ensure_cache(&data_dir).map_err(std::io::Error::other)?;
            let core = operations::Core::new(data_dir).map_err(std::io::Error::other)?;
            app.manage(Arc::new(Mutex::new(core)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_default_workspace,
            get_state,
            plan_mcp,
            plan_mcp_toggle,
            apply_plan,
            add_repository,
            remove_repository,
            list_repository_skills,
            check_updates,
            check_all_updates,
            plan_skill,
            plan_skill_toggle,
            plan_skill_remove,
            sync_agent_usage,
            get_agent_usage,
            sync_agents_usage,
            get_agents_usage,
            get_claude_code_status,
            get_codex_status,
            refresh_pricing,
            fetch_subscriptions,
            list_subscription_kinds,
            add_subscription_plan,
            update_subscription_plan,
            remove_subscription_plan,
            antigravity_login_and_add,
            check_app_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
