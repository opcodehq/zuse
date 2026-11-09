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
    let expected_betas = if authentication == Authentication::AnthropicOauth {
        let mut flags = vec!["claude-code-20250219", "oauth-2025-04-20"];
        for beta in &betas {
            if !flags.contains(&beta.as_str()) {
                flags.push(beta);
            }
        }
        flags.join(",")
    } else {
        betas.join(",")
    };
    let token = if authentication == Authentication::CodexOauth {
        use base64::Engine as _;
        format!("header.{}.signature", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({"https://api.openai.com/auth":{"chatgpt_account_id":"workspace-fixture"}})).unwrap()))
    } else {
        "fixture-token".into()
    };
    let expected_token = token.to_ascii_lowercase();
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
        if authentication != Authentication::ProtocolDefault
            || matches!(protocol, Protocol::Responses | Protocol::ChatCompletions)
        {
            assert!(lower.contains(&format!("authorization: bearer {expected_token}\r\n")));
            if authentication == Authentication::CodexOauth {
                assert!(lower.contains("chatgpt-account-id: workspace-fixture\r\n"));
                assert!(lower.contains("originator: zuse\r\n"));
            }
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
        if authentication == Authentication::GrokOauth {
            assert!(lower.contains("x-xai-token-auth: xai-grok-cli\r\n"));
            assert!(lower.contains("x-grok-model-override: fixture\r\n"));
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
            access_token: token,
            authorization_generation: 0,
            route: Route {
                endpoint,
                protocol,
                subscription: matches!(
                    authentication,
                    Authentication::CodexOauth | Authentication::GrokOauth
                ),
                authentication,
                betas,
                capabilities: Capabilities {
                    images: true,
                    tools: true,
                    reasoning: true,
                    prompt_cache: true,
                },
                max_output_tokens: 4096,
                allowed_reasoning: None,
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
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"a\"}"}}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5,"output_tokens_details":{"thinking_tokens":2}}}),
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
    assert_eq!(output.usage.reasoning_tokens, Some(2));
    assert_eq!(output.usage.output_tokens, 5);
    assert!(output.messages[0].content.to_string().contains("signed"));
    let body = body.await.unwrap();
    assert_eq!(body["tools"][0]["cache_control"]["type"], "ephemeral");
}
#[tokio::test]
async fn messages_accept_empty_argument_deltas_without_losing_initial_input() {
    for initial in [json!({}), json!({"path":"a"})] {
        let events = vec![
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"empty","name":"edit","input":initial}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":""}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
            json!({"type":"message_stop"}),
        ];
        let (credential, _body) = fixture(Protocol::Messages, events, false).await;
        let output = NativeTransport::new()
            .unwrap()
            .request(input(), credential)
            .await
            .unwrap();
        assert_eq!(output.calls.len(), 1);
        assert_eq!(output.calls[0].input, initial);
    }
}

