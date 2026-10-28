# 01 多产品基座与 Web 运行时演进规划：Aether → AutoNote / Actora

> 状态：共享 TS contracts/core/worker 和 Web 客户端已实现；真实 workerd HTTPS 模型/工具/审批/隔离/存储/取消及 OAuth MCP 冒烟已执行。完整 CI、浏览器与重启验收的当前证据见90，产品迁移不在本次交付范围。
> 目标：明确 Aether 是唯一共享基座；AutoNote 是笔记产品；Actora 是工作台产品。两者当前都可能拥有 Web Worker 版本，未来都可能拥有桌面版本。
> 关系：本文件补充 `docs/00-概要设计.md` 的组合边界；若实施中发现契约冲突，先修订本文与 00，再改代码。
> 本页保留多产品组合与迁移路线；当前统一架构入口见00、分层图见03，TS Worker实际组合/API见23。Rust/TS分别实现相同合同，不整体Rust/WASM化。

## 1. 结论

产品关系固定为：

```text
                         ┌─────────────────────────┐
                         │ Aether 共享 AI 基座      │
                         │ contracts / core / UI    │
                         │ Worker adapter / Desktop │
                         └────────────┬────────────┘
                                      │
                   ┌──────────────────┴──────────────────┐
                   │                                     │
          ┌────────▼────────┐                   ┌────────▼────────┐
          │ AutoNote         │                   │ Actora           │
          │ 笔记应用         │                   │ 工作台应用       │
          └───────┬──────────┘                   └───────┬─────────┘
                  │                                      │
          Web Worker / Desktop                    Web Worker / Desktop
```

- **Aether** 是模型、AI 对话、MCP、搜索、权限、凭据和运行时语义的共享基座，不拥有笔记或工作台业务。
- **AutoNote** 是基于 Aether 的笔记应用，包含 Web Worker 版和桌面版；其笔记、文件、版本、同步、分享是产品业务。
- **Actora** 是基于 Aether 的工作台应用，当前先实现 Web Worker 版，未来可实现桌面版；其应用注册、命令、资源、prompt、操作、审批和工作台业务是产品业务。
- AutoNote 与 Actora **不互相依赖**。AutoNote Web 不依赖 Actora Worker；Actora 不作为公共基座。
- Aether 已提供 Web Worker 适配包和桌面/Tauri 适配层。应用依赖公共 contracts/core/UI 与对应运行时端口，不互相依赖产品后端。

## 2. 问题与现状

当前 Aether 已有桌面核心与 Tauri/UI 基座：

```text
crates/aether-*                 Rust 核心能力
plugin/tauri-plugin-aether      Tauri 组合与命令
packages/ui                     共享 Svelte UI、ports、DTO
```

AutoNote Web 与 Actora Framework 又分别形成了部分 Worker 基座实现，存在同语义重复：

| 重复能力                   | AutoNote 当前位置                            | Actora 当前位置                                                   | 规划归属                           |
| -------------------------- | -------------------------------------------- | ----------------------------------------------------------------- | ---------------------------------- |
| 模型供应商/线路/模型能力   | `web/src/worker/ai/providers.ts`、`infer.ts` | `packages/framework/src/worker/services/providers.ts`、`infer.ts` | Aether Web/Core                    |
| 三协议请求、SSE、reasoning | `web/src/worker/ai/protocol.ts`、`runs.ts`   | `packages/framework/src/worker/services/outbound.ts`、`kit.ts`    | Aether Core/Worker                 |
| AI 对话运行与工具循环      | `web/src/worker/ai/runs.ts`                  | `packages/framework/src/worker/services/kit.ts` 等                | Aether Core                        |
| 网络搜索/网页读取          | `web/src/worker/ai/search.ts`                | `packages/framework/src/worker/services/search.ts`                | Aether Web/Core                    |
| MCP OAuth/API key/scope    | `web/src/worker/mcp/*`                       | `packages/framework/src/worker/mcp/*`                             | Aether Web                         |
| MCP 客户端协议             | Aether 依赖与 AutoNote Desktop               | Aether `aether-mcp-client`；Actora Web 当前不启动 stdio           | Aether Core；运行时按能力启用      |
| MCP 客户端片段             | AutoNote 消费 Aether UI 旧列表               | Actora `McpSettings.svelte` 已有 Pi/omp                           | Aether UI 纯函数/组件              |
| 图片生成/编辑              | AutoNote `painting.ts`、`images.ts`          | 基座规划项                                                        | Aether Core/Worker，存储由宿主适配 |

