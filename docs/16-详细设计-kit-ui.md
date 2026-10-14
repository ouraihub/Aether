# 16 详细设计：@ouraihub/aether-ui

> 依赖：接入层命令契约（15 文档）与 `ts-rs` 生成的 DTO。被宿主前端依赖。
> 蓝本：AutoNote `packages/app/src/features/ai/{settings,chat}/*`（设置页、对话面板）、`desktop/src/lib/log/*`（日志面板、前端日志器）；AutoBee `src/lib/components/{ChatPanel,ChatBubble}.svelte`（工具调用展示）。

## 1. 包结构

```text
packages/ui/src/
  ports/            # 纯 TS 端口接口；generated/ 为 ts-rs 输出（不手改）
  tauri/            # 唯一可以 import @tauri-apps/* 的目录：createTauriPorts()
  providers/        # ProvidersSettings、EndpointCard、ModelPickerDialog、SecretInput、ModelSelect
  chat/             # ChatPanel、MessageItem、ToolCallItem、ApprovalCard、SkillPicker、run-session.svelte.ts
  skills/           # SkillsSettings、SkillPreviewDialog、SkillEditor
  mcp/              # McpClientSettings、McpServerSettings
  logs/             # LogPanel、LogSettings、createLogger
  components/ui/    # shadcn-svelte 生成的组件源码（button、dialog、select、field、empty、alert、sonner…），由 CLI 维护
  context.ts  i18n/  markdown.ts  utils.ts（cn、tv）  styles/kit.css  styles/tokens.css
  notify.svelte.ts  KitNotifier.svelte   # createDefaultNotify() 与其 Toaster + AlertDialog
  index.ts          # 根入口：setKitContext、createI18n、renderMarkdown、createDefaultNotify、KitNotifier
  dev/              # 组件试验页（`pnpm -F @ouraihub/aether-ui dev`），不导出
```

```json
"name": "@ouraihub/aether-ui",
"exports": { ".": "...", "./ports": "...", "./tauri": "...", "./providers": "...", "./chat": "...",
             "./skills": "...", "./mcp": "...", "./logs": "...", "./styles.css": "...", "./tokens.css": "..." }
```

- 以源码发布，宿主 Vite 直接编译。一个包、子路径导出，不为每个组件建包。
- ESLint：除 `src/tauri/**` 外禁止 import `@tauri-apps/*`；`svelte/no-at-html-tags` 开启。
- **UI 基础组件用 shadcn-svelte**（仓库 `.claude/skills/shadcn-svelte`，开发 UI 前必读）：`packages/ui/components.json` 配置别名，组件经 `pnpm dlx shadcn-svelte@latest add <name>` 加入 `src/components/ui/`，不手写替代品。遵守其规则：语义色（`bg-primary`、`text-muted-foreground`），`cn()` 合并 class，`flex gap-*` 不用 `space-*`，表单用 `Field.*`，空状态用 `Empty`，提示用 `Alert`，toast 用 `svelte-sonner`，Dialog/Sheet 必须有 Title，覆盖层组件不手写 `z-index`。
- `src/components/ui/` 是 CLI 生成代码：允许按 shadcn-svelte 的 `update` 流程升级，修改须保持最小并在文件头注释原因。

## 2. 端口

```ts
export interface CallOptions { signal?: AbortSignal }
export interface ProvidersPort { list(); save(p: ProviderSave); remove(id); modelOptions(f?: ModelFilter); probe(t: ProbeTarget, o?: CallOptions) }
export interface ChatPort {
  run(req: RunRequest, on: (e: RunEvent) => void, o?: CallOptions): Promise<{ runId: string; conversationId: string | null }>
  approve(runId: string, callId: string, allow: boolean): Promise<void>
  history?: { list(scope?: string); get(id: string); remove(id: string); rename(id: string, title: string) }  // 无 store 时缺省
}
export interface KitPorts { logs?: LogsPort; providers?: ProvidersPort; chat?: ChatPort; skills?: SkillsPort; mcpClient?: McpClientPort; mcpServer?: McpServerPort }  // logs 缺省：宿主没有本地日志文件（如浏览器）
```

