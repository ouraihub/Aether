# 14 详细设计：aether-skills

> 依赖：aether-support。**不依赖** MCP、Chat。没选技能时，Chat 的行为与没装 Skills 完全一样。
> 蓝本：AutoBee `shell/src/llm/skills.rs`（内存解析 ZIP、白名单、大小上限、凭据检测）；voynote `src-tauri/src/skill.rs`（目录导入、手写技能）。不保留它们的包格式与 MCP 绑定。

## 1. 边界

- Skill = 用户可查看、选择、版本化的**领域指引正文**。它改变模型"怎么做"，不赋予任何工具、网络、文件或进程权限。
- 管：导入（单个 `SKILL.md` / 目录 / ZIP）、预览与确认、手写与编辑、修订号、删除、排序、宿主内置技能、按次选择的快照；保存包内的脚本、参考文档与资源文件，并按 `id + revision` 提供读取（§6 `files`/`file`）。
- 不管：**执行脚本**、安装钩子、下载依赖、把包内文件写到磁盘、读任意文件、模型自主加载其他技能（D5）、**登记或管理 MCP 服务**。包内文件只是数据：是否、何时、以何种方式运行脚本由宿主决定并负责审批，本 crate 只交出字节。技能正文可以写"需要地图工具"，实际服务由用户在 MCP 设置里配置。
- 没有"最多启用 3 个"之类的全局开关：技能按**每次运行**选择，受预算约束（12 文档 §2.1）。

## 2. 包格式

```text
example-skill/        # 或 example-skill.zip（根目录或唯一顶层目录），或单个 SKILL.md
├── SKILL.md          # 必需；frontmatter 规则见 §2.1
├── LICENSE           # 可选（LICENSE / LICENSE.md / LICENSE.txt；内容保存，展示用）
├── scripts/**        # 可选；脚本，UTF-8 文本（用法由 SKILL.md 正文说明）
├── references/**     # 可选；参考文档，UTF-8 文本
└── assets/**         # 可选；任意文件（可二进制）
```

- 采用开放的 Agent Skills 目录约定；frontmatter **不**列出脚本，文件归属只由所在目录决定（`scripts/` → `script`、`references/` → `reference`、`assets/` → `asset`）。
- 三个子目录可以嵌套，文件最多位于 4 层目录内（`scripts/a/b/c/x.py` 合法，`scripts/a/b/c/d/x.py` 超深）。每个路径段只允许 `[A-Za-z0-9._-]` 且不得以 `.` 开头（排除 `.git`、`.DS_Store` 等隐藏项）。

```markdown
---
name: travel-planner
description: 帮用户规划公共交通出行
version: 1.2.0
---
（正文）
```

### 2.1 受限 frontmatter 解析器（自写）

- 首行必须是 `---`，到下一个独占一行的 `---` 结束；文件开头的 BOM 去掉；`\r\n` 统一为 `\n`。
- 每行只能是 `key: value`；key 匹配 `^[a-z][a-z0-9_-]{0,31}$`；value 为纯标量，或单/双引号字符串（双引号内只支持 `\"` 与 `\\` 转义）。
- 拒绝：重复键、缩进行（嵌套）、`- ` 列表、`{`/`[` 流式集合、`&`/`*` 锚点与别名、`!` 标签、`|`/`>` 块标量、多文档、frontmatter > 4 KiB。
- 已知键：`name`（必需，`^[a-z0-9][a-z0-9-]{0,63}$`）、`description`（必需，1~500 字符）、`version`（可选，≤ 32 字符；缺省为 `null`，表示"未版本化"，不编造版本）。其他合法键保留在 `extra` 中原样展示，不解释。
- 这不是 YAML 的完整实现，只接受上述子集；不在子集内的一律 `bad_request{reason:"frontmatter_unsupported", field:"<行号>"}`。

### 2.2 限制（读取过程中强制执行，不相信压缩包声明的长度）

