mod omp;
mod omp_usage;
mod operations;
mod pricing;
mod store;
mod subscription;
mod workspace;

use std::sync::{Arc, Mutex};
use tauri::{Manager, State};

type Shared = Arc<Mutex<operations::Core>>;

async fn dispatch<T: Send + 'static>(
    state: State<'_, Shared>,
    task: impl FnOnce(&mut operations::Core) -> omp::Result<T> + Send + 'static,
) -> omp::Result<T> {
    let core = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = core.lock().map_err(|_| "AMC 状态锁已损坏".to_string())?;
        task(&mut guard)
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn get_default_workspace() -> omp::Result<workspace::Workspace> {
    workspace::default_workspace()
}

#[tauri::command]
async fn get_state(
    state: State<'_, Shared>,
    workspace: workspace::Workspace,
) -> omp::Result<operations::State> {
    dispatch(state, move |core| core.state(workspace)).await
}

#[tauri::command]
async fn plan_mcp(
    state: State<'_, Shared>,
    workspace: workspace::Workspace,
    name: String,
    config: Option<serde_json::Value>,
) -> omp::Result<operations::Plan> {
    dispatch(state, move |core| core.plan_mcp(workspace, name, config)).await
}

#[tauri::command]
async fn apply_plan(state: State<'_, Shared>, id: String) -> omp::Result<operations::Message> {
    dispatch(state, move |core| core.apply(id)).await
}

#[tauri::command]
async fn add_repository(
    state: State<'_, Shared>,
    url: String,
    reference: String,
) -> omp::Result<store::Repository> {
    dispatch(state, move |core| core.add_repository(url, reference)).await
}

#[tauri::command]
async fn remove_repository(
    state: State<'_, Shared>,
    repository_id: i64,
) -> omp::Result<operations::Message> {
    dispatch(state, move |core| core.remove_repository(repository_id)).await
}

#[tauri::command]
async fn list_repository_skills(
    state: State<'_, Shared>,
    repository_id: i64,
) -> omp::Result<Vec<operations::RepositorySkill>> {
    dispatch(state, move |core| core.repository_skills(repository_id)).await
}

#[tauri::command]
async fn check_updates(
    state: State<'_, Shared>,
    repository_id: i64,
) -> omp::Result<operations::Message> {
    dispatch(state, move |core| core.check_updates(repository_id)).await
}

#[tauri::command]
async fn check_all_updates(state: State<'_, Shared>) -> omp::Result<operations::Message> {
    dispatch(state, |core| core.check_all_updates()).await
}

#[tauri::command]
async fn plan_skill(
    state: State<'_, Shared>,
    workspace: workspace::Workspace,
    repository_id: i64,
    skill_path: String,
) -> omp::Result<operations::Plan> {
    dispatch(state, move |core| {
        core.plan_skill(workspace, repository_id, skill_path)
    })
    .await
}

#[tauri::command]
async fn plan_sync(
    state: State<'_, Shared>,
    installation_id: i64,
) -> omp::Result<operations::Plan> {
    dispatch(state, move |core| {
        let record = core.store.record(installation_id)?;
        core.plan_sync_record(record)
    })
    .await
}

#[tauri::command]
async fn plan_remove_skill(
    state: State<'_, Shared>,
    installation_id: i64,
) -> omp::Result<operations::Plan> {
    dispatch(state, move |core| core.plan_remove_skill(installation_id)).await
}

#[tauri::command]
async fn rollback_skill(
    state: State<'_, Shared>,
    installation_id: i64,
) -> omp::Result<operations::Plan> {
    dispatch(state, move |core| core.rollback_skill(installation_id)).await
}

#[tauri::command]
async fn sync_agent_usage(
    app: tauri::AppHandle,
    agent_id: String,
    range: String,
) -> omp::Result<omp_usage::UsageStats> {
    let price_file = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join(pricing::FILE_NAME);
    tauri::async_runtime::spawn_blocking(move || {
        omp_usage::sync_agent_usage(&agent_id, &range, &price_file)
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn get_agent_usage(
    app: tauri::AppHandle,
    agent_id: String,
    range: String,
) -> omp::Result<omp_usage::UsageStats> {
    let price_file = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join(pricing::FILE_NAME);
    tauri::async_runtime::spawn_blocking(move || {
        omp_usage::get_agent_usage(&agent_id, &range, &price_file)
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn sync_agents_usage(
    app: tauri::AppHandle,
    range: String,
) -> omp::Result<omp_usage::UsageStats> {
    let price_file = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join(pricing::FILE_NAME);
    tauri::async_runtime::spawn_blocking(move || {
        omp_usage::sync_agents_usage(&range, &price_file)
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn get_agents_usage(
    app: tauri::AppHandle,
    range: String,
) -> omp::Result<omp_usage::UsageStats> {
    let price_file = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join(pricing::FILE_NAME);
    tauri::async_runtime::spawn_blocking(move || {
        omp_usage::get_agents_usage(&range, &price_file)
    })
    .await
    .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn refresh_pricing(app: tauri::AppHandle) -> omp::Result<String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || pricing::refresh(&data_dir))
        .await
        .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn fetch_subscriptions(
    app: tauri::AppHandle,
) -> omp::Result<Vec<subscription::SubscriptionStatus>> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || Ok(subscription::fetch_all(&data_dir)))
        .await
        .map_err(|e| format!("后台操作失败: {e}"))?
}

#[tauri::command]
async fn list_subscription_kinds() -> omp::Result<Vec<subscription::SubscriptionKind>> {
    Ok(subscription::list_kinds())
}

#[tauri::command]
async fn add_subscription_plan(
    app: tauri::AppHandle,
    kind: String,
    name: String,
    platform: String,
    key: String,
) -> omp::Result<subscription::Message> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        subscription::add_plan(&data_dir, &kind, &name, &platform, &key)?;
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
) -> omp::Result<subscription::Message> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        subscription::update_plan(&data_dir, &id, &name, &platform, key.as_deref())?;
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
) -> omp::Result<subscription::Message> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        subscription::remove_plan(&data_dir, &id)?;
        Ok(subscription::Message {
            message: "已移除订阅套餐".to_owned(),
        })
    })
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
            apply_plan,
            add_repository,
            remove_repository,
            list_repository_skills,
            check_updates,
            check_all_updates,
            plan_skill,
            plan_sync,
            plan_remove_skill,
            rollback_skill,
            sync_agent_usage,
            get_agent_usage,
            sync_agents_usage,
            get_agents_usage,
            refresh_pricing,
            fetch_subscriptions,
            list_subscription_kinds,
            add_subscription_plan,
            update_subscription_plan,
            remove_subscription_plan
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
