# AMC · Agent Manager Center

macOS 桌面端(Tauri 2)Agent 管理工具:统一管理 OMP / Claude Code / Codex CLI 的 MCP 配置与 Skills 分发,归档统计三家用量,并展示订阅配额卡片。Rust 后端(`src-tauri/`)+ React 19 前端(`src/`),无其他服务端,所有数据在本机。

## 常用命令

```sh
npm run dev                 # 仅前端 Vite dev server
npm run tauri dev           # 完整应用(dev;更新器检查在 DEV 下自动跳过)
npm run build               # tsc 类型检查 + Vite 构建(前端验证用这个,没有前端测试框架)
cargo test                  # 后端测试(在 src-tauri/ 下;120+ 用例,全离线)
cargo clippy --all-targets  # 后端 lint,保持零警告
npm run tauri build         # 发布构建;需要 TAURI_SIGNING_PRIVATE_KEY 环境变量(见 README)
```

测试约定:fixture 一律放 `std::env::temp_dir()` 下以 uuid 命名的目录并在 Drop 中清理;会改写全局环境变量(HOME/CODEX_HOME 等)的测试必须持 `crate::test_support::env_lock()` 互斥。较大的测试文件用 `#[cfg(test)] #[path = "..._tests.rs"] mod tests;` 拆分(见 operations/settings)。

## 架构与数据位置

- **应用数据目录**(`app.path().app_data_dir()`):`amc.sqlite3`(MCP/Skills/仓库记录)、`usage.sqlite3`(用量归档,WAL)、`subscriptions.json`(订阅凭据,0600)、`model-pricing.json`(计价缓存)、`repositories/<id>/`(Git 缓存)、`skills/<name>/`(技能中央副本)、`backups/`(apply 的备份)。
- **用户级配置(只写这些,写入前必预览)**:OMP `~/.omp[/profiles/<p>]/agent/mcp.json` 与 `config.yml`;Claude `~/.claude.json`(或 `$CLAUDE_CONFIG_DIR/.claude.json`);Codex `$CODEX_HOME/config.toml`(默认 `~/.codex`)。技能投影目录:`~/.agents/skills`(OMP 与 Codex 共用)与 `~/.claude/skills`。
- 后端模块:`operations.rs`(核心:plan/apply)+ `store.rs`(amc.sqlite3)+ `mcp/`、`skills.rs`(各 Agent 适配器与投影)+ `usage/`(适配器 + 归档 store)+ `subscription/`(供应商目录 + 各 vendor)+ `platform.rs`(共享基础设施)+ `pricing.rs`、`update.rs`、`workspace.rs`、`settings.rs`。
- 前端:`App.tsx` 壳(导航 + 全局 state/busy/error/notice/plan 预览),`pages/` 每页一个入口,`apps/<域>/` 放业务组件与 hook,`api.ts` 是唯一的 invoke 封装与类型镜像(字段 camelCase,与 Rust `#[serde(rename_all = "camelCase")]` 对齐;改后端 IPC 类型必须同步这里)。

## 核心设计模型

**统一记录 + 投影(cc-switch 模型)**:SQLite 是唯一事实来源,Agent 用户级配置文件只是投影。所有写路径 = `plan_*`(计算写入 + before 快照 + 脱敏预览)→ 用户确认 → `apply_plan`(防漂移校验 → 写文件 → 写 DB → 失败回滚)。**任何新增写路径都必须走这个模型,不允许绕过预览直接写用户文件。**

- apply 防漂移:写入前逐项比对磁盘内容与预览时的 `before` 快照,不一致即报「自预览后已变化,请重新预览」。
- 原子写:配置文件一律 临时文件(0600 + fsync)→ rename 替换(`platform::write_atomic_private` / operations 内 `write_private`);**不允许原地 truncate 写**。含密钥的文件尤其如此。
- 技能归属:只有「当前启用中的投影」(开关开且内容哈希与记录一致)可被覆盖/删除;任何非 AMC 投影的同名目录(含内容相同的用户副本)一律拒绝——内容相等不能证明归属。

