use super::*;

#[tokio::test]
async fn host_mcp_false_prevents_even_protocol_discovery() {
    let model_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let model_url = format!("http://{}/v1", model_listener.local_addr().unwrap());
    let model_server = tokio::spawn(upstream(model_listener, vec![HELLO, HELLO]));
    let mcp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mcp_url = format!("http://{}/mcp", mcp_listener.local_addr().unwrap());
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
    assert!(
        tokio::time::timeout(Duration::from_millis(30), mcp_listener.accept())
            .await
            .is_err()
    );
    // The same runtime and host allow discovery for chat; observe an actual request.
    let discovery = tokio::spawn(async move {
        let (mut socket, _) = mcp_listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        socket
            .write_all(
                b"HTTP/1.1 503 Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        request
    });
    let (sink, mut receiver) = events();
    service.start(context(&owner), request, sink).await.unwrap();
    assert_end(&until_end(&mut receiver).await, EndStatus::Completed);
    assert!(
        tokio::time::timeout(Duration::from_secs(10), discovery)
            .await
            .unwrap()
            .unwrap()
            .contains("initialize")
    );
    service.shutdown(Duration::from_secs(2)).await;
    mcp.shutdown().await;
    model_server.await.unwrap();
}
