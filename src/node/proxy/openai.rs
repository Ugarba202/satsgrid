use crate::common::error::Result;
use futures_util::Stream;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

#[derive(Clone)]
pub struct OpenAiProxy {
    client: Client,
    upstream_url: String,
}

impl OpenAiProxy {
    pub fn new(upstream_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            upstream_url: upstream_url.into(),
        }
    }

    pub fn new_with_timeout(upstream_url: impl Into<String>, timeout: Duration) -> Self {
        Self {
            client: Client::builder()
                .timeout(timeout)
                .build()
                .unwrap_or_default(),
            upstream_url: upstream_url.into(),
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn upstream_url(&self) -> &str {
        &self.upstream_url
    }

    pub async fn forward_chat_completion(&self, request: Value) -> Result<Value> {
        self.forward_chat_completion_with_headers(request, http::HeaderMap::new())
            .await
    }

    pub async fn forward_chat_completion_with_headers(
        &self,
        request: Value,
        headers: http::HeaderMap,
    ) -> Result<Value> {
        let url = format!("{}/v1/chat/completions", self.upstream_url);

        let mut req_headers = reqwest::header::HeaderMap::new();
        for (k, v) in headers.iter() {
            let key_str = k.as_str().to_lowercase();
            // Drop authorization, but allow other safe headers
            if key_str != "authorization" {
                req_headers.insert(k.clone(), v.clone());
            }
        }

        // Ensure Content-Type is set
        if !req_headers.contains_key("content-type") {
            req_headers.insert("content-type", "application/json".parse().unwrap());
        }

        let resp = self
            .client
            .post(&url)
            .headers(req_headers)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    crate::common::error::Error::Upstream(format!("Upstream timeout: {}", e))
                } else {
                    crate::common::error::Error::Upstream(format!(
                        "Upstream connection error: {}",
                        e
                    ))
                }
            })?;

        let status = resp.status();
        if !status.is_success() {
            let err_text = resp
                .text()
                .await
                .unwrap_or_else(|_| "Unknown upstream error".to_string());
            return Err(crate::common::error::Error::Upstream(format!(
                "Upstream error ({}): {}",
                status, err_text
            )));
        }

        resp.json::<Value>().await.map_err(|e| {
            crate::common::error::Error::Upstream(format!("Upstream JSON parse error: {}", e))
        })
    }

    pub async fn list_models(&self) -> Result<Value> {
        let url = format!("{}/v1/models", self.upstream_url);

        let resp = self.client.get(&url).send().await.map_err(|e| {
            if e.is_timeout() {
                crate::common::error::Error::Upstream(format!("Upstream timeout: {}", e))
            } else {
                crate::common::error::Error::Upstream(format!("Upstream connection error: {}", e))
            }
        })?;

        let status = resp.status();
        if !status.is_success() {
            let err_text = resp
                .text()
                .await
                .unwrap_or_else(|_| "Unknown upstream error".to_string());
            return Err(crate::common::error::Error::Upstream(format!(
                "Upstream error ({}): {}",
                status, err_text
            )));
        }

        resp.json::<Value>().await.map_err(|e| {
            crate::common::error::Error::Upstream(format!("Upstream JSON parse error: {}", e))
        })
    }

    pub async fn stream_chat_completion(
        &self,
        request: Value,
    ) -> Result<impl Stream<Item = Result<bytes::Bytes>>> {
        let url = format!("{}/v1/chat/completions", self.upstream_url);

        let resp = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                crate::common::error::Error::Upstream(format!("Upstream stream error: {}", e))
            })?;

        let status = resp.status();
        if !status.is_success() {
            return Err(crate::common::error::Error::Upstream(format!(
                "Upstream stream HTTP error: {}",
                status
            )));
        }

        use futures_util::StreamExt;
        let stream = resp
            .bytes_stream()
            .map(|result| result.map_err(|e| crate::common::error::Error::Upstream(e.to_string())));

        Ok(stream)
    }
}
