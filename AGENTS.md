# AGENTS.md：给 AI 编码助手的硬性规则

## 1. 推送前必须通过 `pnpm run ci`

- `pnpm run ci`（`scripts/ci.mjs`）就是 GitHub CI 的 windows job，两边执行同一份步骤：前端检查、示例构建、依赖规则、DTO、i18n、`cargo fmt`、`clippy -D warnings`、**整个 workspace** 的 `cargo test`、插件各 feature 单独编译。
- 只跑改动 crate 的测试（`-p xxx`）不算验证。曾经出过的事故：删掉 `EndpointSave.apiKey` 后只测了 `aether-providers`，`tauri-plugin-aether` 的测试夹具还在用旧字段，CI 挂了 5 个测试。
- `pnpm run ci` 不通过，不提交、不推送、不汇报“已完成”。

## 2. 改契约时，全仓库找旧名字

契约包括：`crates/*/src/dto.rs`、`packages/ui/src/ports/`、数据库迁移、i18n key、Tauri 命令名、错误 reason。

1. 改之前，在 Aether **和** 宿主仓库（如 `../AutoNote`）里搜索旧字段名或旧 key，**包括** `tests`、`fixtures`、`examples`、`scripts`、文档。
2. 一次改完所有引用，不留别名或兼容字段。
3. 改了 `dto.rs` 先跑 `pnpm kit:dto` 重新生成 TS。

## 3. 不许为了变绿而削弱检查

- 不删除、跳过或放宽测试，不加 `#[ignore]`、`.skip`、`allow(...)` 来让 CI 通过。测试失败时，先判断是代码错了还是测试过时了：测试过时，就按新行为改写它，不能直接删掉。
- 不从 `scripts/ci.mjs` 或 `.github/workflows/` 里删步骤、降低严格程度。确实需要调整时，先说明理由并取得用户同意。
- 新增检查只加到 `scripts/ci.mjs`，工作流会自动跟上，不要只改其中一边。

## 4. 推送之后

- 推送后到 GitHub Actions 看结果：`gh run list -R ouraihub/Aether -L 3`，需要先登录。CI 失败就立刻修复，不留给下一个人。
- 宿主要更新引用的 Aether commit 时，先确认这个 commit 的 CI 是绿的。

## 5. Windows 环境

- Rust 命令一律通过 `node_modules\@ouraihub\msvc-kit\cargo-msvc.bat` 运行（先 `pnpm install`）。Git 自带的 `link.exe` 会挡住 MSVC 的链接器。
- 在 Git Bash 里运行时，用 `cmd.exe /d /c "call node_modules\@ouraihub\msvc-kit\cargo-msvc.bat test --workspace"`。

## 6. 提交信息和代码注释

`pnpm run ci` 第一步是 `release-kit check`，不合规直接失败。规则全文见 [release-kit 规则](https://github.com/ouraihub/release-kit#规则)，要点：

- 格式 `<type>(<scope>): <摘要>`，一条提交只做一件事，标题不超过 72 个字符。
- 正文默认不写；只在 diff 看不出原因时写 1–3 行"为什么"。不写文件列表、字段清单、验证记录、AI 署名。
- 提交、注释、标识符里不写参考或对标项目名（禁用词在 `release-kit.json`）。注释写行为和约束；来源只写进 `docs/`。
