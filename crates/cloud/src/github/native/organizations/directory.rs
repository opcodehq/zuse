//! WorkOS transport is deliberately isolated from policy and SQL. Pagination is
//! complete or fails; a partial roster must never admit extra paid seats.
use super::*;
use futures::StreamExt;
use reqwest::{Method, Url};
pub struct WorkosDirectory {
    http: reqwest::Client,
    key: Zeroizing<String>,
    origin: Url,
}
impl WorkosDirectory {
    pub fn from_config(config: &crate::config::Config) -> Result<Self, ApiError> {
        let key = config
            .workos_api_key
            .clone()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| ApiError::unavailable("organizations_not_configured"))?;
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| ApiError::unavailable("organizations_unavailable"))?,
            key: Zeroizing::new(key),
            origin: Url::parse("https://api.workos.com/").unwrap(),
        })
    }
    async fn request(
        &self,
        method: Method,
        url: Url,
        body: Option<Value>,
    ) -> Result<Value, ApiError> {
        if url.origin() != self.origin.origin() {
            return Err(ApiError::unavailable("organizations_invalid_origin"));
        }
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(self.key.as_str());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| ApiError::unavailable("organizations_unavailable"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(match status.as_u16() {
                404 => ApiError::not_found("organization_resource_not_found"),
                409 => ApiError::conflict("organization_conflict"),
                400 | 422 => ApiError::bad_request("organization_invalid_request"),
                _ => ApiError::unavailable("organizations_unavailable"),
            });
        }
        if status.as_u16() == 204 {
            return Ok(Value::Null);
        }
        let mut bytes = vec![];
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|_| ApiError::unavailable("organizations_invalid_response"))?;
            if bytes.len() + chunk.len() > 1024 * 1024 {
                return Err(ApiError::unavailable("organizations_invalid_response"));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::unavailable("organizations_invalid_response"))
    }
    fn endpoint(&self, segments: &[&str]) -> Url {
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .expect("constant hierarchical origin")
            .clear()
            .extend(segments);
        url
    }
    async fn list<T: serde::de::DeserializeOwned>(
        &self,
        resource: &str,
        org: &str,
    ) -> Result<Vec<T>, ApiError> {
        let mut after = None::<String>;
        let mut seen = BTreeSet::new();
        let mut rows = vec![];
        loop {
            let mut url = self.endpoint(&["user_management", resource]);
            {
                let mut q = url.query_pairs_mut();
                q.append_pair("organization_id", org)
                    .append_pair("limit", "100");
                if let Some(cursor) = &after {
                    q.append_pair("after", cursor);
                }
            }
            let page = self.request(Method::GET, url, None).await?;
            let entries = page["data"]
                .as_array()
                .ok_or_else(|| ApiError::unavailable("organizations_invalid_response"))?;
            if entries.len() > 100 {
                return Err(ApiError::unavailable("organizations_invalid_response"));
            }
            for entry in entries {
                rows.push(
                    serde_json::from_value(entry.clone())
                        .map_err(|_| ApiError::unavailable("organizations_invalid_response"))?,
                );
            }
            let cursor = page
                .get("list_metadata")
                .and_then(|m| m.get("after"))
                .ok_or_else(|| ApiError::unavailable("organizations_invalid_response"))?;
            after = match cursor {
                Value::Null => None,
                Value::String(s) if !s.is_empty() && s.len() <= 4096 => Some(s.clone()),
                _ => return Err(ApiError::unavailable("organizations_invalid_response")),
            };
            if let Some(cursor) = &after {
                if seen.len() >= 100 || !seen.insert(cursor.clone()) {
                    return Err(ApiError::unavailable("organization_roster_too_large"));
                }
            } else {
                return Ok(rows);
            }
        }
    }
}
#[async_trait]
impl Directory for WorkosDirectory {
    async fn members(&self, org: &str) -> Result<Vec<Member>, ApiError> {
        self.list("organization_memberships", org).await
    }
    async fn invitations(&self, org: &str) -> Result<Vec<Invitation>, ApiError> {
        self.list("invitations", org).await
    }
    async fn admit(
        &self,
        org: &str,
        actor: &str,
        previous: Option<&str>,
    ) -> Result<Member, ApiError> {
        let (method, url, body) = if let Some(id) = previous {
            (
                Method::PUT,
                self.endpoint(&[
                    "user_management",
                    "organization_memberships",
                    id,
                    "reactivate",
                ]),
                json!({}),
            )
        } else {
            (
                Method::POST,
                self.endpoint(&["user_management", "organization_memberships"]),
                json!({"organization_id":org,"user_id":actor,"role_slug":"member"}),
            )
        };
        serde_json::from_value(self.request(method, url, Some(body)).await?)
            .map_err(|_| ApiError::unavailable("organizations_invalid_response"))
    }
    async fn deactivate(&self, id: &str) -> Result<(), ApiError> {
        self.request(
            Method::PUT,
            self.endpoint(&[
                "user_management",
                "organization_memberships",
                id,
                "deactivate",
            ]),
            Some(json!({})),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Query, State},
        routing::get,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn workos_pagination_is_complete_or_fails_closed() {
        let mode = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app=Router::new().route("/user_management/organization_memberships",get(|State(mode):State<Arc<AtomicUsize>>,Query(query):Query<std::collections::HashMap<String,String>>|async move{
   assert_eq!(query.get("organization_id").map(String::as_str),Some("team"));assert_eq!(query.get("limit").map(String::as_str),Some("100"));
   let data=json!([{"id":if query.contains_key("after"){"second"}else{"first"},"user_id":"actor","organization_id":"team","status":"active","directory_managed":false,"role":{"slug":"member"}}]);
   Json(match mode.load(Ordering::SeqCst){1=>json!({"data":data,"list_metadata":{"after":"repeated"}}),2=>json!({"data":data}),_=>json!({"data":data,"list_metadata":{"after":if query.contains_key("after"){Value::Null}else{json!("next")}}})})
  })).with_state(mode.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let directory = WorkosDirectory {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            key: Zeroizing::new("isolated-fixture".into()),
            origin: Url::parse(&format!("http://{address}/")).unwrap(),
        };
        let members = directory.members("team").await.unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[1].id, "second");
        mode.store(1, Ordering::SeqCst);
        assert_eq!(
            directory.members("team").await.err().unwrap().code,
            "organization_roster_too_large"
        );
        mode.store(2, Ordering::SeqCst);
        assert_eq!(
            directory.members("team").await.err().unwrap().code,
            "organizations_invalid_response"
        );
        let url = directory.endpoint(&[
            "user_management",
            "organization_memberships",
            "id/../bad",
            "reactivate",
        ]);
        assert!(url.path().contains("id%2F..%2Fbad"));
        server.abort();
    }
}
