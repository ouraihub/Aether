//! Executes the MCP result projection, including error payloads and non-text blocks.
use super::mcp_execution;
use aether_chat::ToolExecution;
use aether_mcp_client::McpCallResult;
use serde_json::json;

#[test]
fn completed_mcp_content_and_structured_payload_survive_success_and_error() {
    let content = vec![
        json!({"type":"text","text":"external text"}),
        json!({"type":"image","mimeType":"image/png","data":"aW4x"}),
    ];
    let structured = Some(json!({"rows":[{"id":1}]}));
    for is_error in [false, true] {
        let execution = mcp_execution(McpCallResult::Completed {
            content: content.clone(),
            structured_content: structured.clone(),
            is_error,
        });
        let expected = Some(json!({"content":content,"structuredContent":structured}));
        match execution {
            ToolExecution::Ok { text, payload } => {
                assert!(!is_error);
                assert!(text.starts_with("external text\n"));
                assert!(text.contains("\"type\":\"image\""));
                assert!(
                    !text.contains("<untrusted"),
                    "chat owns the external-content wrapper"
                );
                assert_eq!(payload, expected);
            }
            ToolExecution::Error {
                code,
                message,
                payload,
            } => {
                assert!(is_error);
                assert_eq!(code, "mcp_error");
                assert!(message.contains("external text"));
                assert!(message.contains("\"mimeType\":\"image/png\""));
                assert_eq!(payload, expected);
            }
            ToolExecution::Unknown { reason } => assert_eq!(
                Some(reason),
                None,
                "completed MCP result cannot become unknown"
            ),
        }
    }
}

#[test]
fn mcp_known_failure_and_uncertain_effect_are_distinct() {
    assert!(
        matches!(mcp_execution(McpCallResult::Failed { code:"protocol".into(), message:"invalid result".into() }), ToolExecution::Error { code, message, payload:None } if code == "protocol" && message == "invalid result")
    );
    assert!(
        matches!(mcp_execution(McpCallResult::Unknown { reason:"connection lost after dispatch".into() }), ToolExecution::Unknown { reason } if reason == "connection lost after dispatch")
    );
}