#[tokio::test]
async fn messages_reject_empty_argument_deltas_for_unknown_blocks() {
    let (credential, _body) = fixture(Protocol::Messages, vec![
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":""}}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ], false).await;
    let result = NativeTransport::new()
        .unwrap()
        .request(input(), credential)
        .await;
    assert!(matches!(result, Err(Error::Model { code, .. }) if code == "unknown_content_block"));
}

#[tokio::test]
async fn messages_reject_nonempty_malformed_arguments_after_empty_delta() {
    let events = vec![
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"invalid","name":"edit","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{"}}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ];
    let (credential, _body) = fixture(Protocol::Messages, events, false).await;
    let result = NativeTransport::new()
        .unwrap()
        .request(input(), credential)
        .await;
    assert!(matches!(result, Err(Error::Model { code, .. }) if code == "invalid_tool_arguments"));
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

#[tokio::test]
async fn subscription_oauth_emits_native_attribution_without_changing_identity_or_cache_boundaries()
{
    let (mut credential, body) = authenticated_fixture(Protocol::Messages, vec![
        json!({"type":"message_start","message":{"usage":{"input_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"ready"}}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
        json!({"type":"message_stop"}),
    ], false, Authentication::AnthropicOauth, Vec::new()).await;
    credential.route.subscription = true;
    let mut request = input();
    request.reasoning = Some("medium".into());
    assert_eq!(
        NativeTransport::new()
            .unwrap()
            .request(request, credential)
            .await
            .unwrap()
            .text,
        "ready"
    );
    let body = body.await.unwrap();
    let prefix = body["system"][0]["text"].as_str().unwrap();
    assert!(prefix.starts_with("x-anthropic-billing-header:"));
    assert!(prefix.contains(env!("CARGO_PKG_VERSION")));
    assert!(body["system"][0].get("cache_control").is_none());
    assert_eq!(body["system"][1]["text"], "instructions");
    assert_eq!(body["system"][1]["cache_control"]["type"], "ephemeral");
    assert_eq!(body["output_config"]["effort"], "medium");
    assert!(!body.to_string().contains("built on"));
}
#[test]
fn subscription_oauth_requires_its_exact_protocol_and_preserves_attribution_without_system_text() {
    let mut route = Route::subscription("chatgpt").unwrap();
    route.authentication = Authentication::AnthropicOauth;
    assert!(route.validate_authentication().is_err());
    route.protocol = Protocol::Messages;
    route.subscription = false;
    assert!(route.validate_authentication().is_err());
    route.subscription = true;
    assert!(route.validate_authentication().is_ok());
    let mut request = input();
    Arc::make_mut(&mut request.messages).retain(|m| m.role != Role::System);
    let credential = Credential {
        connection_id: "fixture".into(),
        access_token: "unused".into(),
        authorization_generation: 0,
        route,
    };
    let fingerprint = request::fingerprint(&request, &credential);
    let body = request::body(&request, &credential).unwrap();
    assert_eq!(body["system"].as_array().unwrap().len(), 1);
    assert!(
        body["system"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("x-anthropic-billing-header:")
    );
    Arc::make_mut(&mut request.messages).push(Message::text(Role::User, "next turn"));
    assert_eq!(request::fingerprint(&request, &credential), fingerprint);
}

#[tokio::test]
async fn explicit_model_reasoning_capabilities_reject_unsupported_levels_before_sending() {
    let (mut credential, _body) = fixture(Protocol::Messages, vec![], false).await;
    credential.route.allowed_reasoning = Some(vec!["medium".into()]);
    let mut request = input();
    request.reasoning = Some("high".into());
    let result = NativeTransport::new()
        .unwrap()
        .request(request, credential)
        .await;
    assert!(matches!(result,Err(Error::Model{code,..}) if code=="unsupported_reasoning_level"));
}
#[tokio::test]
async fn tool_only_streams_report_first_content_and_provider_wait_separately() {
    let events = vec![
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call","name":"edit","input":{}}}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ];
    let (credential, _body) = fixture(Protocol::Messages, events, false).await;
    let seen = Arc::new(std::sync::Mutex::new(vec![]));
    let capture = seen.clone();
    let output = NativeTransport::new()
        .unwrap()
        .with_observer(Arc::new(move |attempt| {
            capture.lock().unwrap().push(attempt)
        }))
        .request(input(), credential)
        .await
        .unwrap();
    assert!(output.usage.first_token_ms.is_none());
    let records = seen.lock().unwrap();
    let timing = &records[0].timing;
    assert!(timing.first_content_ms.is_some());
    assert!(timing.first_text_ms.is_none());
    assert!(timing.headers_ms.is_some());
    assert!(timing.preparation_ms.is_some());
    assert!(timing.stream_ms.is_some());
}

#[tokio::test]
async fn registered_plan_route_keeps_public_responses_contract_and_selected_effort() {
    let (mut credential, _body) = fixture(Protocol::Responses, vec![], false).await;
    credential.route = Route::subscription("chatgpt").unwrap();
    assert_eq!(
        credential.route.endpoint,
        "https://api.openai.com/v1/responses"
    );
    let mut request = input();
    request.reasoning = Some("medium".into());
    let body = super::request::body(&request, &credential).unwrap();
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert!(body["input"].is_array());
    assert!(body["instructions"].is_string());
    assert_eq!(body["reasoning"]["effort"], "medium");
    assert_eq!(body["tools"][0]["type"], "namespace");
    assert_eq!(body["tools"][0]["name"], "zuse");
    for field in [
        "max_output_tokens",
        "previous_response_id",
        "temperature",
        "top_p",
        "prompt",
        "text",
    ] {
        assert!(body.get(field).is_none(), "unsupported plan field: {field}");
    }
    assert!(
        !body["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "system")
    );
}

#[tokio::test]
async fn mixed_tool_text_and_images_survive_supported_provider_translations() {
    let output = crate::ToolOutput::Mixed {
        r#type: "mixed".into(),
        parts: vec![
            json!({"type":"text","text":"Screenshot from browser"}),
            json!({"type":"file","mediaType":"image/png","data":{"type":"data","data":"aGVsbG8="}}),
        ],
    };
    assert!(output.validate());
    let encoded = serde_json::to_vec(&output).unwrap();
    let restored: crate::ToolOutput = serde_json::from_slice(&encoded).unwrap();
    assert!(restored.validate());
    for protocol in [
        Protocol::Responses,
        Protocol::Messages,
        Protocol::GenerateContent,
        Protocol::ChatCompletions,
    ] {
        let (mut credential, _body) = fixture(protocol, vec![], false).await;
        let mut request = input();
        let call = crate::Call {
            id: "image-call".into(),
            name: "edit".into(),
            input: json!({}),
        };
        Arc::make_mut(&mut request.messages).push(Message::tool(&call, &output));
        let body = super::request::body(&request, &credential);
        if protocol == Protocol::ChatCompletions {
            assert!(
                matches!(body,Err(Error::Model{code,..}) if code=="unsupported_image_tool_result")
            );
            continue;
        }
        let body = body.unwrap().to_string();
        assert!(body.contains("Screenshot from browser"));
        assert!(body.contains("aGVsbG8="));
        credential.route.capabilities.images = false;
        assert!(
            matches!(super::request::body(&request,&credential),Err(Error::Model{code,..}) if code=="unsupported_images")
        );
    }
}

#[tokio::test]
async fn local_codex_subscription_sends_workspace_header_and_omits_api_output_limit() {
    let (credential, body) = authenticated_fixture(Protocol::Responses,
        vec![json!({"type":"response.output_text.delta","delta":"hello"}),json!({"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1}}})],
        false, Authentication::CodexOauth, vec![]).await;
    NativeTransport::new()
        .unwrap()
        .request(input(), credential)
        .await
        .unwrap();
    let body = body.await.unwrap();
    assert!(body.get("max_output_tokens").is_none());
    assert_eq!(body["store"], false);
    assert_eq!(model_name("local/codex/gpt-test"), "gpt-test");
    assert_eq!(model_name("local/claude/claude-test"), "claude-test");
}

#[tokio::test]
async fn local_grok_uses_cli_proxy_authentication() {
    let (credential, body) = authenticated_fixture(
        Protocol::ChatCompletions,
        vec![json!({"choices":[{"delta":{"content":"hello"},"finish_reason":"stop"}]})],
        true,
        Authentication::GrokOauth,
        vec![],
    )
    .await;
    let output = NativeTransport::new()
        .unwrap()
        .request(input(), credential)
        .await
        .unwrap();
    assert_eq!(output.text, "hello");
    assert_eq!(body.await.unwrap()["model"], "fixture");
}