| 项 | 上限 |
| --- | --- |
| ZIP 文件 | 5 MiB |
| 条目数（ZIP 条目；目录导入时为遍历到的文件与目录） | 200 |
| 累计解压/读取字节 | 16 MiB |
| 单个 `scripts/`、`references/`、`assets/` 文件 | 1 MiB |
| `SKILL.md` | 64 KiB（正文 ≥ 1 字符） |
| `LICENSE` | 32 KiB |
| 目录层级 | 4 |

- 接受的文件：`SKILL.md`、`LICENSE`、`LICENSE.md`、`LICENSE.txt`，以及 `scripts/**`、`references/**`、`assets/**` 下的文件（§2）。其他任何路径 → `bad_request{reason:"unsupported_files"}`，并列出路径（目录带尾部 `/`）。
- ZIP 中拒绝：绝对路径、`..`、反斜杠路径、规范化后重复的路径、符号链接条目、加密条目。
- 目录导入：递归读取上述三个子目录，其他子目录列为不支持；遇到符号链接或 junction 一律拒绝，不跟随。
- 全程在内存中处理，不解压到磁盘。每个文件按 `min(自身上限, 剩余累计额度) + 1` 字节截断读取，单文件超限与累计超限都不依赖 ZIP 头里的长度。
- `scripts/`、`references/` 必须是合法 UTF-8；`assets/` 可为任意字节。
- 疑似凭据（`looks_like_secret`）→ 拒绝，并给出位置（`<路径>:<行号>`，如 `SKILL.md:6`、`scripts/run.py:2`），原文不进日志、不进错误消息。所有文本文件都检测：`SKILL.md`、许可文件、脚本、参考文档，以及能按 UTF-8 解码的资源文件。检测只是辅助，不保证发现所有秘密。
- 错误（均为 `bad_request`）：超限 `reason:"too_large"`，`field` 为 `zip`/`entries`/`total`/`SKILL.md`/许可文件名/文件路径，带 `limit`；ZIP 路径 `field:"zip"` + `absolute_path`/`path_traversal`/`backslash_path`/`duplicate_path`/`link`/`encrypted`/`invalid`；目录中的链接 `field:"<相对路径>"`（导入根本身为 `path`）+ `link`；非法文件名 `field:"<路径>"` + `invalid_name`；超深 `field:"<路径>"` + `too_deep`（带 `limit:4`）；脚本或参考文档非 UTF-8 `field:"<路径>"` + `not_utf8`；凭据 `field:"<路径>:<行号>"` + `secret_detected`；`unsupported_files` 的 `field` 为逗号分隔的路径。

## 3. 数据模型（`skills.db`，migration 1–2）

```sql
CREATE TABLE skills (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,              -- frontmatter name（slug）
  description TEXT NOT NULL,
  version TEXT,                           -- NULL = 未版本化
  body TEXT NOT NULL,
  license TEXT,
  extra TEXT NOT NULL DEFAULT '{}',       -- 未解释的 frontmatter 键
  origin TEXT NOT NULL CHECK (origin IN ('imported','manual','builtin')),
  revision INTEGER NOT NULL,              -- 内容每变一次 +1
  digest TEXT NOT NULL,                   -- 见下；预览与安装用同一算法
  position INTEGER NOT NULL,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
);
```

migration 2：

```sql
CREATE TABLE skill_files (
  skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
  path TEXT NOT NULL,                     -- 包内相对路径，`/` 分隔，如 scripts/run.py
  kind TEXT NOT NULL CHECK (kind IN ('script','reference','asset')),
  size INTEGER NOT NULL,                  -- 字节
  sha256 TEXT NOT NULL,                   -- 小写十六进制
  content BLOB NOT NULL,
  PRIMARY KEY (skill_id, path)
);
```