- 签名中的 DTO 均来自 `ports/generated`。`SkillsPort`、`McpClientPort`、`McpServerPort`、`LogsPort` 与 15 文档 §5 命令一一对应。
- `ports` 同时导出 `createFakePorts({ <port>?: Partial<…>, omit? })` 与 `KitClientError`/`isKitClientError`。
- 端口只声明 DTO 已由 ts-rs 生成的方法。`SkillsPort.preview(source)/install(source, confirmedDigest, replace)` 用 `Source`（`{ path }` 或 `{ bytes: { fileName, bytes } }`，SK3-3）。`ChatPort.history` 用 `ConversationSummary`/`Conversation`/`Turn`/`TurnStatus`（CH2-4，`remove` 返回 `void`）。`McpServerPort.scopes()/setScopes(ids)` 都返回 `ScopeState[]`（`{ id, labelKey, destructive, enabled }`，MS5-2）。`LogsPort.getLevel/setLevel` 用 `LogStatus { level: LogLevel, available }`（U1-2）。`mcp_client_refresh` 没有取消命令，`refresh` 不接收 `signal`。
  - `createTauriPorts({ providers: true, chat: true, chatHistory: true, ... })` 只生成宿主声明的端口（`chatHistory` 对应后端传了 store 且授予 `aether:chat-history`）；组件在端口缺失时不渲染对应区域，不报错。
- 失败抛 `KitClientError { code, message, details }`；`signal` 中止时调用对应 cancel 命令并抛 `code='cancelled'`。
- `ChatPort.run` 的 Promise 在运行**被接受**时 resolve；运行结果看 `end` 事件。

## 3. 上下文与主题

```ts
interface KitContext {
  ports: KitPorts
  i18n: { t(key: string, params?: Record<string, string | number>): string; locale: string }
  notify: { toast(kind: 'info' | 'success' | 'error', text: string): void; confirm(o: { title: string; body: string; danger?: boolean }): Promise<boolean> }
  logger: KitLogger
}
setKitContext(ctx)  // 宿主根组件调用一次；未自带通知实现的宿主用 createDefaultNotify()（svelte-sonner + shadcn AlertDialog）
```

- aether 不读写宿主偏好；需要持久化的选择（默认模型、上次选的技能）通过组件 props/事件交给应用。
- **主题就是 shadcn-svelte 的 CSS 变量**（`--background`、`--foreground`、`--card`、`--muted-foreground`、`--primary`、`--destructive`、`--border` 等）。三个已用 shadcn-svelte 的宿主无需额外配置；未用的宿主引入 `@ouraihub/aether-ui/styles.css`（这组变量的默认值，亮/暗两套，中性色带少量 265° 色相）。aether 不定义自己的颜色变量。
- **尺寸令牌**（`styles/tokens.css`，`styles.css` 已包含；自带 Tailwind 入口的宿主在 `@import 'tailwindcss'` 之后引入 `@ouraihub/aether-ui/tokens.css` 并 `@source` aether 的 `src`）。默认值取自 AutoNote，宿主在自己的 `:root` 覆盖：

  | 变量 | 默认 | Tailwind 工具类 | 用途 |
  | --- | --- | --- | --- |
  | `--kit-text-xs` | 11.5px | `text-kit-xs` | 元信息、分组标题 |
  | `--kit-text-sm` | 12px | `text-kit-sm` | 次要文字、说明、状态行 |
  | `--kit-text-base` | 13px | `text-kit-base` | 正文、控件文字 |
  | `--kit-text-lg` | 13.5px | `text-kit-lg` | 弹窗标题 |
  | `--kit-control-h` | 30px | `h-kit-control` / `size-kit-control` | 按钮、输入框、下拉 |
  | `--kit-control-h-sm` | 28px | `h-kit-control-sm` / `size-kit-control-sm` | `size="sm"` 控件 |
  | `--kit-radius` | 8px | `rounded-kit` | 控件、卡片、气泡；`--radius` 默认也取它 |
  | `--kit-radius-sm` | 6px | `rounded-kit-sm` | 小控件、列表项、标签 |

  字重：正文 400，标签与按钮 500，区块标题 600。暗色变体同时匹配 `.dark` 与 `[data-theme="dark"]` 祖先。组件内 `cn`/`tv` 用 `utils.ts` 中扩展过的合并配置，宿主传入的 `text-*`/`h-*`/`rounded-*` 能正确覆盖令牌类。
- **弹窗层级**：aether 的对话框用 shadcn Dialog，渲染到 body（portal），不手写 `z-index`。宿主若自己的设置面板也是 shadcn Dialog，后打开的天然在上层；宿主若用非 shadcn 的高 `z-index` 容器，由宿主负责不把 aether 弹窗嵌进该容器之下（陷阱 1）。

## 4. 组件

### 4.1 providers

- `ProvidersSettings`：左栏服务商（状态点：有密钥有模型=绿、缺密钥或密钥不可读=黄、无模型=灰）；右栏名称 + 线路卡（同时只展开一张）；地址下方 `→ GET <probeUrl>`；`SecretInput` 只显示打码值，有"替换""清除"两个显式操作；"获取模型"打开 `ModelPickerDialog`（按 group 分组、全选、改类型）；未保存改动提示条。**不含角色区。**
- `ModelSelect`：`<ModelSelect value={ModelRef|null} filter={{ modelType: 'chat' }} onchange={...} placeholder? />`。应用用它做自己的"对话模型""写作模型"设置，值由应用保存；模型已被删除时显示"已失效，请重新选择"。
- 获取模型失败：按 `ProbeError.kind` 翻译 + `hint` + 上游原文。

