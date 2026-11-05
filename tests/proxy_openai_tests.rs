use satsgrid::node::proxy::openai::OpenAiProxy;
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_proxy_forwards_request() {
    let mock_server = MockServer::start().await;
    let proxy = OpenAiProxy::new(mock_server.uri());

    let req_body = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "Hello"}]
    });

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_json(&req_body))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "success"})))
        .expect(1)
        .mount(&mock_server)
        .await;

    let _response = proxy
        .forward_chat_completion(req_body)
        .await
        .expect("Failed to forward request");
}

#[tokio::test]
async fn test_proxy_forwards_required_upstream_headers() {
    let mock_server = MockServer::start().await;
    let proxy = OpenAiProxy::new(mock_server.uri());

    let req_body = json!({"model": "test"});

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Content-Type", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&mock_server)
        .await;

    let mut headers = http::HeaderMap::new();
    headers.insert(
        "Authorization",
        "L402 macaroon=\"base64...\", preimage=\"hex...\""
            .parse()
            .unwrap(),
    );

    headers.insert("Content-Type", "application/json".parse().unwrap());

    let _response = proxy
        .forward_chat_completion_with_headers(req_body, headers)
        .await
        .expect("Failed to forward request");
}

#[tokio::test]
async fn test_proxy_returns_upstream_response() {
    let mock_server = MockServer::start().await;
    let proxy = OpenAiProxy::new(mock_server.uri());

    let upstream_resp = json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "created": 1677652288,
        "model": "llama3.2",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hello there"
            },
            "finish_reason": "stop"
        }]
    });

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&upstream_resp))
        .mount(&mock_server)
        .await;

    let response = proxy
        .forward_chat_completion(json!({"model": "test"}))
        .await
        .expect("Failed to forward request");

    assert_eq!(response, upstream_resp);
}

#[tokio::test]
async fn test_proxy_maps_upstream_error() {
    let mock_server = MockServer::start().await;
    let proxy = OpenAiProxy::new(mock_server.uri());

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .mount(&mock_server)
        .await;

    let result = proxy
        .forward_chat_completion(json!({"model": "test"}))
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_proxy_handles_upstream_timeout() {
    let mock_server = MockServer::start().await;
    let proxy = OpenAiProxy::new_with_timeout(mock_server.uri(), Duration::from_millis(100));

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(300)))
        .mount(&mock_server)
        .await;

    let result = proxy
        .forward_chat_completion(json!({"model": "test"}))
        .await;

    assert!(result.is_err());
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.to_lowercase().contains("timeout") || err_str.to_lowercase().contains("deadline")
    );
}

#[tokio::test]
async fn test_proxy_supports_streaming() {
    let mock_server = MockServer::start().await;
    let proxy = OpenAiProxy::new(mock_server.uri());

    let sse_body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n\
                    data: [DONE]\n\n";

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&mock_server)
        .await;

    let mut stream = proxy
        .stream_chat_completion(json!({"model": "test", "stream": true}))
        .await
        .expect("Failed to start stream");

    use futures_util::StreamExt;

    let mut chunks = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("Stream error");
        chunks.push(String::from_utf8_lossy(&chunk).into_owned());
    }

    let full_output = chunks.join("");
    assert!(full_output.contains("data: [DONE]"));
}

#[tokio::test]
async fn test_proxy_list_models_success() {
    let mock_server = MockServer::start().await;
    let proxy = OpenAiProxy::new(mock_server.uri());

    let models_resp = json!({
        "object": "list",
        "data": [
            {"id": "deepseek-coder:6.7b", "object": "model", "created": 1700000000, "owned_by": "ollama"},
            {"id": "llama3.2:latest", "object": "model", "created": 1700000001, "owned_by": "ollama"}
        ]
    });

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&models_resp))
        .expect(1)
        .mount(&mock_server)
        .await;

    let response = proxy.list_models().await.expect("Failed to list models");
    assert_eq!(response, models_resp);
}

#[tokio::test]
async fn test_proxy_list_models_error() {
    let mock_server = MockServer::start().await;
    let proxy = OpenAiProxy::new(mock_server.uri());

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(502).set_body_string("Bad Gateway"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let result = proxy.list_models().await;
    assert!(result.is_err());
}
