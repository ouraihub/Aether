# AGENTS.md：给 AI 编码助手的硬性规则

## 1. 推送前按影响范围验证

- 默认 `pnpm run ci` 仍完整执行所有检查；本地与远端都使用 `scripts/ci.mjs`，不维护两套步骤。
- 用户于 2026-10-27 批准风险分层及夜间原生验证：普通文档执行 `node scripts/ci.mjs --scope docs`；工作流、根 AGENTS 和 CI 调度／门禁脚本执行 `--scope automation`（语法、行为及提交检查，不编译产品）；纯 UI 源码执行 `--scope frontend`；Rust、依赖、生成 DTO、产品构建脚本及未知路径在日常 CI 执行 Windows 完整检查。macOS/Linux 移至独立 `unix.yml`，仅手动或每天北京时间 02:00 定时运行，不阻塞 PR 合并。分类依据提交差异，不接受 PR 自报类别；实际编译参数／工具链变化须手动验证受影响平台。
- 完整检查保留前端、示例构建、依赖、DTO、i18n、Rust fmt/clippy、整个 workspace 测试与插件各 feature 编译；不以只测单 crate 代替。
- 对应检查不通过，不提交、不推送、不汇报完成。无法确认范围时完整执行。
- 发版前确认待发布提交的 macOS/Linux 完整检查通过；夜间失败必须处理，不能把未运行当通过。GitHub 定时运行可能延迟，02:00 是计划时间而非准点保证。

## 2. 改契约时，全仓库找旧名字

契约包括：`crates/*/src/dto.rs`、`packages/ui/src/ports/`、数据库迁移、i18n key、Tauri 命令名、错误 reason。

1. 改之前，在 Aether **和** 宿主仓库（如 `../AutoNote`）里搜索旧字段名或旧 key，**包括** `tests`、`fixtures`、`examples`、`scripts`、文档。
2. 一次改完所有引用，不留别名或兼容字段。
3. 改了 `dto.rs` 先跑 `pnpm kit:dto` 重新生成 TS。

## 3. 不许为了变绿而削弱检查

- 不删除、跳过或放宽测试，不加 `#[ignore]`、`.skip`、`allow(...)` 来让 CI 通过。测试失败时，先判断是代码错了还是测试过时了：测试过时，就按新行为改写它，不能直接删掉。
- 不从 `scripts/ci.mjs` 或 `.github/workflows/` 里删步骤、降低严格程度。确实需要调整时，先说明理由并取得用户同意。
- 新增检查只加到 `scripts/ci.mjs`，工作流会自动跟上，不要只改其中一边。
- 合并必需检查为确定性 `CI gate`，分类失败或任一适用任务失败／取消／意外跳过都拒绝。AI review 为辅助意见，不能启用自动合并；由维护者或获授权助手结合审查意见与 CI 决定合并。不增 Actions 读取权限、不部署凭据复用策略。此为用户批准的门禁简化，不得继续扩大豁免范围。

## 4. 推送之后

- 推送后到 GitHub Actions 看结果：`gh run list -R ouraihub/Aether -L 3`，需要先登录。CI 失败就立刻修复，不留给下一个人。
- 宿主要更新引用的 Aether commit 时，先确认这个 commit 的 CI 是绿的。

## 5. Windows 环境

- Rust 命令一律通过 `node_modules\@ouraihub\desktop-kit\msvc\cargo-msvc.bat` 运行（先 `pnpm install`）。Git 自带的 `link.exe` 会挡住 MSVC 的链接器。
- 在 Git Bash 里运行时，用 `cmd.exe /d /c "call node_modules\@ouraihub\desktop-kit\msvc\cargo-msvc.bat test --workspace"`。

## 6. 提交信息和代码注释

`pnpm run ci` 第一步是 `release-kit check`，不合规直接失败。规则全文见 [release-kit 规则](https://github.com/ouraihub/release-kit#规则)，要点：

- 格式 `<type>(<scope>): <摘要>`，一条提交只做一件事，标题不超过 72 个字符。
- 正文默认不写；只在 diff 看不出原因时写 1–3 行"为什么"。不写文件列表、字段清单、验证记录、AI 署名。
- 提交、注释、标识符里不写参考或对标项目名（禁用词在 `release-kit.json`）。注释写行为和约束；来源只写进 `docs/`。