这不是要求把两套数据库或业务路由合并；是把“相同的基座语义和服务”提炼到 Aether，消除双重实现。

## 3. Aether 目标分层

### 3.1 Contracts

Web 与桌面共同使用的公共契约：

- `ModelRef`、`Provider`、`Endpoint`、`Protocol`、`ModelType`；
- `ReasoningConfig`、`ReasoningEffort`、模型能力与价格；
- `RunRequest`、`RunAccepted`、`RunEvent`、`ToolCall`、`ToolSpec`；
- `Approval`、`PermissionMode`、`ToolEffect`、`ToolSource`；
- `McpScope`、MCP grant/API key/server/tool 视图；
- `SearchHit`、搜索读取结果；
- `KitError`、错误码、详情；
- 用量、成本、会话/操作终态和取消语义。

Rust `ts-rs` DTO 统一输出到 `packages/contracts/src/generated`，`@ouraihub/aether-contracts` 导出公共类型及预编译校验；UI ports、Core、Worker共同消费，不维护第二套近似DTO。后端 ToolCall/ToolSpec 等执行类型仍归 Core，不冒充所有内部类型都是公共wire。

### 3.2 Core

不依赖 Worker、Tauri、D1、SQLite、浏览器或产品业务：

- 三协议请求/流式解析；
- reasoning 与模型能力映射；
- 工具目录、调用 ID 去重、审批状态和取消语义；
- MCP 工具协议、scope 判断、OAuth/API key 语义的纯部分；
- 搜索结果规范化、URL/地址策略的纯校验；
- 用量/成本计算；
- 错误映射和失败/未知终态；
- 客户端配置片段生成；
- 运行时端口接口。

Core 不读取业务数据库，不知道 `note`、`task`、`repo`、`folder` 等应用实体。

### 3.3 UI

`@ouraihub/aether-ui` 提供：

- ChatPanel、消息、工具调用、审批卡片；
- Model/Provider/Reasoning 设置组件；
- Search 设置与结果显示；
- MCP 客户端连接管理组件；
- MCP 外部客户端配置片段生成器；
- skills、logs、通用错误和状态组件。

组件只消费明确 ports，不调用 Worker、D1、Tauri 或业务数据库。

### 3.4 Worker Adapter

已实现 `@ouraihub/aether-worker`，负责把 Core/Contracts 接到 Cloudflare：

- D1 JSON文档CAS、每run一个DO、R2不可变对象；KV/Queues不是当前运行链路的必需实现；
- Worker `fetch`、HTTP 错误与 JSON/SSE；
- OAuth provider、MCP endpoint、API key hash/grant 持久化；
- Worker 凭据加密；
- 网络 `fetch`、超时和地址策略；
- 会话、对话历史、图片对象存储适配器；
- 宿主注册业务工具和资源。

Worker Adapter 不引入 AutoNote 或 Actora 类型。

### 3.5 Desktop Adapter

保留现有 Rust/Tauri 路线：

- SQLite、原生凭据（DPAPI/Keychain/Secret Service）；
- Tauri IPC/Channel；
- 本地MCP stdio、本机文件等由独立能力及宿主授权提供；Skills自身只管理包，不执行脚本。
- 无UI运行服务负责组合，`tauri-plugin-aether`仅做接入；CLI/未来原生UI可直接消费服务。
- 应用KitHost注入模型、上下文、工具和授权，完整合同见25。

未来 Actora Desktop 复用该适配器与 Aether UI，但不把 Actora 工作台状态放进 Aether。

## 4. 业务应用边界

### 4.1 AutoNote

Aether 只提供能力；AutoNote 注册：

- `notes:list/read/create/update/trash/restore` 等笔记工具；
- 笔记上下文、文件夹、标签、版本、分享；
- 笔记资源授权与scope，使用 Aether 通用会话/分支存储服务而不迁入笔记业务表；
- 图片/附件与同步/备份；
- AutoNote 特有的 Web/Desktop 路由和设置。

### 4.2 Actora

Aether 只提供能力；Actora 注册：

- 应用清单、命令、资源、prompt；
- 工作台 Shell、标签、应用生命周期；
- 操作、审批、恢复与审计；
- AutoFlow/GitHub/渠道等具体应用业务；
- 工作台业务 MCP 工具。

Actora 的 `packages/framework/src/worker` 当前实现是**提炼来源和迁移目标候选**，不是新的共享基座。通用部分回到 Aether，Actora 保留工作台服务。