### 4.2 chat

```ts
interface ChatPanelProps {
  purpose?: string                                  // 默认 'chat'，原样放进 RunRequest
  model?: ModelRef | null                           // 应用给的默认模型；面板内可临时切换
  scope?: string
  context?: { kind: string; id: string; label: string } | null   // 显示为可移除标签
  skills?: SkillRef[]                               // 受控：当前选中的技能
  onSkillsChange?: (refs: SkillRef[]) => void       // 应用决定是否记住
  onInsert?: (markdown: string) => void             // 有它才显示"插入"；运行中禁用
  onSaveNote?: (markdown: string) => Promise<void> | void // 宿主真实创建笔记，无回调不显示
  onExportMarkdown?: (markdown: string) => Promise<void> | void // 桌面宿主原生保存，无回调用Web Blob下载
  onToolPayload?: (tool: string, payload: unknown) => void
  onOpenSettings?: () => void                       // 没有任何对话模型时显示"打开设置"
  conversationId?: string | null                    // 挂载时恢复的会话；已不存在（not_found）则新对话
  onConversationChange?: (id: string | null) => void  // 显示的会话变化时调用（新运行建会话、打开历史、删除、新对话），应用据此持久化
  onClose?: () => void
}
```

- 历史下拉（有 `history` 端口时）、新对话、删除当前会话（`notify.confirm` 确认后 `history.remove`，再回到新对话）、上下文标签（× 本轮不带）、Markdown 渲染（`{@attach}` + 已清洗 HTML）、每轮复制（剪贴板，短暂显示"已复制"）/ 插入、最后一轮重新生成、停止、失败原因与重试（= 重新生成最后一轮）、空状态（无模型 → 提示 + "打开设置"；空会话 → 简短提示）、窄屏全屏 + Esc 关闭（有 `onClose`、焦点在面板内且没有运行时）。
- 2026-10-14批准的Voynote参考改造：紧凑两行顶部，一体输入卡片（Enter换行、Ctrl/Cmd+Enter发送、IME期间不发送、展开保留草稿），AI回复卡片与框架独立正文排版；末轮重新生成／重试在回复内。消息区单独滚动，向上阅读暂停跟随。历史／新建／删除／重命名复用现有端口；加载失败与无模型分开。首批不新增消息树／附件DTO或无后端按钮。输入未接纳失败可恢复，终态保留部分输出。通用局部CSS在完整框架入口与宿主Tailwind入口均有效，不覆盖宿主主题。
- 上下文标签移除仅代表本轮context=null；scope仍为会话归属。宿主必须继续校验会话和关联资源访问权限，不能以无payload免鉴权；若非空context仍需匹配scope。写作必填由宿主继续执行。
- 实施证据：完整UI lint/格式/typecheck（0错误0警告）与11文件68项通过，i18n337键、DTO64文件、依赖13crate一致。真实Chromium→AutoNote HTTP端口→Hono/SQLite→HTTPS回环SSE验Enter/CtrlEnter、思考high出站参数、移除context且历史保留、重命名HTTP读回、Esc只关会话Dialog、长流向上阅读scrollTop保持0；320/375/414/768容器无横向溢出。独立styles.css入口与宿主Tailwind入口均读到有效正文背景／边框；静态运行期语义变量防止inline主题被裁剪。付费供应商与原生手机软键盘不在此回环证明中。
- 用户复核后批准完整回复操作：展开按钮固定输入卡片右上角；回复左侧复制／更多／重答／换模型／翻译／删除／保存笔记，右侧用作上下文。候选共享questionId只显示一次问题，来源模型和选中态可读；selectTurn持久选择同会话路径，“分支”也选择同会话路径并清空草稿，fork仅“复制为新对话”。更多编辑CAS、Markdown下载均有真实行为；编辑／语言Dialog错误保草稿，当前选中及有后代回复不可删除。调用稳定turnId；运行中禁止结构变更；End后读回须先完成后端持久化。CH8附件／并发比较尚未授权。
- 思考强度：Gauge／当前档位入口向上右对齐弹层，强度／恢复默认、更快—更智能、原生离散range；只遍历模型声明档位。default独立于显式档位，弹层打开和视觉中位不写medium；拖动／方向键／Home/End后才写合法值；none为显式关闭。选中模型时始终保留入口，无能力时明确说明并提供宿主设置入口，不显示伪造滑块；单显式档位用默认＋选项，切模型恢复default，运行中禁用，Esc先关弹层回焦点。复用RunRequest.reasoningEffort，不引入serviceTier/FastMode、模型目录或偏好存储。
- ProvidersSettings保存／删除成功后通过可选onchanged回调通知宿主；宿主刷新角色选择和密钥线路，不因旧的初次加载快照把刚保存的模型继续显示为不可选。
- `SkillPicker`：输入框上方的技能选择（列表只含元数据）；显示所选正文合计 / 预算；`end` 或请求错误为 `skill_changed` 时提示"技能已更新，请重新确认"并打开预览。
- 工具：`tool_call` 显示"正在调用 <name>"（MCP 标服务名）；`approval_required` 显示 `ApprovalCard`（工具名、来源、参数预览、"允许本次 / 拒绝"）；`tool_result` 折叠为一行，`unknown` 标"结果未知，可能已执行"，`denied` 标"已拒绝"。
- `end`：`completed` + `exhausted` 时显示"已达到工具调用轮数上限"；`failed` 显示错误与"重试"；`cancelled` 显示"已停止"。
- 一次性生成（如编辑器润色）不用 ChatPanel：应用直接调 `ChatPort.run({ purpose: 'polish', ... })`，自己处理 `text_delta`。

