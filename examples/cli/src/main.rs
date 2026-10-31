//! 不依赖 Tauri 的命令行宿主：保存一个服务商 → 解析模型 → 跑一次 Chat（docs/90 R6-1、R6-2）。
//!
//! 用法：`example-cli <消息>`，环境变量：
//! `AETHER_BASE_URL`（必填）、`AETHER_MODEL`（必填）、
//! `AETHER_PROTOCOL`（默认 `openai-chat`）、
//! `AETHER_API_KEY`（可选；回环地址可省）、
//! `AETHER_TOOL=1`（提供只读工具 `get_secret_word`，自动执行，用于验证工具调用）。

use aether_observability::LogConfig;
use aether_providers::{
    EndpointKeySave, EndpointSave, ModelRef, ModelSave, ModelType, Protocol,
    ProviderSave, Store, TypeSource,
};
use aether_runtime::AddressPolicy;
use aether_runtime::{
    Approval, AuthorizeContext, CancellationToken, EndStatus, HostTool, KitHost,
    RunConfig, RunContext, RunDependencies, RunEvent, RunOwner, RunRequest,
    RunService,
    ToolCall,
    ToolEffect,
    ToolError,
    ToolOutput,
    ToolSource,
    ToolSpec,
};
use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};

/// The only tool: read-only, so `Auto`. Its answer is not guessable, which
/// proves the model really received the result.
const TOOL: &str = "get_secret_word";
const WORD: &str = "pelican-42";

/// The same host-policy seam used by the desktop adapter.
struct CliHost {
    with_tool: bool,
}

#[async_trait::async_trait]
impl KitHost for CliHost {
    fn tools(&self, _request: &RunRequest, _run_id: &str) -> Vec<HostTool> {
        if !self.with_tool {
            return Vec::new();
        }
        vec![HostTool {
            spec: ToolSpec {
                name: TOOL.to_owned(),
                description: "Returns today's secret word.".to_owned(),
                parameters: serde_json::json!({
                    "type": "object", "properties": {}
                }),
                approval: Approval::Auto,
                effect: ToolEffect::Read,
                source: ToolSource::Host,
            },
        }]
    }

    fn authorize(
        &self,
        call: &ToolCall,
        context: &AuthorizeContext<'_>,
    ) -> Approval {
        if call.name == TOOL {
            context.proposed
        } else {
            Approval::Deny
        }
    }

    async fn call_tool(
        &self,
        call: &ToolCall,
        _cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        if call.name != TOOL {
            return Err(ToolError::not_found());
        }
        Ok(ToolOutput {
            text: WORD.to_owned(),
            payload: None,
        })
    }
}

fn var(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is not set"))
}

#[expect(clippy::print_stderr, reason = "a CLI reports failures on stderr")]
fn main() {
    match run() {
        Ok(EndStatus::Completed) => {}
        Ok(status) => {
            eprintln!("run ended: {status:?}");
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("example-cli: {error}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<EndStatus, Box<dyn std::error::Error>> {
    let message = std::env::args()
        .nth(1)
        .ok_or("usage: example-cli <message>")?;
    let base_url = var("AETHER_BASE_URL")?;
    let model_id = var("AETHER_MODEL")?;
    let protocol: Protocol = serde_json::from_value(serde_json::Value::String(
        std::env::var("AETHER_PROTOCOL")
            .unwrap_or_else(|_| "openai-chat".to_owned()),
    ))?;
    let with_tool = std::env::var_os("AETHER_TOOL").is_some();

    let root = aether_observability::data_root("aether-cli")?;
    let logs = aether_observability::init(
        &root.join("logs"),
        LogConfig::default(),
        None,
    )?;

    // Providers: one provider named "cli", replaced as a whole on every start.
    let store = Store::open(&root)?;
    let existing = store.list()?.into_iter().find(|p| p.name == "cli");
    let saved = store.save(ProviderSave {
        id: existing.as_ref().map(|p| p.id.clone()),
        name: "cli".to_owned(),
        enabled: None,
        notes: None,
        preset_id: None,
        endpoints: vec![EndpointSave {
            id: existing
                .and_then(|p| p.endpoints.into_iter().next())
                .map(|e| e.id),
            slug: "main".to_owned(),
            base_url,
            protocol,
            key_strategy: None,
            models: vec![ModelSave {
                id: model_id.clone(),
                model_type: ModelType::Chat,
                type_source: TypeSource::User,
                context_window: None,
                max_output: None,
                can_vision: false,
                supports_tools: Some(with_tool),
                reasoning: None,
                native_web_search: false,
                name: None,
                group_name: None,
                pricing: None,
            }],
            image_base_url: None,
            image_edit_base_url: None,
            headers: None,
        }],
    })?;
    let endpoint = saved
        .iter()
        .find(|p| p.name == "cli")
        .and_then(|p| p.endpoints.first())
        .ok_or("saved provider has no endpoint")?;
    let endpoint_id = endpoint.id.clone();
    // Stored keys are kept, so the key only has to be passed once.
    if let Ok(key) = std::env::var("AETHER_API_KEY")
        && endpoint.keys.is_empty()
    {
        store.save_key(EndpointKeySave {
            endpoint_id: endpoint_id.clone(),
            id: None,
            key: Some(key),
            label: None,
            enabled: None,
            restore: None,
        })?;
    }
    let request: RunRequest = serde_json::from_value(serde_json::json!({
        "purpose": "chat", "model": ModelRef { endpoint_id, model_id },
        "skills": [], "input": message, "regenerate": false, "language": "en"
    }))?;
    let executor = tokio::runtime::Runtime::new()?;
    let result = executor.block_on(async {
        let runs = RunService::new(
            Arc::new(CliHost { with_tool }),
            RunConfig::default(),
            RunDependencies::new(Arc::new(store)),
            AddressPolicy::default(),
        )?;
        let (end, mut ended) = tokio::sync::mpsc::unbounded_channel();
        let out = Mutex::new(std::io::stdout());
        let started = runs
            .start(
                RunContext {
                    owner: RunOwner::new(),
                    own_mcp_port: None,
                },
                request,
                Arc::new(move |event| {
                    let mut out = out
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    let written = match &event {
                        RunEvent::TextDelta { text } => {
                            out.write_all(text.as_bytes())
                        }
                        RunEvent::ToolResult {
                            call_id, outcome, ..
                        } => writeln!(out, "\n[tool {call_id}: {outcome:?}]"),
                        RunEvent::End { status, .. } => {
                            let _ = end.send(*status);
                            writeln!(out)
                        }
                        RunEvent::SearchStart
                        | RunEvent::SearchResult { .. }
                        | RunEvent::RoundStart { .. }
                        | RunEvent::ToolCall { .. }
                        | RunEvent::ApprovalRequired { .. }
                        | RunEvent::Usage { .. }
                        | RunEvent::Context { .. }
                        | RunEvent::ReasoningDelta { .. } => Ok(()),
                    };
                    // CLI policy: stdout closure loses the echo, not the run
                    // or its result.
                    let _ = written.and_then(|()| out.flush());
                    Ok(())
                }),
            )
            .await;
        let status = match started {
            Ok(_) => ended.recv().await.ok_or_else(|| {
                aether_runtime::KitError::internal(std::io::Error::other(
                    "run closed without End",
                ))
            }),
            Err(error) => Err(error),
        };
        runs.shutdown(std::time::Duration::from_secs(3)).await;
        status
    });
    logs.flush()?;
    Ok(result?)
}