### 4.3 明确不共享

不把以下内容硬合并：

- AutoNote notes/folders/tags/versions/attachments/share/sync/backup；
- Actora apps/commands/resources/prompts/operations/approvals/GitHub/Actions/channels；
- AutoNote 与 Actora 的 D1 表、迁移、租户/主体模型；
- AutoNote 桌面本地 MCP server 与 Actora Worker 远程 MCP endpoint 的生命周期；
- Worker D1/R2/KV 和 Desktop SQLite/原生存储的实现；
- 应用业务 scope。Aether 只定义授权语义，业务 scope 由应用注册。

## 5. MCP 统一设计

MCP 分为两个方向，不能混成一个组件：

### 5.1 MCP Server：应用对外提供工具

Aether 公共能力：

- Streamable HTTP/RPC；
- OAuth 2.1 PKCE、protected resource metadata、动态注册；
- API key（仅 hash 存储、一次显示、撤销）；
- grant、scope、限流、Origin/地址策略；
- 工具目录、schema、调用边界、错误和审计端口；
- Pi/omp/Codex/Claude/Hermes/OpenClaw 等连接片段。

AutoNote 注册笔记工具；Actora 注册工作台/搜索/应用工具。

### 5.2 MCP Client：应用内 AI 连接外部 MCP

Aether Core/Desktop：

- HTTP/stdio transport；
- discovery、trust、tool approval；
- credential filtering；
- process group/Job Object 生命周期；
- cancellation、timeout、unknown outcome。

Worker 版不启动本机 stdio；如需要外部进程，使用明确的外部执行器，不把 shell/stdio 搬入请求处理器。

## 6. 迁移路线

### Phase A：冻结边界

1. 将本文纳入 Aether 架构文档和任务清单。
2. 已落地 `packages/contracts`/`packages/core`/`packages/worker` 与 `examples/worker`；不复制 AutoNote 业务。
3. 列出 AutoNote Web 与 Actora Worker 的重复函数、类型、测试和迁移依赖。
4. 选 Aether contracts 作为唯一公共语义来源；未迁移前明确标注临时适配层。

### Phase B：先迁纯逻辑

从 AutoNote 与 Actora 取较成熟实现，合并到 Aether：

1. protocol/SSE/三协议请求构造；
2. model inference/reasoning；
3. search result normalization；
4. MCP scope/key/grant 纯逻辑；
5. error/usage/cost；
6. MCP client snippets（包含 Pi/omp）。

每项先以公共测试固定行为，再删除两边重复实现。

### Phase C：建立 Worker Adapter

1. Aether Worker 已实现 D1/DO/R2/credential/outbound/identity 适配；运行状态及安全合同见23/29。
2. 通用服务属于Aether；Actora工作台与AutoNote笔记服务仍留产品。本仓实现不代表两产品重复代码已迁完。
3. AutoNote Web 实现 Aether Worker ports，并逐功能切换。
4. AutoNote notes/attachments等业务通过应用适配器注入；通用history已由Aether提供，旧产品历史的数据迁移仍归产品，不声称本仓已迁完。

### Phase D：UI 统一

1. Aether UI 提供 Model/Search/Chat/MCP/Approval 组件。
2. 将 Actora `clientSnippets()` 提炼为 Aether UI 的纯函数；AutoNote 和 Actora 都消费。
3. AutoNote 的 `McpClientSection.svelte` 保持薄包装；Actora 的 `McpSettings.svelte` 保留 Worker grant/key 设置，但使用共享客户端片段组件。
4. 删除第二份客户端列表和模板；补双宿主组件测试。

### Phase E：桌面对齐

1. AutoNote Desktop 继续使用 Aether Rust/Tauri，不等待 Worker 迁移完成。
2. 为 Aether Desktop contracts/core 补齐与 Worker 等价的端口语义。
3. 未来 Actora Desktop 复用 Aether Tauri/UI 与本地 adapter，只注册工作台业务。
4. 桌面独有的 SQLite/原生凭据/stdio/skill 执行不进入 Worker 包。

## 7. 兼容和迁移规则

- 不给 AutoNote/Actora 保留两套长期协议别名；迁移完成后删除旧 Worker 实现。
- 数据库迁移在应用仓库完成；Aether 不读取 AutoNote/Actora 旧表。
- Web API 变更采用版本化 contracts，先迁宿主再删除旧实现；不以 `any`/类型断言维持假兼容。
- Aether Worker 和 Desktop 的同一语义必须有跨运行时契约测试；实际存储/网络测试各在对应运行时执行。
- Aether 不能因 AutoNote 需求引入产品名、目录探测或业务特判。

