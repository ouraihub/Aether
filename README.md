# aether

[![CI](https://github.com/ouraihub/Aether/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/ouraihub/Aether/actions/workflows/ci.yml)

A shared AI foundation with Rust/Tauri desktop services, a TypeScript execution core, Cloudflare Worker adapters, and Svelte UI: model providers, streaming chat, tool approval, Skills, MCP client/server, search, images, and observability. Product routes, login, business data and final resource authorization stay in each host.

- Web 基座：`@ouraihub/aether-contracts` → `@ouraihub/aether-core` → `@ouraihub/aether-worker`；[Worker示例](examples/worker/README.md)组合真实D1/R2与每run一个Durable Object，UI使用`@ouraihub/aether-ui/web`。设计入口：`docs/02-概要设计-Web共享基座.md` → `docs/22-详细设计-跨运行时公共契约.md`、`docs/23-详细设计-Web Worker运行时.md`、`docs/29-详细设计-Web数据与安全.md`。
- 新版职责分层：先读 [00 总览](docs/00-概要设计.md) → [03 自顶向下模块图](docs/03-契约优先架构与职责拆分设计.md) → [模块详细设计索引与迁移验收](docs/24-详细设计-职责分层与实现契约.md)。各能力接口直接更新原设计，长主题分文件，不并行维护新旧两套规范。

## npm 包

本地与 GitHub Actions 统一使用 Node LTS `26.13.0`（Lithium），由 `.node-version` 固定、`actions/setup-node` 读取；`package.json` 的 `engines.node` 同步约束，`devEngines.runtime` 让 pnpm 自动下载并用于项目脚本。pnpm 固定 `12.16.1`（本次升级时 npm `latest` 正式版），由 `packageManager` 声明，不使用预发布版本或浮动 `latest`。

Node 26 下的 jsdom 测试使用 jsdom 自身的 `localStorage`，不读取 Node 原生 Web Storage。

`@ouraihub/aether-{contracts,core,ui,presets,worker}` 以 LGPL-3.0-only 发布至 npmjs，供宿主通过 pnpm 安装固定版本，例如 `pnpm add --save-exact @ouraihub/aether-ui@0.1.0`。发布包保留 TypeScript/Svelte 源码，需要宿主构建工具编译；Worker 包同时包含 `migrations/`，UI 包包含其公开导出的 `fixtures/`。

主干合入 `packages/*/package.json` 的版本更新后，`Publish npm packages` 工作流先对同一提交运行 Windows 完整 CI，并复用 Unix 工作流运行 macOS、Linux 完整 CI，三个平台全部成功后才在独立任务使用仓库 `NPM_TOKEN` Secret 发布尚未发布的版本；也可在 Actions 手动触发，但只允许 `main`。每个 Action 固定提交，发布凭据只注入 publish 步骤，不用于安装或 PR 检查。已发布的同版本自动跳过，不强制覆盖；新改动须先升级对应包版本。pnpm 将内部 `workspace:*` 转为包版本，依赖先于调用方发布。

技能创作工作区在创建时规范化系统临时目录别名（例如 macOS `/var`），避免误判为模型路径链接；工作区内部的软链接、junction 和路径逃逸仍拒绝。

系统技能发现仍拒绝用户输入路径中的链接；其测试先规范化测试框架创建的临时根目录，避免 macOS 系统别名掩盖实际的发现、打包及链接拒绝验收。

本地应急发布仍可使用用户级 `.npmrc`，不把 token 放入仓库。完整 CI 通过后执行 `pnpm --filter "./packages/**" -r publish --access public --no-git-checks --registry https://registry.npmjs.org`。更换 token 时同步更新仓库 `NPM_TOKEN` Secret。

## 目录

| 路径                         | 内容                                                                                     |
| ---------------------------- | ---------------------------------------------------------------------------------------- |
| `crates/aether-*`            | 核心 crate，不依赖 Tauri                                                                 |
| `plugin/tauri-plugin-aether` | Tauri 接入层，按 feature 启用能力                                                        |
| `packages/ui`                | `@ouraihub/aether-ui`，子路径导出各能力组件                                              |
| `packages/contracts`         | 公共TS绑定/预编译校验；canonical DTO：`src/generated`，不手改                            |
| `packages/core`              | TS三协议/RunEngine、Providers/Search/Skills/MCP/Images执行，不依赖UI或平台I/O            |
| `packages/worker`            | 原生fetch组合、WorkerRuntime/DO、D1 CAS/R2、密文凭据与公共能力路由                       |
| `examples/`                  | Tauri `assistant`/`providers-only`/`mcp-server-only`、无Tauri `cli`、Cloudflare `worker` |
| `scripts/`                   | Windows 本地构建用的便携 MSVC 环境脚本                                                   |
| `contracts/`                 | 跨语言JSON Schema、共享样本和行为场景；检查说明见[合同资产](contracts/README.md)         |

Worker运行采用两阶段：`POST /api/v1/runs`准备并返回`{accepted,events}`，首次有效SSE订阅才执行；30秒未订阅取消，断开取消，不支持replay，重启未终态转interrupted而非重放工具。D1保存actor隔离JSON与revision CAS，大文档指向不可变R2 blob；旧blob有意保留至显式安全GC，不承诺D1/R2跨服务事务。

Worker凭据采用AES-GCM key ring与application/tenant/subject/purpose/recordId AAD；public MCP/OAuth只在HTTPS启用。生产远程出站必须使用真正执行connect-time地址策略的可信egress，Search页面读取另需all-address DNS checker；binding配置不是安全证明，普通fetch不能代替。示例loopback例外仅用于本地受控验收，产品仍需接入真实身份/CSRF/授权与部署资源。

当前已实现TS/Worker公共能力，并已执行真实workerd HTTPS模型/工具/审批、隔离、D1/R2、取消/replay拒绝与OAuth PKCE/MCP scope冒烟；完整CI、浏览器UI、重启及Native/各平台验证的最新结果统一见[90任务清单](docs/90-开发任务清单.md)，不将局部冒烟等同产品迁移或全量验收。

## 本地检查（Windows）

```bat
pnpm install
pnpm run ci
```

本地构建固定使用便携 MSVC（由 [desktop-kit](https://github.com/ouraihub/desktop-kit) 提供；默认 `D:\Applications\msvc-portable\msvc`，可用 `PORTABLE_MSVC_ROOT` 覆盖；首次运行 `node_modules\@ouraihub\desktop-kit\msvc\setup-msvc.bat` 安装）。

合并到 `main` 必须通过确定性 `CI gate`，分支须包含最新 `main`，管理员也受约束。普通文档仅运行策略／提交检查；工作流、根 `AGENTS.md` 和 CI 调度／门禁脚本进入自动化检查（工作流语法、分类／门禁行为、提交规则），不因配置变化编译产品；纯 `packages/ui/src` 手写 TS/Svelte/CSS 运行前端 lint、格式、类型、测试、示例构建及 i18n；Rust、依赖、生成 DTO、产品构建脚本和未知路径运行 Windows 完整检查。混合改动取最高风险，无法确定范围完整执行；最终门禁要求所有适用任务真实成功，不把意外 skipped 算通过。自动化检查不能证明新编译命令有效；修改工具链或实际编译参数时须另行手动运行受影响平台的完整检查。

本地 `pnpm run ci` 默认完整执行；显式 `node scripts/ci.mjs --scope docs|automation|frontend` 与远端轻检查复用相同步骤。日常 `CI` 手动触发运行 Windows 完整检查。同 PR 新提交取消旧运行；不实现历史凭据复用，不新增 `actions: read`。AI review 保留辅助意见，但不作为必需检查或自动合并裁判；维护者或获授权助手须处理实质问题后通过正常 PR 合并，不使用管理员绕过。历史失败记录保留，验收看最新提交适用检查。

完整检查还包含纯support/chat/images、独立conversations和插件单feature编译门禁，以及`node scripts/contracts-check.mjs`：AJV共享样本、TypeScript绑定赋值和真实Rust序列化共同校验。Native宿主使用`aether-runtime`运行/会话服务，Tauri插件只做IPC/生命周期适配；外仓升级前阅读[破坏性迁移与验收](docs/24-详细设计-职责分层与实现契约.md)。

## macOS / Linux 凭据与完整检查

macOS/Linux 已移至独立工作流 `macOS and Linux`（`.github/workflows/unix.yml`），仅支持手动触发和每天北京时间 **02:00**（UTC 前一天 18:00，cron `0 18 * * *`）定时运行主干，不在 PR／push 时启动，也不阻塞日常 `CI gate`。GitHub 定时任务可能延迟。手动执行：`gh workflow run unix.yml --ref <分支或tag>`；发版前确认待发布提交两端通过，夜间失败在 Actions 中处理，不当作成功。完整原生检查内容不变。

Windows DPAPI 格式保持不变。macOS 使用用户 Keychain，Linux 使用已解锁的持久 Secret Service（例如 GNOME Keyring）；没有服务或服务锁定就拒绝保存凭据，不影响不需要凭据的离线功能，不回落普通文件主密钥或明文。宿主可复用 `aether_providers::secrets::{protect, unprotect}` 的字节接口，具体格式和错误见 [基础设计 §2](docs/10-详细设计-基础与可观测性.md#2-secret决策-d3windows--macos--linux-原生存储)。

Linux 原生编译需 `libdbus-1-dev`，完整 Tauri workspace 另需 GTK/WebKitGTK 4.1、AppIndicator、OpenSSL 和打包工具（依赖清单见 `.github/workflows/unix.yml`，Ubuntu 22.04 基线）。在桌面已解锁的 Secret Service 会话中直接运行 `pnpm install --frozen-lockfile && pnpm run ci`；headless runner 用真实服务：

Linux CI 的 apt 使用 Ubuntu 官方归档源而非 Azure 区域镜像，禁用推荐包并显式安装 `xauth`；安装步骤限时 8 分钟，HTTP(S) 无响应限时 30 秒。下载异常会失败而非消耗整个任务时限，不能当作平台检查已通过。

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
