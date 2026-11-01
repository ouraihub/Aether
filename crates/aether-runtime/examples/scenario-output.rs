//! Executes shared scenarios against RunService and a controlled loopback model.
use aether_chat::{Approval, ToolEffect, ToolSource, ToolSpec};
use aether_runtime::{
    AddressPolicy, CancellationToken, EventSink, HostTool, KitHost, ModelRef, RunConfig,
    RunContext, RunDependencies, RunOwner, RunRequest, RunService, ToolCall, ToolError, ToolOutput,
};
use aether_testkit::{Exhausted, Gate, Reply, Server};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;
type Error = Box<dyn std::error::Error>;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Model {
    protocol: String,
    model_id: String,
    context_window: u32,
    max_output: u32,
}
#[derive(Deserialize)]
struct Definition {
    description: String,
    parameters: Value,
    effect: ToolEffect,
    output: String,
}
#[derive(Deserialize)]
struct Selected {
    name: String,
    approval: Approval,
}
#[derive(Deserialize)]
struct Call {
    id: String,
    name: String,
    args: Value,
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Parts {
    #[serde(default)]
    text: Vec<String>,
    #[serde(default)]
    reasoning: Vec<String>,
    #[serde(default)]
    calls: Vec<Call>,
    gate: Option<String>,
    after_gate: Option<Box<Parts>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Scenario {
    input: String,
    tools: Vec<Selected>,
    replies: Vec<Parts>,
    max_rounds: Option<u8>,
}
#[derive(Deserialize)]
struct Fixtures {
    model: Model,
    tools: BTreeMap<String, Definition>,
    scenarios: BTreeMap<String, Scenario>,
}
struct Host {
    tools: Vec<ToolSpec>,
    outputs: BTreeMap<String, String>,
    dispatches: Mutex<Vec<String>>,
}
#[async_trait::async_trait]
impl KitHost for Host {
    fn tools(&self, _: &RunRequest, _: &str) -> Vec<HostTool> {
        self.tools
            .iter()
            .cloned()
            .map(|spec| HostTool { spec })
            .collect()
    }
    async fn call_tool(
        &self,
        call: &ToolCall,
        _: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        self.dispatches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call.id.clone());
        Ok(ToolOutput {
            text: self
                .outputs
                .get(&call.name)
                .ok_or_else(ToolError::not_found)?
                .clone(),
            payload: None,
        })
    }
}
struct Recorder {
    events: Mutex<Vec<Value>>,
    changed: Notify,
    closed: AtomicBool,
    rejected: Notify,
}
impl Recorder {
    fn events(&self) -> Vec<Value> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    fn sink(self: &Arc<Self>) -> EventSink {
        let recorder = Arc::clone(self);
        Arc::new(move |event| {
            if recorder.closed.load(Ordering::Acquire) {
                recorder.rejected.notify_one();
                return Err(aether_runtime::KitError::cancelled());
            }
            recorder
                .events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(serde_json::to_value(event).map_err(aether_runtime::KitError::internal)?);
            recorder.changed.notify_waiters();
            Ok(())
        })
    }
    async fn wait(&self, predicate: impl Fn(&Value) -> bool) -> Result<(), Error> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let notified = self.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self
                    .events
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .iter()
                    .any(&predicate)
                {
                    return;
                }
                notified.await;
            }
        })
        .await
        .map_err(Into::into)
    }
}
fn expand(value: &Value) -> Result<Value, Error> {
    if let Value::Object(map) = value {
        if let (Some(prefix), Some(repeat), Some(count)) =
            (map.get("prefix"), map.get("repeat"), map.get("count"))
        {
            return Ok(Value::String(format!(
                "{}{}",
                prefix.as_str().ok_or("prefix")?,
                repeat
                    .as_str()
                    .ok_or("repeat")?
                    .repeat(usize::try_from(count.as_u64().ok_or("count")?)?)
            )));
        }
        return map
            .iter()
            .map(|(k, v)| Ok((k.clone(), expand(v)?)))
            .collect::<Result<serde_json::Map<_, _>, Error>>()
            .map(Value::Object);
    }
    Ok(value.clone())
}
fn parts(reply: &Parts) -> Result<String, Error> {
    let mut body = String::new();
    for text in &reply.reasoning {
        body.push_str(&format!(
            "data: {}\n\n",
            json!({"choices":[{"delta":{"reasoning_content":text}}]})
        ));
    }
    for text in &reply.text {
        body.push_str(&format!(
            "data: {}\n\n",
            json!({"choices":[{"delta":{"content":text}}]})
        ));
    }
    for (index, call) in reply.calls.iter().enumerate() {
        body.push_str(&format!("data: {}\n\n",json!({"choices":[{"delta":{"tool_calls":[{"index":index,"id":call.id,"type":"function","function":{"name":call.name,"arguments":expand(&call.args)?.to_string()}}]}}]})));
    }
    Ok(body)
}
fn wire(reply: &Parts, gate: &Gate) -> Result<Reply, Error> {
    let mut body = parts(reply)?;
    let mut response = Reply::sse_head();
    if reply.gate.is_some() {
        response = response.then_raw(body).then_wait(gate);
        body = match &reply.after_gate {
            Some(after) => parts(after)?,
            None => String::new(),
        };
    }
    let calls = reply.after_gate.as_deref().unwrap_or(reply).calls.len();
    body.push_str(&format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"delta":{},"finish_reason":if calls==0 {"stop"} else {"tool_calls"}}]})
    ));
    Ok(response.then_raw(body))
}
async fn run(id: &str, scenario: &Scenario, fixtures: &Fixtures) -> Result<Value, Error> {
    let gate = Gate::new();
    let replies = scenario
        .replies
        .iter()
        .map(|r| wire(r, &gate))
        .collect::<Result<Vec<_>, _>>()?;
    let server = Server::script(replies, Exhausted::Reply(Reply::close())).await;
    let root = tempfile::tempdir()?;
    let providers = Arc::new(aether_providers::Store::open(root.path())?);
    let model = &fixtures.model;
    let saved=providers.save(serde_json::from_value(json!({"name":"Scenario","endpoints":[{"slug":"scenario","baseUrl":format!("{}/v1",server.url()),"protocol":model.protocol,"models":[{"id":model.model_id,"modelType":"chat","typeSource":"user","contextWindow":model.context_window,"maxOutput":model.max_output,"canVision":false,"supportsTools":true}]}]}))?)?;
    let endpoint = saved
        .first()
        .and_then(|p| p.endpoints.first())
        .ok_or("missing endpoint")?;
    let reference = ModelRef {
        endpoint_id: endpoint.id.clone(),
        model_id: model.model_id.clone(),
    };
    let mut tools = Vec::new();
    let mut outputs = BTreeMap::new();
    for selected in &scenario.tools {
        let definition = fixtures.tools.get(&selected.name).ok_or("missing tool")?;
        tools.push(ToolSpec {
            name: selected.name.clone(),
            description: definition.description.clone(),
            parameters: definition.parameters.clone(),
            approval: selected.approval,
            effect: definition.effect,
            source: ToolSource::Host,
        });
        outputs.insert(selected.name.clone(), definition.output.clone());
    }
    let host = Arc::new(Host {
        tools,
        outputs,
        dispatches: Mutex::new(Vec::new()),
    });
    let mut config = RunConfig::default();
    config.budget.max_rounds = scenario.max_rounds.unwrap_or(config.budget.max_rounds);
    let service = RunService::new(
        host.clone(),
        config,
        RunDependencies::new(providers),
        AddressPolicy::default(),
    )?;
    let owner = RunOwner::new();
    let recorder = Arc::new(Recorder {
        events: Mutex::new(Vec::new()),
        changed: Notify::new(),
        closed: AtomicBool::new(false),
        rejected: Notify::new(),
    });
    let request: RunRequest = serde_json::from_value(
        json!({"purpose":"chat","model":reference,"skills":[],"input":scenario.input,"regenerate":false,"language":"en"}),
    )?;
    let accepted = service
        .start(
            RunContext {
                owner: owner.clone(),
                own_mcp_port: None,
            },
            request,
            recorder.sink(),
        )
        .await?;
    let mut marks = Vec::new();
    let mut replies = Vec::new();
    match id {
        "acceptance-response-race" => {
            marks.push("backend-accepted".to_owned());
            gate.open();
            marks.push("model-released".to_owned());
            recorder.wait(|e| e["kind"] == "end").await?;
            marks.extend(["end-received".to_owned(), "accepted-delivered".to_owned()]);
        }
        "tool-approval-allow" => {
            recorder.wait(|e| e["kind"] == "approval_required").await?;
            marks.push("approval-required".to_owned());
            let count = host
                .dispatches
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len();
            marks.push(format!("dispatches-before-approval:{count}"));
            let event = recorder
                .events()
                .into_iter()
                .find(|e| e["kind"] == "approval_required")
                .ok_or("approval event")?;
            let call = event["callId"].as_str().ok_or("call id")?;
            service.approve(&owner, &accepted.run_id, call, true)?;
            replies.push(json!({"op":"approve","status":"ok"}));
            let duplicate = match service.approve(&owner, &accepted.run_id, call, true) {
                Ok(_) => return Err("duplicate approval unexpectedly succeeded".into()),
                Err(error) => error,
            };
            replies.push(json!({"op":"approve-again","status":duplicate.code(),"reason":duplicate.details().reason()}));
            recorder.wait(|e| e["kind"] == "end").await?;
            marks.push("end-received".to_owned());
        }
        "receiver-send-failure" => {
            let text = scenario
                .replies
                .first()
                .and_then(|r| r.text.first())
                .ok_or("first text")?;
            recorder
                .wait(|e| e["kind"] == "text_delta" && e["text"] == *text)
                .await?;
            marks.push("first-delta".to_owned());
            recorder.closed.store(true, Ordering::Release);
            marks.push("receiver-closed".to_owned());
            gate.open();
            marks.push("model-released".to_owned());
            tokio::time::timeout(Duration::from_secs(10), recorder.rejected.notified()).await?;
            service.shutdown(Duration::from_secs(2)).await;
            marks.push("terminal".to_owned());
        }
        "round-exhaustion" => {
            recorder.wait(|e| e["kind"] == "end").await?;
            marks.push("end-received".to_owned());
        }
        _ => return Err("unknown executable scenario".into()),
    }
    service.shutdown(Duration::from_secs(2)).await;
    let mut snapshot = serde_json::to_value(service.snapshot(&owner, &accepted.run_id)?)?;
    snapshot["runId"] = json!("<run>");
    let mut accepted = serde_json::to_value(accepted)?;
    accepted["runId"] = json!("<run>");
    Ok(
        json!({"accepted":[accepted],"events":recorder.events(),"snapshots":[{"label":"final","value":snapshot}],"replies":replies,"modelRequests":server.seen().len(),"dispatches":host.dispatches.lock().unwrap_or_else(PoisonError::into_inner).clone(),"marks":marks}),
    )
}
#[tokio::main]
async fn main() -> Result<(), Error> {
    let fixtures: Fixtures =
        serde_json::from_str(include_str!("../../../contracts/scenario-fixtures.json"))?;
    let mut scenarios = BTreeMap::new();
    for (id, scenario) in &fixtures.scenarios {
        scenarios.insert(id, run(id, scenario, &fixtures).await?);
    }
    let stdout = io::stdout();
    let mut out = stdout.lock();
    serde_json::to_writer(&mut out, &json!({"backend":"native","scenarios":scenarios}))?;
    out.write_all(b"\n")?;
    Ok(())
}