### 4.3 skills

- `SkillsSettings`：列表（名称、版本或"未版本化"、来源、修订、字数）、上移/下移排序、查看全文、编辑（手写）、复制内置、删除。
- 导入：`<input type="file" accept=".md,.zip">` 读出字节，以 `Source.bytes` 发送（不引入 `plugin-dialog`；目录导入只在 Rust `Source::Path` 提供）→ `SkillPreviewDialog` 显示**全文**、`extra` 键 → 确认时带 `digest`；返回 `content_changed` 时用新预览重新确认；同名时显示现有版本并要求勾选"替换"。
- `ChatPanel` 的 `skillsBudget` prop（默认 48 000）对应后端 `Budget.skills_chars`，只用于显示；超出由后端 `budget_exceeded` 拒绝。`SkillPicker` 保留已选项的原修订号，编辑后的技能由 `snapshot` 拒绝为 `skill_changed`，确认新全文后才换成新修订；关闭预览则移出选择。

### 4.4 mcp

- `McpClientSettings`：服务列表（名称、打码 URL、凭据键名、启用、信任、内网授权）；编辑弹窗（headers 与 query 凭据只写不读）；"测试连接"后显示工具列表，每个工具一个 `auto/ask/deny` 下拉（默认 ask，只读工具旁提示"可设为自动"）；首次启用时确认信任；打开内网授权时二次确认。
- `McpServerSettings`：状态、启停、端口、作用域开关（destructive 二次确认）、token 列表与撤销；新建 token 一次性显示并提供"复制 Claude Code 配置""复制通用 JSON 配置"。

### 4.5 logs

`LogPanel`（每 2 秒 `tail(300)`，不可见时停止；只看问题；打开目录）、`LogSettings`（级别即时生效；`available=false` 时提示写入失败）。`createLogger(port)`：本地白名单同 10 文档 §10；队列 200（满时先丢 debug/info）；每 20 条或 250ms 批量上报；`pagehide` 时立即上报。

## 5. i18n

键前缀 `providers.*`、`chat.*`、`skills.*`、`mcp.*`、`logs.*`、`errors.<code>`；`createI18n(locale, overrides?)` 允许应用覆盖文案；`pnpm kit:i18n` 检查两种语言键集合一致且组件中使用的键都存在。

## 6. 测试

- vitest + jsdom + `createFakePorts()`（完整声明全部端口，避免各处手写不完整的假对象）。
- 必测行为：凭据三态与"清除"显式操作；获取模型等待保存；流式、停止、唯一终态渲染；审批允许/拒绝/取消；`skill_changed` 重新确认；技能预览 `content_changed`；端口缺失时区域隐藏。
- 对话框关闭后 DOM 中不残留全屏 fixed 节点（包括 shadcn Dialog 的 overlay）。

## 7. 陷阱

1. 弹窗层级：不手写 `z-index`（shadcn 规则）。aether 弹窗一律 portal 到 body；若宿主把 aether 组件放进自定义的高 `z-index` 容器，后果由宿主负责（AutoNote 曾因此出现挑选弹窗被设置弹窗遮住）。
2. 关闭弹窗时整个遮罩层卸载（不只是隐藏内容），否则透明全屏层会挡住整个窗口的点击。不要在 shadcn Dialog 外再包一层常驻的 fixed 容器。
3. 保存服务商时保留线路 `id`。
4. 运行中切换会话或关闭面板：先 abort 当前运行（触发 `chat_cancel`），再切换。
