# aether

A Rust + Tauri 2 framework for AI desktop apps with model providers, streaming chat, tool approval, Skills, MCP client/server, observability, and Svelte UI.

- 设计与任务：`docs/00-概要设计.md` → 各详细设计 → `docs/90-开发任务清单.md`（开发唯一入口）。
- 当前状态：已实现至 R6-2；R6-1 §12.1 文档待补。

## 目录

| 路径 | 内容 |
| --- | --- |
| `crates/aether-*` | 核心 crate，不依赖 Tauri |
| `plugin/tauri-plugin-aether` | Tauri 接入层，按 feature 启用能力 |
| `packages/ui` | `@ouraihub/aether-ui`，子路径导出各能力组件 |
| `examples/` | `assistant`、`providers-only`、`mcp-server-only`（Tauri 窗口）与 `cli`（无 Tauri） |
| `scripts/` | Windows 本地构建用的便携 MSVC 环境脚本 |

## 本地检查（Windows）

```bat
pnpm install
node_modules\@ouraihub\desktop-kit\msvc\cargo-msvc.bat check --workspace --all-features
pnpm run check
```

本地构建固定使用便携 MSVC（由 [desktop-kit](https://github.com/ouraihub/desktop-kit) 提供；默认 `D:\Applications\msvc-portable\msvc`，可用 `PORTABLE_MSVC_ROOT` 覆盖；首次运行 `node_modules\@ouraihub\desktop-kit\msvc\setup-msvc.bat` 安装）。

## macOS / Linux 凭据与完整检查

Windows DPAPI 格式保持不变。macOS 使用用户 Keychain，Linux 使用已解锁的持久 Secret Service（例如 GNOME Keyring）；没有服务或服务锁定就拒绝保存凭据，不影响不需要凭据的离线功能，不回落普通文件主密钥或明文。宿主可复用 `aether_providers::secrets::{protect, unprotect}` 的字节接口，具体格式和错误见 [基础设计 §2](docs/10-详细设计-基础与可观测性.md#2-secret决策-d3windows--macos--linux-原生存储)。

Linux 原生编译需 `libdbus-1-dev`，完整 Tauri workspace 另需 GTK/WebKitGTK 4.1、AppIndicator、OpenSSL 和打包工具（依赖清单见 `.github/workflows/ci.yml`，Ubuntu 22.04 基线）。在桌面已解锁的 Secret Service 会话中直接运行 `pnpm install --frozen-lockfile && pnpm run ci`；headless runner 用真实服务：

```sh
dbus-run-session -- xvfb-run -a bash -euc '
  export XDG_RUNTIME_DIR="$(mktemp -d)"
  chmod 700 "$XDG_RUNTIME_DIR"
  eval "$(printf "%s" aether-ci | gnome-keyring-daemon --unlock --components=secrets)"
  pnpm run ci
'
```

macOS 在已解锁的用户 Keychain 会话中运行同一 `pnpm run ci`；CI 创建临时、已解锁的默认 Keychain，并只为该一次性钥匙串预置允许不同测试可执行文件访问的随机测试主密钥，结束后移除（不依赖 mock 或开发者凭据；生产代码不放宽 Keychain ACL）。两平台完整检查保留 fmt、Clippy、整个 workspace 测试和各插件 feature 检查；此外编译 `native-secrets` 示例，以四个并发子进程首次创建主密钥，另起进程读回凭据和provider数据库、逐字节篡改拒绝。Linux 使用隔离HOME，再切断该进程D-Bus地址验证保存失败且数据库原密文不变，恢复会话后再次读取。macOS保留真实HOME以保持原生用户Keychain偏好域，另建空的临时默认钥匙串隔离首次创建，完成后恢复默认钥匙串及搜索列表；不能仅改子进程HOME却沿用原用户域的钥匙串配置。

三个 Tauri 窗口示例共用 `examples/icon.ico`（Windows）和 `examples/icon.png`（32×32 RGBA，macOS/Linux）；即使 `bundle.active=false`，`generate_context!()` 仍读取窗口图标。完整 DTO 检查会编译这些示例，不得通过跳过 workspace 或关闭 DTO 检查规避缺图标错误。

单独原生冒烟命令（必须在同一个已解锁会话、相同 HOME 中执行；只写加密测试数据）：

```sh
cargo build -p aether-providers --example native-secrets
target/debug/examples/native-secrets race /tmp/aether-native-race
target/debug/examples/native-secrets seal /tmp/aether-native.blob
target/debug/examples/native-secrets open /tmp/aether-native.blob
target/debug/examples/native-secrets tamper /tmp/aether-native.blob
# Linux 验证服务不可用仍保留数据库凭据：
DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent/aether-bus target/debug/examples/native-secrets unavailable /tmp/aether-native.blob
target/debug/examples/native-secrets open /tmp/aether-native.blob
target/debug/examples/native-secrets forget /tmp/aether-native.blob
```

纯加密单元测试不访问原生存储；整个 workspace 的凭据集成测试需要真实服务。`forget` 清除示例文件/数据库，不删除共享主密钥（其他 BLOB 与备份仍需它）；新锁文件 `HOME/.aether/credential-v1.lock` 只有锁元数据，不能在运行中删除。更换 HOME/系统用户、丢失 Keychain 条目或跨平台搬移数据库不会自动迁移密钥，需重新输入凭据。

## 清理磁盘

Rust 编译产物会占用数 GB，定期清理：

```bat
scripts\clean.bat           :: 删除 target\、生成文件；保留所有 data\ 目录
scripts\clean.bat --deep    :: 同时删除 node_modules（之后需 pnpm install）
```

仓库内有进程（示例程序、cargo、vite）占用文件时会拒绝执行。

## UI 开发

基础组件用 shadcn-svelte。动 UI 前先读 `.claude/skills/shadcn-svelte/SKILL.md`，新页面或改版参考 `.claude/skills/hallmark/SKILL.md`。
