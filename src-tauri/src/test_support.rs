//! 测试专用辅助。进程级互斥锁：任何需要临时修改进程环境变量的测试
//! 都必须持有它，否则并行的测试会互相看到对方的脏环境。

#[allow(dead_code)]
pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
}