## 8. 验收标准

### 基座

- Aether contracts/core/worker 类型检查与测试通过；无 AutoNote/Actora 导入。
- 三协议流、reasoning、tool approval/cancel、MCP scope/grant/key、search normalization 由公共测试覆盖。
- MCP client snippet 只有一份实现，Pi/omp/Codex/Claude/Hermes/OpenClaw/generic 均有测试。

### AutoNote

- Web 的模型、搜索、AI 对话、MCP OAuth/API key 和工具循环来自 Aether Worker。
- AutoNote 只保留笔记/文件/版本/分享/同步/备份业务。
- Desktop 仍通过 Aether Tauri；Web/Desktop 的公共请求/事件/错误契约一致。
- 现有笔记、历史、图片、附件、MCP 访问和权限回归通过。

### Actora

- Worker 的通用 provider/search/chat/MCP 代码不再重复维护。
- Actora 保留工作台应用注册、commands/resources/prompts/operations/approvals/GitHub 业务。
- Pi/omp 及其他 MCP 客户端片段与 AutoNote 同源。
- 未来 Desktop 只需实现 Aether Desktop adapter，不复制 Worker 业务基座。

### 真实验收

- AutoNote Web：真实模型流、搜索、MCP endpoint/API key、笔记工具调用。
- Actora Web：真实模型流、搜索、MCP endpoint OAuth/API key、工作台工具调用。
- AutoNote Desktop：真实 SQLite/原生凭据/MCP stdio/skills。
- Actora Desktop：实施时补同等本地能力验收；Web 不声明拥有本机进程能力。

## 9. 未决决策

1. **包形态已定**：`@ouraihub/aether-contracts`、`@ouraihub/aether-core`、`@ouraihub/aether-worker` 为当前 private workspace 包；公开发布另走版本与消费者迁移门禁。
2. **公共契约来源已定**：语言无关schema/行为场景为共同规范，Rust保留ts-rs绑定到 `packages/contracts/src/generated`；TS包index及CSP安全预编译校验由contracts脚本生成，Rust/TS执行消费共同场景。
3. **对话历史已进入Aether Worker**：公共会话图、预留、分支与D1迁移归Worker包，产品业务表/迁移仍归应用；application/tenant/subject强隔离，不读取产品旧表。
4. **MCP Server 与 MCP Client 的 UI 是否同页**：推荐共享基础组件和客户端片段，不共享完整设置页面；两个方向的授权对象不同。
5. **Actora Desktop 的执行边界**：推荐复用 Aether Tauri 与本地 runner；在 Desktop 设计启动前单独确认本地文件、skills、stdio 和工作台命令的权限矩阵。

## 10. 第一批实施任务建议

| 编号 | 任务                          | 产出                                                 | 不做                      |
| ---- | ----------------------------- | ---------------------------------------------------- | ------------------------- |
| B0   | 公共基座边界冻结              | 本文、00 概要设计增补、依赖图                        | 不改 AutoNote/Actora 代码 |
| B1   | Web contracts 抽取            | Aether Worker contracts、错误/事件/权限测试          | 不迁业务表                |
| B2   | 纯逻辑迁移                    | protocol/infer/search/MCP/client snippets            | 不接 D1/R2                |
| B3   | Aether Worker provider/search | ports + D1/credential/http adapters                  | 不迁 AutoNote routes      |
| B4   | Aether Worker chat            | run/history/image storage ports                      | 不迁笔记业务              |
| B5   | Aether Worker MCP             | OAuth/API key/scope/RPC/registration                 | 不注册产品工具            |
| B6   | AutoNote Web 适配             | 删除重复 worker AI/MCP，保留 note routes             | 不改 Desktop              |
| B7   | Actora Web 适配               | 删除重复 framework services，保留 workspace/commands | 不接 Desktop              |
| B8   | Aether UI Pi/omp              | 共享片段/组件/测试                                   | 不让 UI 管 token          |
| B9   | 双宿主真实验收                | AutoNote/Actora Web + MCP 客户端                     | 不把单元测试冒充 E2E      |

执行顺序：`B0 → B1 → B2 → B3/B4/B5 → B6/B7 → B8/B9`。桌面适配和业务迁移分别按对应产品节奏进行，不阻塞公共 Web contracts 先落地。
