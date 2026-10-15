use super::*;

#[derive(Debug, Deserialize)]
pub struct ConnectSession {
    pub token: String,
    pub expires_at: String,
}

#[derive(Debug, Deserialize)]
pub struct Connection {
    pub connection_id: String,
    pub provider_config_key: String,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct RecordsPage {
    #[serde(default)]
    pub records: Vec<Value>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

pub struct NangoClient {
    http: reqwest::Client,
    api_url: String,
    secret_key: String,
}

impl NangoClient {
    pub fn new(http: reqwest::Client, api_url: &str, secret_key: &str) -> Self {
        Self { http, api_url: api_url.trim_end_matches('/').to_string(), secret_key: secret_key.to_string() }
    }

    pub fn configured(&self) -> bool {
        !self.secret_key.is_empty()
    }

    async fn send<T: serde::de::DeserializeOwned>(&self, req: reqwest::RequestBuilder) -> Result<T, NangoError> {
        if !self.configured() {
            return Err(NangoError::NotConfigured);
        }
        let res = req
            .bearer_auth(&self.secret_key)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| NangoError::Unavailable(e.to_string()))?;
        let status = res.status();
        if status.is_success() {
            return res.json().await.map_err(|e| NangoError::Unavailable(format!("decode: {e}")));
        }
        let msg = format!("Nango returned {status}");
        Err(match status.as_u16() {
            429 | 500..=599 => NangoError::Unavailable(msg),
            // 424 = connection refresh exhausted: the user must reconnect.
            401 | 403 | 404 | 424 => NangoError::Connection(msg),
            _ => NangoError::Rejected(msg),
        })
    }

    /// Short-lived (≈30 min) session restricted to one integration and tagged with
    /// the workspace and user, so the resulting connection can be verified later.
    pub async fn create_connect_session(
        &self,
        workspace_id: &str,
        user_id: &str,
        user_email: &str,
        integration: &str,
    ) -> Result<ConnectSession, NangoError> {
        #[derive(Deserialize)]
        struct Envelope {
            data: ConnectSession,
        }
        let mut tags = json!({ "end_user_id": user_id, "organization_id": workspace_id });
        if !user_email.is_empty() {
            tags["end_user_email"] = json!(user_email);
        }
        let body = json!({ "tags": tags, "allowed_integrations": [integration] });
        let env: Envelope = self.send(self.http.post(format!("{}/connect/sessions", self.api_url)).json(&body)).await?;
        Ok(env.data)
    }

    pub async fn get_connection(&self, connection_id: &str, provider_config_key: &str) -> Result<Connection, NangoError> {
        self.send(
            self.http
                .get(format!("{}/connections/{}", self.api_url, urlencode(connection_id)))
                .query(&[("provider_config_key", provider_config_key)]),
        )
        .await
    }

    pub async fn delete_connection(&self, connection_id: &str, provider_config_key: &str) -> Result<(), NangoError> {
        let _: Value = self
            .send(
                self.http
                    .delete(format!("{}/connections/{}", self.api_url, urlencode(connection_id)))
                    .query(&[("provider_config_key", provider_config_key)]),
            )
            .await
            .or_else(|e| match e {
                // Some responses have an empty body; treat decode failure after 2xx as success.
                NangoError::Unavailable(m) if m.starts_with("decode") => Ok(Value::Null),
                other => Err(other),
            })?;
        Ok(())
    }

    /// Full Gmail message via Nango's proxy (the catalog sync only stores metadata).
    /// Retries once: right after a connection is created the proxy can briefly 403.
    pub async fn gmail_message_full(&self, connection_id: &str, provider_config_key: &str, id: &str) -> Result<Value, NangoError> {
        if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(NangoError::Rejected("invalid message id".into()));
        }
        let call = || {
            self.send::<Value>(
                self.http
                    .get(format!("{}/proxy/gmail/v1/users/me/messages/{id}", self.api_url))
                    .header("Connection-Id", connection_id)
                    .header("Provider-Config-Key", provider_config_key)
                    .query(&[("format", "full")]),
            )
        };
        match call().await {
            Err(NangoError::Connection(_) | NangoError::Unavailable(_)) => {
                tokio::time::sleep(Duration::from_millis(800)).await;
                call().await
            }
            other => other,
        }
    }

    /// Generic Gmail call through Nango's proxy (`path` is relative to `users/me/`).
    pub async fn gmail_call(
        &self,
        method: reqwest::Method,
        connection_id: &str,
        provider_config_key: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, NangoError> {
        let mut req = self
            .http
            .request(method, format!("{}/proxy/gmail/v1/users/me/{path}", self.api_url))
            .header("Connection-Id", connection_id)
            .header("Provider-Config-Key", provider_config_key);
        if let Some(b) = body {
            req = req.json(b);
        }
        self.send(req).await
    }

    /// One page of Gmail message IDs matching `q` (e.g. `newer_than:14d`).
    pub async fn gmail_list_ids(
        &self,
        connection_id: &str,
        provider_config_key: &str,
        q: &str,
        page_token: Option<&str>,
    ) -> Result<(Vec<String>, Option<String>), NangoError> {
        let mut query: Vec<(&str, &str)> = vec![("q", q), ("maxResults", "100")];
        if let Some(t) = page_token {
            query.push(("pageToken", t));
        }
        let page: Value = self
            .send(
                self.http
                    .get(format!("{}/proxy/gmail/v1/users/me/messages", self.api_url))
                    .header("Connection-Id", connection_id)
                    .header("Provider-Config-Key", provider_config_key)
                    .query(&query),
            )
            .await?;
        let ids = page
            .get("messages")
            .and_then(Value::as_array)
            .map(|m| m.iter().filter_map(|x| x.get("id").and_then(Value::as_str).map(str::to_string)).collect())
            .unwrap_or_default();
        Ok((ids, page.get("nextPageToken").and_then(Value::as_str).map(str::to_string)))
    }

    pub async fn list_records(
        &self,
        connection_id: &str,
        provider_config_key: &str,
        model: &str,
        cursor: Option<&str>,
        modified_after: Option<&str>,
    ) -> Result<RecordsPage, NangoError> {
        let limit = PAGE_LIMIT.to_string();
        let mut query: Vec<(&str, &str)> = vec![("model", model), ("limit", &limit)];
        if let Some(c) = cursor {
            query.push(("cursor", c));
        } else if let Some(m) = modified_after {
            query.push(("modified_after", m));
        }
        self.send(
            self.http
                .get(format!("{}/records", self.api_url))
                .header("Connection-Id", connection_id)
                .header("Provider-Config-Key", provider_config_key)
                .query(&query),
        )
        .await
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
