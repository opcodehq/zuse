use super::*;
use crate::{
    Message, ModelInput, RequestBudget, RequestPurpose, Role, ToolCategory, ToolDefinition, Tools,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};
struct Budget;
#[async_trait]
impl RequestBudget for Budget {
    async fn reserve(&self) -> Result<(), Error> {
        Ok(())
    }
}
fn input() -> ModelInput {
    ModelInput {
        root_id: "root".into(),
        agent_id: "root".into(),
        model: "fixture".into(),
        reasoning: None,
        messages: Arc::new(vec![
            Message::text(Role::System, "instructions"),
            Message::text(Role::User, "task"),
        ]),
        tools: Arc::new(Tools::from([(
            "edit".into(),
            ToolDefinition {
                description: "Edit".into(),
                parameters: json!({"type":"object"}),
                category: ToolCategory::Edit,
            },
        )])),
        purpose: RequestPurpose::Agent,
        retry_index: 0,
        cancel: CancellationToken::new(),
        text: Arc::new(|_| {}),
        budget: Arc::new(Budget),
        report: Arc::new(|_| {}),
    }
}
async fn fixture(
    protocol: Protocol,
    events: Vec<Value>,
    end: bool,
) -> (Credential, oneshot::Receiver<Value>) {
    authenticated_fixture(
        protocol,
        events,
        end,
        Authentication::ProtocolDefault,
        Vec::new(),
    )
    .await
}
async fn authenticated_fixture(
    protocol: Protocol,
    events: Vec<Value>,
    end: bool,
    authentication: Authentication,
    betas: Vec<String>,
) -> (Credential, oneshot::Receiver<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = oneshot::channel();
    let expected_betas = betas.join(",");
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0; 4096];
        let boundary = loop {
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&chunk[..n]);
            if let Some(i) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
                break i + 4;
            }
        };
        let headers = String::from_utf8(bytes[..boundary].to_vec()).unwrap();
        let lower = headers.to_ascii_lowercase();
        if authentication == Authentication::Bearer
            || matches!(protocol, Protocol::Responses | Protocol::ChatCompletions)
        {
            assert!(lower.contains("authorization: bearer fixture-token\r\n"));
            assert!(!lower.contains("x-api-key:"));
            assert!(!lower.contains("x-goog-api-key:"));
        } else if protocol == Protocol::Messages {
            assert!(lower.contains("x-api-key: fixture-token\r\n"));
            assert!(!lower.contains("authorization:"));
        } else {
            assert!(lower.contains("x-goog-api-key: fixture-token\r\n"));
        }
        if !expected_betas.is_empty() {
            assert!(lower.contains(&format!("anthropic-beta: {expected_betas}\r\n")));
        }
        let length = headers
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|s| s.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while bytes.len() - boundary < length {
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&chunk[..n]);
        }
        tx.send(serde_json::from_slice(&bytes[boundary..boundary + length]).unwrap())
            .unwrap();
        let hold = events.iter().any(|e| e["fixtureHold"] == true);
        let mut body = events
            .iter()
            .filter(|e| e["fixtureHold"] != true)
            .map(|e| format!("data: {e}\r\n\r\n"))
            .collect::<String>();
        if end {
            body.push_str("data: [DONE]\r\n\r\n");
        }
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()+usize::from(hold)).as_bytes()).await.unwrap();
        // Fragment inside JSON, delimiters and multi-byte text.
        for part in body.as_bytes().chunks(7) {
            if socket.write_all(part).await.is_err() {
                break;
            }
        }
        if hold {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                socket.read(&mut [0u8; 1]),
            )
            .await;
        }
    });
    (
        Credential {
            connection_id: "account".into(),
            access_token: "fixture-token".into(),
            authorization_generation: 0,
            route: Route {
                endpoint,
                protocol,
                subscription: false,
                authentication,
                betas,
                capabilities: Capabilities {
                    images: true,
                    tools: true,
                    reasoning: true,
                    prompt_cache: true,
                },
                max_output_tokens: 4096,
                tool_namespace: None,
                include_encrypted_reasoning: false,
            },
        },
        rx,
    )
}
#[tokio::test]
async fn responses_streams_text_tools_usage_and_preserves_same_account_reasoning() {
    let events = vec![
        json!({"type":"response.output_text.delta","delta":"héllo"}),
        json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"héllo"}]}}),
        json!({"type":"response.output_item.done","output_index":1,"item":{"type":"reasoning","id":"r1","encrypted_content":"private"}}),
        json!({"type":"response.output_item.done","output_index":2,"item":{"type":"function_call","call_id":"call1","name":"edit","arguments":"{\"path\":\"a\"}"}}),
        json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":100,"output_tokens":10,"input_tokens_details":{"cached_tokens":80},"output_tokens_details":{"reasoning_tokens":4}}}}),
    ];
    let (c, body) = fixture(Protocol::Responses, events, false).await;
    let transport = NativeTransport::new().unwrap();
    let route = c.route.clone();
    let mut request = input();
    let output = transport.request(request.clone(), c).await.unwrap();
    assert_eq!(output.text, "héllo");
    assert_eq!(output.calls[0].input, json!({"path":"a"}));
    assert_eq!(output.usage.cached_tokens, Some(80));
    assert_eq!(output.usage.uncached_input_tokens, Some(20));
    assert_eq!(output.usage.reasoning_tokens, Some(4));
    assert!(output.usage.request_id.is_some());
    let body = body.await.unwrap();
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    let key = body["prompt_cache_key"].clone();
    Arc::make_mut(&mut request.messages).extend(output.messages);
    let c = Credential {
        connection_id: "account".into(),
        access_token: "unused".into(),
        authorization_generation: 0,
        route,
    };
    let resumed = request::body(&request, &c).unwrap();
    assert_eq!(resumed["prompt_cache_key"], key);
    assert!(
        resumed["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["encrypted_content"] == "private")
    );
    let subscription = Credential {
        connection_id: "account".into(),
        access_token: "unused".into(),
        authorization_generation: 0,
        route: Route::subscription("chatgpt").unwrap(),
    };
    let subscription_body = request::body(&input(), &subscription).unwrap();
    assert_eq!(subscription_body["tools"][0]["type"], "namespace");
    assert!(subscription_body.get("max_output_tokens").is_none());
    let c = Credential {
        connection_id: "other".into(),
        access_token: "unused".into(),
        authorization_generation: 0,
        route: c.route,
    };
    let switched = request::body(&request, &c).unwrap();
    assert!(!switched.to_string().contains("private"));
}
#[tokio::test]
async fn messages_streams_tool_arguments_signatures_and_disjoint_cache_counters() {
    let events = vec![
        json!({"type":"message_start","message":{"id":"m1","usage":{"input_tokens":7,"cache_read_input_tokens":80,"cache_creation_input_tokens":13}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"thought"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed"}}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"c1","name":"edit","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"a\"}"}}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}),
        json!({"type":"message_stop"}),
    ];
    let (c, body) = fixture(Protocol::Messages, events, false).await;
    let output = NativeTransport::new()
        .unwrap()
        .request(input(), c)
        .await
        .unwrap();
    assert_eq!(output.calls[0].input, json!({"path":"a"}));
    assert_eq!(output.usage.input_tokens, 100);
    assert_eq!(output.usage.uncached_input_tokens, Some(7));
    assert_eq!(output.usage.cache_write_tokens, Some(13));
    assert!(output.messages[0].content.to_string().contains("signed"));
    let body = body.await.unwrap();
    assert_eq!(body["tools"][0]["cache_control"]["type"], "ephemeral");
}
#[tokio::test]
async fn generate_content_and_chat_completions_normalize_streams() {
    let(c,body)=fixture(Protocol::GenerateContent,vec![json!({"candidates":[{"content":{"parts":[{"text":"answer"},{"functionCall":{"name":"edit","args":{"path":"a"}},"thoughtSignature":"signed"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":100,"cachedContentTokenCount":75,"candidatesTokenCount":10,"thoughtsTokenCount":3}})],false).await;
    let t = NativeTransport::new().unwrap();
    let result = t.request(input(), c).await.unwrap();
    assert_eq!(result.text, "answer");
    assert_eq!(result.usage.output_tokens, 13);
    assert_eq!(result.usage.cached_tokens, Some(75));
    assert_eq!(result.calls.len(), 1);
    assert!(result.messages[0].content.to_string().contains("signed"));
    assert!(body.await.unwrap()["tools"].is_array());
    let(c,body)=fixture(Protocol::ChatCompletions,vec![json!({"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}),json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"edit","arguments":"{\"x\":"}}]},"finish_reason":null}]}),json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]},"finish_reason":"tool_calls"}]}),json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}})],true).await;
    let result = t.request(input(), c).await.unwrap();
    assert_eq!(result.text, "hello");
    assert_eq!(result.calls[0].input, json!({"x":1}));
    assert_eq!(result.usage.cached_tokens, None);
    assert_eq!(result.usage.uncached_input_tokens, None);
    assert!(body.await.unwrap()["messages"].is_array());
}
#[tokio::test]
async fn incomplete_streams_and_capability_mismatches_fail_closed() {
    let (c, body) = fixture(
        Protocol::Responses,
        vec![json!({"type":"response.output_text.delta","delta":"partial"})],
        false,
    )
    .await;
    assert!(
        NativeTransport::new()
            .unwrap()
            .request(input(), c)
            .await
            .is_err()
    );
    body.await.unwrap();
    let mut c = Credential {
        connection_id: "account".into(),
        access_token: "unused".into(),
        authorization_generation: 0,
        route: Route::subscription("chatgpt").unwrap(),
    };
    c.route.capabilities.reasoning = false;
    let mut request = input();
    request.reasoning = Some("high".into());
    assert!(request::body(&request, &c).is_err());
    let mut decoder = stream::Sse::default();
    assert!(decoder.push(&vec![b'a'; stream::FRAME_LIMIT + 1]).is_err());
    let t = NativeTransport::new().unwrap();
    t.close().await;
    assert!(matches!(
        t.request(request, c).await,
        Err(Error::Interrupted)
    ));
}