## 铁律(违反会反复返工)

1. **密钥/凭据绝不过 Tauri 边界**:前端只见 `[已隐藏]` 占位(`platform::HIDDEN_MCP_VALUE`)或 key 尾部 hint。展示走 `redact_mcp_in_place`/`codex::preview_redacted`(整份文件脱敏,不只 mcp_servers 段);保存时 `restore_mcp_secrets` 从 DB 旧值回填,回填来源只用 DB,不用可能漂移的 Agent 文件;错误信息输出前先 `sanitize`(把 token 替换为 `***`)。
2. **Git 一律带安全参数**:每次调用都加 `-c core.hooksPath=/dev/null -c protocol.ext.allow=never`,env 设 `GIT_TERMINAL_PROMPT=0`、`GIT_SSH_COMMAND="ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new"`;URL/ref 用 `validated_url`/`validated_ref` 校验;clone 时 URL 前必须有 `--` 分隔。仓库中的内容(SKILL.md 等)按不可信数据处理:路径校验 + `no_links` 拒绝符号链接 + 快照大小上限(4096 文件 / 单文件 64MiB / 总量 128MiB)。
3. **HTTP 层统一模式**(各 subscription vendor):`ureq::Agent` 带 `timeout_global`,且必须 `.http_status_as_error(false)`,非 2xx 走自己的状态分支把响应体(截断 200 字符,注意 char boundary)带进错误信息;错误信息经 sanitize 后才返回。
4. **错误文案用中文**,格式「<动作>失败:<原因>」;面向用户的消息同样中文。代码注释也用中文(与现有代码一致)。
5. **前端竞态防护**:每个异步 hook 用自增 `loadId`/nonce 丢弃过期响应(见 `useUsage`、`useSubscriptions`、App 的 reload);订阅流式事件按 nonce 过滤。新写异步加载必须照抄这个模式。
6. **子进程调用要有界**:`--version`/探测类子进程必须带超时(`platform::cli_status`);用户主动触发的长命令(omp stats / update)可以阻塞但不得持锁干等。
7. **usage 归档不变量**:`(source, external_id)` 主键去重;无 token 的失败请求不入库;查询永远读归档不读活源;成本在查询时按当前计价表重算。新 Agent 接入:在 `AGENT_IDS` 注册 + 实现 `AgentUsageAdapter`,不动其他。
8. **订阅 store**:id 单调递增不复用;写文件必须原子替换;文件存在但解析失败要报错(带路径),绝不静默清零覆盖。

## 发布流程

版本号三处同步:`package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`(CI 会校验与 tag 一致)。tag `v*` 触发 `.github/workflows/release.yml`:先检查 `docs/releases/vX.Y.Z.md` 存在且逐条确认 feat 提交(`<!--feat:<oid>-->` 隐藏标记)→ 双架构构建 `--bundles app,dmg`(需 `TAURI_SIGNING_PRIVATE_KEY` secrets,产物 `.app.tar.gz{,.sig}` 按架构重命名)→ 组装 `latest.json` → 先传二进制最后补传 `latest.json`(它出现即代表 Release 就绪)。**发版前写 release notes 是硬性步骤。**

## 已知限制(改动前先了解)

- OMP `config.yml` 经 serde_yaml 整文件重序列化,**会丢注释/排版**(Codex 的 config.toml 走 toml_edit 保格式)。预览 diff 会如实展示;若要根治需引入格式保持的 YAML 编辑器。
- Antigravity / Cursor 的额度接口是未公开的内部接口,字段形状可能随版本变化——解析端只认精确字段,缺失即降级,绝不臆造数值。
- DMG 未做 Apple 公证(README 有 xattr 说明);更新包用 Tauri minisign 签名。
- Windows/Linux 可编译,但产品只面向 macOS。
