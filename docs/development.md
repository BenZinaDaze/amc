# 开发指南

面向本仓库的贡献者与维护者;使用者的安装与功能说明见 [README](../README.md)。

## 环境要求

- Node.js 22+ 与 npm
- Rust stable(含 macOS 双 target,发布构建需要)
- macOS(产品只面向 macOS;Windows/Linux 可编译但不在支持范围)

## 常用命令

```sh
npm install            # 安装前端依赖
npm run tauri dev      # 完整应用开发运行(DEV 下更新器检查自动跳过)
npm run build          # 前端类型检查 + Vite 构建
cargo test             # 后端测试(在 src-tauri/ 下,全离线)
cargo clippy --all-targets  # 后端 lint,保持零警告
npm run tauri build    # 发布构建(需要签名私钥,见下)
```

测试与架构约定见仓库根目录的 `CLAUDE.md`。

## 更新签名密钥

开启更新器归档后，`npm run tauri build` 需要签名私钥（仅 `tauri dev` 不需要）：

```sh
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/amc.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
```

私钥只保存在本机 `~/.tauri/amc.key` 与仓库 Secrets（`TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`）；丢失后无法为已发布版本签发更新，只能更换密钥对重新发布。

## 发布流程

1. 版本号三处同步:`package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`(CI 会校验与 tag 一致),同时刷新 `package-lock.json` 与 `src-tauri/Cargo.lock`。
2. 撰写 `docs/releases/vX.Y.Z.md`(硬性步骤):写明本版功能与修复;范围内容有 `feat` 提交时,须逐条追加 `<!--feat:<完整 oid>-->` 隐藏标记。
3. 提交 `chore: release vX.Y.Z`,打轻量标签 `vX.Y.Z` 并推送。
4. 标签触发 `.github/workflows/release.yml`:notes 校验 → 双架构构建 `--bundles app,dmg` → 产物按架构重命名 → 组装 `latest.json` → 先传二进制、最后补传 `latest.json`(它出现即代表 Release 就绪)。