- `digest` = sha256：`name`、`description`、`version`、`body` 各带长度前缀（避免拼接歧义，`None` 与 `""` 不同）；技能有许可或文件时再追加许可文本与按路径排序的每个 `(path, sha256)`。没有许可与文件的技能（手写、内置、只有 `SKILL.md` 的导入）摘要与 migration 1 相同，升级不改修订号；migration 1 时代带许可的导入技能在下次替换或编辑时摘要会变，修订号 +1 一次。
- 文件随技能写在同一事务：安装/替换时整体替换该技能的文件；任一文件变化（增、删、改字节）都使 `digest` 变化，从而 `revision+1`。删除技能级联删除文件。
- 手写技能（`save_manual` 新建）、内置技能没有文件；编辑导入技能的正文保留原有文件；`duplicate` 连同文件一起复制。
- 迁移 1→2 只新增表，已有技能、修订号、排序不变。

- 安装、编辑、替换都只在 `skills.db` 内一个事务完成，没有跨库操作。
- 修订号随任何内容变化递增；排序变化不改修订号。
- `builtin`：随应用版本发布，`Store::open` 时按宿主传入内容同步（`name` 匹配，内容不同则 `revision+1`；宿主不再提供的内置技能删除；名称已被用户技能占用则跳过并记 `warn`）。不可删除、不可编辑；用户可"复制为自定义技能"，副本 `origin='manual'`、新名称，之后内置更新不影响副本。

## 4. 导入、确认与更新

```mermaid
sequenceDiagram
  participant UI
  participant S as aether-skills
  UI->>S: preview(source)
  S-->>UI: Preview { digest, name, description, version, body, license, files, action: install|replace, current? }
  Note over UI: 用户查看全文后确认
  UI->>S: install(source, confirmedDigest)
  S->>S: 重新读取并完整校验，计算 digest
  alt digest ≠ confirmedDigest
    S-->>UI: conflict{reason:"content_changed"}，UI 重新调用 preview 取新的 Preview，需重新确认
  else 同名已存在且未传 replace=true
    S-->>UI: conflict{reason:"exists"}
  else
    S->>S: 事务写入（替换时 revision+1）
    S-->>UI: SkillDto[]
  end
```

- "重新校验合法"不等于"用户批准过新内容"：安装必须带预览时的 `digest`，不一致就要重新确认。
- 同名替换必须显式 `replace=true`；`Preview.current` 给出现有版本与修订，供 UI 展示差异。版本号只用于提示，不自动覆盖用户编辑过的内容。
- `builtin` 同名的外部包 → `conflict{reason:"builtin_name"}`。
- 预览对话框列出包内全部文件（路径、类型、大小）；含 `scripts/` 时显示提示“包含脚本，本应用不会自动执行；宿主可能在你批准后运行”。列表项对含脚本的技能显示“含脚本”标记（`SkillMeta.hasScripts`）。

## 5. 选择与运行快照

```rust
pub use aether_support::SkillRef; // { pub id: String, pub revision: u32 }：定义在 aether-support，接入层在未启用 skills feature 时也要反序列化 RunRequest.skills
pub struct SkillSnapshot { pub id: String, pub revision: u32, pub name: String, pub body: String }
impl Store {
    /// 运行开始时由接入层调用。任一项修订号不符 → conflict{reason:"skill_changed", entity:id}，UI 刷新后让用户重新确认。
    pub fn snapshot(&self, refs: &[SkillRef]) -> Result<Vec<SkillSnapshot>, KitError>;
}
```

- UI 列表只拿元数据（不含正文）；正文在用户打开查看或选中时才加载。
- 选择是**每次运行**的参数，由 UI 传入 `id + revision`；aether 不存"当前启用了哪些"。应用若要记住用户上次的选择，自己存 `SkillRef` 列表；修订变化后旧引用会被 `snapshot` 拒绝，从而强制重新确认。
- 快照在运行开始时固定；运行中的编辑、排序、删除不影响本次运行。
- 预算检查在 Chat `validate` 中做（所选正文合计 ≤ `skills_chars`），超出直接报错，不截断。
- 会话存储只记录 `{id, revision}`，不复制正文；技能被删除不影响历史消息。

## 6. API 与 DTO

