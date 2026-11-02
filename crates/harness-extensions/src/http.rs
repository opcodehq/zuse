//! MCP binding over the workspace HTTP pool. Protocol lifecycle stays in the SDK.
use futures::{StreamExt, stream::BoxStream};
use http::{HeaderName, HeaderValue};
use rmcp::{
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    transport::streamable_http_client::{
        HttpStatusError, SseError, StreamableHttpClient, StreamableHttpError,
        StreamableHttpPostResponse,
    },
};
use sse_stream::{Sse, SseStream};
use std::{collections::HashMap, sync::Arc};
type Headers = HashMap<HeaderName, HeaderValue>;
type Failure = StreamableHttpError<std::io::Error>;
fn fail(message: &'static str) -> Failure {
    Failure::Io(std::io::Error::other(message))
}
#[derive(Clone)]
pub(crate) struct Http {
    pub client: reqwest::Client,
    pub uri: Arc<str>,
}
impl Http {
    fn request(
        &self,
        method: reqwest::Method,
        uri: &str,
        session: Option<Arc<str>>,
        auth: Option<String>,
        headers: Headers,
    ) -> Result<reqwest::RequestBuilder, Failure> {
        if uri != self.uri.as_ref() {
            return Err(fail("MCP endpoint changed"));
        }
        let mut request = self
            .client
            .request(method, uri)
            .header("accept", "application/json, text/event-stream");
        if let Some(session) = session {
            request = request.header("mcp-session-id", session.as_ref());
        }
        if let Some(auth) = auth {
            request = request.bearer_auth(auth);
        }
        for (name, value) in headers {
            request = request.header(name, value);
        }
        Ok(request)
    }
    async fn response(request: reqwest::RequestBuilder) -> Result<reqwest::Response, Failure> {
        let response = request
            .send()
            .await
            .map_err(|_| fail("MCP HTTP connection failed"))?;
        if !response.status().is_success() {
            return Err(Failure::HttpStatus(HttpStatusError::new(
                response.status().as_u16(),
                None,
            )));
        }
        Ok(response)
    }
    fn events(
        response: reqwest::Response,
        max: usize,
    ) -> BoxStream<'static, Result<Sse, SseError>> {
        let mut bytes = 0usize;
        let mut event = 0usize;
        let mut line = 0usize;
        let mut cr = false;
        let stream = response.bytes_stream().map(move |chunk| {
            let chunk = chunk.map_err(|_| std::io::Error::other("MCP SSE disconnected"))?;
            bytes = bytes.saturating_add(chunk.len());
            if bytes > 4 * 1024 * 1024 {
                return Err(std::io::Error::other("MCP stream exceeds 4 MiB"));
            }
            for &byte in &chunk {
                event = event.saturating_add(1);
                if event > max.min(1024 * 1024) {
                    return Err(std::io::Error::other("MCP event too large"));
                }
                if byte == b'\n' {
                    if line == 0 || (line == 1 && cr) {
                        event = 0;
                    }
                    line = 0;
                } else {
                    line += 1;
                }
                cr = byte == b'\r';
            }
            Ok(chunk)
        });
        SseStream::from_bytes_stream(stream).boxed()
    }
}
impl StreamableHttpClient for Http {
    type Error = std::io::Error;
    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session: Option<Arc<str>>,
        auth: Option<String>,
        headers: Headers,
    ) -> Result<StreamableHttpPostResponse, Failure> {
        self.post_message_with_max_sse_event_size(uri, message, session, auth, headers, 1024 * 1024)
            .await
    }
    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session: Option<Arc<str>>,
        auth: Option<String>,
        headers: Headers,
        max: usize,
    ) -> Result<StreamableHttpPostResponse, Failure> {
        let body = serde_json::to_vec(&message).map_err(|_| fail("Invalid MCP request"))?;
        if body.len() > 1024 * 1024 {
            return Err(fail("MCP request exceeds 1 MiB"));
        }
        let response = Self::response(
            self.request(reqwest::Method::POST, &uri, session, auth, headers)?
                .header("content-type", "application/json")
                .body(body),
        )
        .await?;
        if response.status().as_u16() == 202 || response.content_length() == Some(0) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        if session.as_ref().is_some_and(|s| s.len() > 4096) {
            return Err(fail("MCP session ID too large"));
        }
        let kind = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_owned();
        if kind == "text/event-stream" {
            return Ok(StreamableHttpPostResponse::Sse(
                Self::events(response, max),
                session,
            ));
        }
        if kind != "application/json" {
            return Err(fail("Unsupported MCP content type"));
        }
        let mut stream = response.bytes_stream();
        let mut data = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| fail("MCP body disconnected"))?;
            if data.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err(fail("MCP response too large"));
            }
            data.extend_from_slice(&chunk);
        }
        let message: ServerJsonRpcMessage =
            serde_json::from_slice(&data).map_err(|_| fail("Invalid MCP response"))?;
        Ok(StreamableHttpPostResponse::Json(message, session))
    }
    async fn delete_session(
        &self,
        uri: Arc<str>,
        session: Arc<str>,
        auth: Option<String>,
        headers: Headers,
    ) -> Result<(), Failure> {
        Self::response(self.request(
            reqwest::Method::DELETE,
            &uri,
            Some(session),
            auth,
            headers,
        )?)
        .await?;
        Ok(())
    }
    async fn get_stream(
        &self,
        uri: Arc<str>,
        session: Option<Arc<str>>,
        last: Option<String>,
        auth: Option<String>,
        headers: Headers,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, Failure> {
        self.get_stream_with_max_sse_event_size(uri, session, last, auth, headers, 1024 * 1024)
            .await
    }
    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session: Option<Arc<str>>,
        last: Option<String>,
        auth: Option<String>,
        headers: Headers,
        max: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, Failure> {
        let mut request = self.request(reqwest::Method::GET, &uri, session, auth, headers)?;
        if let Some(last) = last {
            request = request.header("last-event-id", last);
        }
        let response = request
            .send()
            .await
            .map_err(|_| fail("MCP stream connection failed"))?;
        if response.status().as_u16() == 405 {
            return Err(Failure::ServerDoesNotSupportSse);
        }
        if !response.status().is_success() {
            return Err(Failure::HttpStatus(HttpStatusError::new(
                response.status().as_u16(),
                None,
            )));
        }
        Ok(Self::events(response, max))
    }
}
