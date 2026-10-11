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
scripts\cargo-msvc.bat check --workspace --all-features
pnpm install
pnpm run check
```

本地构建固定使用便携 MSVC（默认 `D:\Applications\msvc-portable\msvc`，可用 `PORTABLE_MSVC_ROOT` 覆盖；首次运行 `scripts\setup-msvc.bat` 安装）。

## 清理磁盘

Rust 编译产物会占用数 GB，定期清理：

```bat
scripts\clean.bat           :: 删除 target\、生成文件；保留所有 data\ 目录
scripts\clean.bat --deep    :: 同时删除 node_modules（之后需 pnpm install）
```

仓库内有进程（示例程序、cargo、vite）占用文件时会拒绝执行。

## UI 开发

基础组件用 shadcn-svelte。动 UI 前先读 `.claude/skills/shadcn-svelte/SKILL.md`，新页面或改版参考 `.claude/skills/hallmark/SKILL.md`。
