//! 不依赖 Tauri 的命令行宿主：保存一个服务商 → 解析模型 → 跑一次 Chat（docs/90 R6-1、R6-2）。
//!
//! 用法：`example-cli <消息>`，环境变量：
//! `AETHER_BASE_URL`（必填）、`AETHER_MODEL`（必填）、`AETHER_PROTOCOL`（默认 `openai-chat`）、`AETHER_API_KEY`（可选；回环地址可省）、
//! `AETHER_TOOL=1`（提供只读工具 `get_secret_word`，自动执行，用于验证工具调用）。

use std::io::Write;

use aether_chat::{
    AddressPolicy, Approval, Approver, BoxFuture, Budget, CancellationToken, EndStatus, RunEvent,
    RunInput, Runner, ToolCall, ToolExecution, ToolExecutor, ToolSource, ToolSpec,
};
use aether_observability::LogConfig;
use aether_providers::{
    EndpointKeySave, EndpointSave, ModelRef, ModelSave, ModelType, Protocol, ProviderSave,
    ReasoningEffort, Store, TypeSource,
};

/// The only tool: read-only, so `Auto`. Its answer is not guessable, which proves the model
/// really received the result.
const TOOL: &str = "get_secret_word";
const WORD: &str = "pelican-42";

/// Runs [`TOOL`]; anything else is denied, and nothing ever waits for approval.
struct CliTools;

#[async_trait::async_trait]
impl ToolExecutor for CliTools {
    fn authorize(&self, call: &ToolCall, default: Approval) -> Approval {
        if call.name == TOOL {
            default
        } else {
            Approval::Deny
        }
    }
    async fn execute(&self, _call: &ToolCall, _cancel: &CancellationToken) -> ToolExecution {
        ToolExecution::Ok {
            text: WORD.to_owned(),
            payload: None,
        }
    }
}

impl Approver for CliTools {
    fn wait(
        &self,
        _run: &str,
        _call: &ToolCall,
        _cancel: &CancellationToken,
    ) -> BoxFuture<'_, bool> {
        Box::pin(async { false })
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
        std::env::var("AETHER_PROTOCOL").unwrap_or_else(|_| "openai-chat".to_owned()),
    ))?;
    let with_tool = std::env::var_os("AETHER_TOOL").is_some();

    let root = aether_observability::data_root("aether-cli")?;
    let logs = aether_observability::init(&root.join("logs"), LogConfig::default(), None)?;

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
        })?;
    }
    let connection = store.resolve(&ModelRef {
        endpoint_id,
        model_id,
    })?;

    // Chat: one run, text streamed to stdout, tool calls reported on their own lines.
    let tools = if with_tool {
        vec![ToolSpec {
            name: TOOL.to_owned(),
            description: "Returns today's secret word.".to_owned(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
            approval: Approval::Auto,
            source: ToolSource::Host,
        }]
    } else {
        Vec::new()
    };
    let input = RunInput {
        run_id: "cli".to_owned(),
        budget: Budget::default().for_connection(&connection),
        connection,
        system: Vec::new(),
        history: Vec::new(),
        input: message,
        images: Vec::new(),
        tools,
        reasoning_effort: ReasoningEffort::Default,
        native_web_search: false,
    };
    let runner = Runner::new(AddressPolicy::default())?;
    let tokio = tokio::runtime::Runtime::new()?;
    let mut out = std::io::stdout();
    let summary = tokio.block_on(runner.run(
        input,
        &CliTools,
        &CliTools,
        &CancellationToken::new(),
        |event| {
            // A closed stdout only loses the echo; the run itself goes on.
            let written = if let RunEvent::TextDelta { text } = &event {
                out.write_all(text.as_bytes())
            } else if let RunEvent::ToolResult {
                call_id, outcome, ..
            } = &event
            {
                writeln!(out, "\n[tool {call_id}: {outcome:?}]")
            } else {
                Ok(())
            };
            let _ = written.and_then(|()| out.flush());
        },
    ));
    let _ = writeln!(out);
    logs.flush()?;
    Ok(summary.status)
}