```rust
pub struct BuiltinSkill { pub name: &'static str, pub description: &'static str, pub version: Option<&'static str>, pub body: &'static str }
pub struct Store { /* Mutex<Connection> */ }
impl Store {
    pub fn open(root: &Path, builtin: &'static [BuiltinSkill]) -> Result<Self, KitError>;
    pub fn list(&self) -> Result<Vec<SkillMeta>, KitError>;
    pub fn get(&self, id: &str) -> Result<SkillDto, KitError>;                         // 含 files 清单，不含文件内容
    pub fn files(&self, id: &str, revision: u32) -> Result<Vec<SkillFileMeta>, KitError>;
    pub fn file(&self, id: &str, revision: u32, path: &str) -> Result<Vec<u8>, KitError>;
    pub fn preview(&self, source: &Source) -> Result<Preview, KitError>;
    pub fn install(&self, source: &Source, confirmed_digest: &str, replace: bool) -> Result<Vec<SkillMeta>, KitError>;
    pub fn save_manual(&self, input: &ManualSave) -> Result<Vec<SkillMeta>, KitError>;   // 新建或编辑（带 expectedRevision）
    pub fn duplicate(&self, id: &str, new_name: &str) -> Result<Vec<SkillMeta>, KitError>; // 连同文件复制
    pub fn reorder(&self, ids: &[String]) -> Result<Vec<SkillMeta>, KitError>;
    pub fn delete(&self, id: &str) -> Result<Vec<SkillMeta>, KitError>;                 // builtin → bad_request
    pub fn snapshot(&self, refs: &[SkillRef]) -> Result<Vec<SkillSnapshot>, KitError>;
}
pub enum Source { Path(PathBuf) /* 绝对路径：SKILL.md、目录或 .zip */, Bytes { file_name: String, bytes: Vec<u8> } /* 文件选择或拖拽 */ }
// JSON（ts-rs `Source`）：{ "path": "C:\\…" } | { "bytes": { "fileName": "x.zip", "bytes": [80, 75, …] } }
```

```ts
interface SkillMeta { id: string; name: string; description: string; version: string | null; origin: 'imported' | 'manual' | 'builtin';
  revision: number; bodyChars: number; updatedAt: number; hasScripts: boolean; fileCount: number }
type SkillFileKind = 'script' | 'reference' | 'asset'
interface SkillFileMeta { path: string; kind: SkillFileKind; size: number }   // 按 path 排序
interface SkillDto extends SkillMeta { body: string; license: string | null; extra: Record<string, string>; files: SkillFileMeta[] }
interface Preview { digest: string; name: string; description: string; version: string | null; body: string; license: string | null;
  extra: Record<string, string>; files: SkillFileMeta[]; action: 'install' | 'replace'; current: { version: string | null; revision: number } | null }
interface ManualSave { id?: string; expectedRevision?: number; name: string; description: string; version?: string | null; body: string }
```

- `files`/`file` 供宿主（例如执行脚本前的审批与运行环境）按固定修订读取：`revision` 与当前不符或技能已删除 → `conflict{reason:"skill_changed", entity:id}`；路径不存在 → `not_found{entity:"skill_file"}`。本 crate 不执行、不落盘；`SkillSnapshot` 不变，Chat 只注入正文。Tauri 插件不新增 `skills_*` 命令，DTO 变化随生成类型生效。
- `save_manual` 编辑时 `expectedRevision` 必须等于当前修订，否则 `conflict{reason:"revision_changed"}`（防两个窗口互相覆盖）；缺省 → `bad_request{field:"expectedRevision"}`。新名称已被占用 → `conflict{reason:"exists"}`（内置名为 `builtin_name`）。

## 7. 陷阱

1. 预览与安装之间文件可能被换：靠 `digest` 比对，不靠"重新校验通过"。
2. 不在正文里做"清洗"：用户正文原样保存；安全边界在 Chat 的外部内容包装、工具目录与审批（12 文档 §2.2）。
3. 解压限制按实际读出的字节计数，不读 ZIP 头里声明的大小。
4. Windows 目录 junction 也算链接，必须拒绝（`symlink_metadata` + `FILE_ATTRIBUTE_REPARSE_POINT`）。
5. 名称是唯一键：重命名手写技能时检查冲突；内置技能的名称保留给内置。
