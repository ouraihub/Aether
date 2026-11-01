use super::*;

#[tokio::test]
async fn host_mcp_false_prevents_even_protocol_discovery() {
    let model_server = upstream(vec![sse(HELLO), sse(HELLO)]).await;
    let model_url = format!("{}/v1", model_server.url());
    let mut mcp_server = Server::script(
        vec![Reply::raw(
            "HTTP/1.1 503 Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        )],
        Exhausted::Reply(Reply::close()),
    )
    .await;
    let mcp_url = format!("{}/mcp", mcp_server.url());
    let dir = tempfile::tempdir().unwrap();
    let (providers, request) = local_model(dir.path(), &model_url, "hi");
    let mcp_store = Arc::new(aether_mcp_client::Store::open(dir.path()).unwrap());
    mcp_store
        .save(aether_mcp_client::ServerSave {
            id: None,
            slug: "local".into(),
            name: "Local".into(),
            transport: aether_mcp_client::McpTransport::Http { url: mcp_url },
            secret: None,
            enabled: true,
            trusted: true,
            allow_private: true,
        })
        .unwrap();
    let mcp = Arc::new(aether_mcp_client::McpRuntime::new(mcp_store));
    let mut deps = RunDependencies::new(providers);
    deps.mcp = Some(mcp.clone());
    let service = RunService::new(
        Arc::new(AllModes),
        RunConfig::default(),
        deps,
        AddressPolicy::default(),
    )
    .unwrap();
    let owner = RunOwner::new();
    let (sink, mut receiver) = events();
    service
        .start(
            context(&owner),
            RunRequest {
                purpose: "writing".into(),
                ..request.clone()
            },
            sink,
        )
        .await
        .unwrap();
    assert_end(&until_end(&mut receiver).await, EndStatus::Completed);
    // A real enabled endpoint exists, but even initialize must not be dispatched.
    mcp_server.no_request(Duration::from_millis(30)).await;
    // The same runtime and host allow discovery for chat; observe an actual request.
    let (sink, mut receiver) = events();
    service.start(context(&owner), request, sink).await.unwrap();
    assert_end(&until_end(&mut receiver).await, EndStatus::Completed);
    assert!(mcp_server.request().await.raw_text().contains("initialize"));
    service.shutdown(Duration::from_secs(2)).await;
    mcp.shutdown().await;
    wire_requests(&model_server, 2);
}