#[tokio::test]
async fn cancelling_one_native_stream_does_not_interrupt_another() {
    let started = Arc::new(tokio::sync::Notify::new());
    let notice = started.clone();
    let (slow, slow_body) = fixture(
        Protocol::Responses,
        vec![
            json!({"type":"response.output_text.delta","delta":"partial"}),
            json!({"fixtureHold":true}),
        ],
        false,
    )
    .await;
    let (fast,fast_body)=fixture(Protocol::Responses,vec![json!({"type":"response.output_text.delta","delta":"complete"}),json!({"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1}}})],false).await;
    let transport = NativeTransport::new().unwrap();
    let mut slow_input = input();
    slow_input.text = Arc::new(move |_| notice.notify_one());
    let cancel = slow_input.cancel.clone();
    let t = transport.clone();
    let slow_task = tokio::spawn(async move { t.request(slow_input, slow).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    cancel.cancel();
    let output = transport.request(input(), fast).await.unwrap();
    assert_eq!(output.text, "complete");
    assert!(matches!(slow_task.await.unwrap(), Err(Error::Interrupted)));
    slow_body.await.unwrap();
    fast_body.await.unwrap();
}
#[tokio::test]
async fn physical_request_metering_and_partial_usage_survive_errors() {
    let directory = tempfile::tempdir().unwrap();
    let ledger =
        crate::evaluation::RequestLedger::new(directory.path().join("requests"), 1).unwrap();
    let attempts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let reports = attempts.clone();
    let transport = NativeTransport::new()
        .unwrap()
        .with_budget(Arc::new(ledger))
        .with_observer(Arc::new(move |attempt| {
            reports.lock().unwrap().push(attempt)
        }));
    let (credential,body)=fixture(Protocol::Messages,vec![json!({"type":"message_start","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":20}}}),json!({"type":"error","error":{"type":"overloaded_error"}})],false).await;
    assert!(transport.request(input(), credential).await.is_err());
    body.await.unwrap();
    let credential = Credential {
        connection_id: "account".into(),
        access_token: "unused".into(),
        authorization_generation: 0,
        route: Route::subscription("chatgpt").unwrap(),
    };
    assert!(
        matches!(transport.request(input(),credential).await,Err(Error::Model{code,..})if code=="evaluation_budget_exhausted")
    );
    let reports = attempts.lock().unwrap();
    assert_eq!(reports.len(), 2);
    assert!(reports[0].dispatched);
    assert_eq!(reports[0].usage.as_ref().unwrap().cached_tokens, Some(20));
    assert_eq!(reports[0].usage.as_ref().unwrap().input_tokens, 30);
    assert!(!reports[1].dispatched);
    assert_eq!(
        reports[1].error_code.as_deref(),
        Some("evaluation_budget_exhausted")
    );
}
#[test]
fn cache_configuration_fingerprints_are_stable_across_appended_turns() {
    let mut request = input();
    let mut credential = Credential {
        connection_id: "a".into(),
        access_token: "not-logged".into(),
        authorization_generation: 0,
        route: Route::subscription("chatgpt").unwrap(),
    };
    let before = request::fingerprint(&request, &credential);
    let key = request::cache_key(&request, &credential);
    Arc::make_mut(&mut request.messages).push(Message::text(Role::User, "another turn"));
    assert_eq!(request::fingerprint(&request, &credential), before);
    assert_eq!(request::cache_key(&request, &credential), key);
    request.reasoning = Some("high".into());
    assert_ne!(request::fingerprint(&request, &credential), before);
    credential.authorization_generation = 1;
    assert_ne!(request::cache_key(&request, &credential), key);
    let c = Credential {
        connection_id: "a".into(),
        access_token: "not-logged".into(),
        authorization_generation: 0,
        route: Route::subscription("supergrok").unwrap(),
    };
    let body = request::body(&input(), &c).unwrap();
    assert!(body.get("include").is_none());
    assert_eq!(body["tools"][0]["type"], "function");
}

#[tokio::test]
async fn aborted_call_reports_its_physical_attempt() {
    let started = Arc::new(tokio::sync::Notify::new());
    let notice = started.clone();
    let (credential, body) = fixture(
        Protocol::Responses,
        vec![
            json!({"type":"response.output_text.delta","delta":"partial"}),
            json!({"fixtureHold":true}),
        ],
        false,
    )
    .await;
    let attempts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let reports = attempts.clone();
    let transport = NativeTransport::new()
        .unwrap()
        .with_observer(Arc::new(move |attempt| {
            reports.lock().unwrap().push(attempt)
        }));
    let mut request = input();
    request.text = Arc::new(move |_| notice.notify_one());
    let task = tokio::spawn(async move { transport.request(request, credential).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    body.await.unwrap();
    let attempts = attempts.lock().unwrap();
    assert_eq!(attempts.len(), 1);
    assert!(attempts[0].dispatched);
    assert_eq!(attempts[0].error_code.as_deref(), Some("request_abandoned"));
}

#[test]
fn image_tool_results_follow_protocol_capabilities() {
    let mut request = input();
    Arc::make_mut(&mut request.messages).push(Message {
        role: Role::Tool,
        content: json!([{"type":"tool-result","toolCallId":"c1","toolName":"image","output":{"type":"content","value":[{"type":"image","mediaType":"image/png","data":{"type":"base64","data":"AQID"}}]}}]).into(),
        metadata: Default::default(),
    });
    let mut credential = Credential {
        connection_id: "a".into(),
        access_token: "unused".into(),
        authorization_generation: 0,
        route: Route::subscription("chatgpt").unwrap(),
    };
    for protocol in [
        Protocol::Responses,
        Protocol::Messages,
        Protocol::GenerateContent,
    ] {
        credential.route.protocol = protocol;
        let body = request::body(&request, &credential).unwrap();
        assert!(body.to_string().contains("AQID"));
        if protocol == Protocol::GenerateContent {
            assert_eq!(
                body["contents"][1]["parts"][0]["functionResponse"]["parts"][0]["inlineData"]["mimeType"],
                "image/png"
            );
        }
    }
    credential.route.protocol = Protocol::ChatCompletions;
    assert!(
        matches!(request::body(&request, &credential), Err(Error::Model {code, ..}) if code == "unsupported_image_tool_result")
    );
    credential.route.capabilities.images = false;
    assert!(
        matches!(request::body(&request, &credential), Err(Error::Model {code, ..}) if code == "unsupported_images")
    );
}

#[tokio::test]
async fn explicit_bearer_authentication_preserves_medium_reasoning_and_subscription_usage() {
    let (mut credential, body) = authenticated_fixture(Protocol::Messages, vec![
        json!({"type":"message_start","message":{"usage":{"input_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"ready"}}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
        json!({"type":"message_stop"}),
    ], false, Authentication::Bearer, vec!["oauth-2025-04-20".into()]).await;
    credential.route.subscription = true;
    let mut request = input();
    request.reasoning = Some("medium".into());
    let output = NativeTransport::new()
        .unwrap()
        .request(request, credential)
        .await
        .unwrap();
    assert_eq!(output.text, "ready");
    assert_eq!(output.usage.billing_route.as_deref(), Some("subscription"));
    let body = body.await.unwrap();
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "medium");
}
#[test]
fn provider_beta_configuration_is_bounded_and_default_route_hashes_stay_compatible() {
    let mut route = Route::subscription("chatgpt").unwrap();
    let serialized = serde_json::to_value(&route).unwrap();
    assert!(serialized.get("authentication").is_none());
    assert!(serialized.get("betas").is_none());
    route.betas = vec!["oauth-2025-04-20".into()];
    assert!(route.validate_authentication().is_err());
    route.protocol = Protocol::Messages;
    assert!(route.validate_authentication().is_ok());
    route.betas = vec!["invalid\r\nheader".into()];
    assert!(route.validate_authentication().is_err());
}
