use async_trait::async_trait;
use latch_core::BackendKind;
use serde_json::{json, Value};

pub type DetectResult<T> = Result<T, DetectError>;

#[derive(Debug)]
pub enum DetectError {
    Http(String),
}

impl std::fmt::Display for DetectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DetectError::Http(v) => f.write_str(v),
        }
    }
}

impl std::error::Error for DetectError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[async_trait]
pub trait HttpProbe: Send + Sync {
    async fn get(&self, path: &str) -> DetectResult<ProbeResponse>;
    async fn post_json(&self, path: &str, body: Value) -> DetectResult<ProbeResponse>;
}

pub struct ReqwestProbe {
    client: reqwest::Client,
    base_url: String,
}

impl ReqwestProbe {
    pub fn new(base_url: impl Into<String>, client: reqwest::Client) -> Self {
        Self {
            client,
            base_url: base_url.into(),
        }
    }

    fn make_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }
}

#[async_trait]
impl HttpProbe for ReqwestProbe {
    async fn get(&self, path: &str) -> DetectResult<ProbeResponse> {
        let resp = self
            .client
            .get(self.make_url(path))
            .send()
            .await
            .map_err(|e| DetectError::Http(e.to_string()))?;
        to_probe_response(resp).await
    }

    async fn post_json(&self, path: &str, body: Value) -> DetectResult<ProbeResponse> {
        let resp = self
            .client
            .post(self.make_url(path))
            .json(&body)
            .send()
            .await
            .map_err(|e| DetectError::Http(e.to_string()))?;
        to_probe_response(resp).await
    }
}

async fn to_probe_response(resp: reqwest::Response) -> DetectResult<ProbeResponse> {
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect::<Vec<_>>();
    let body = resp
        .text()
        .await
        .map_err(|e| DetectError::Http(e.to_string()))?;
    Ok(ProbeResponse {
        status,
        headers,
        body,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct DetectionReport {
    pub backend: BackendKind,
    pub confidence: f32,
    pub reason: String,
}

pub async fn detect_backend(probe: &dyn HttpProbe) -> DetectResult<DetectionReport> {
    if let Ok(metrics) = probe.get("/metrics").await {
        let lower = metrics.body.to_lowercase();
        if lower.contains("vllm:") {
            return Ok(DetectionReport {
                backend: BackendKind::DirectVllm,
                confidence: 0.98,
                reason: "metrics endpoint contains vllm:*".to_string(),
            });
        }
        if lower.contains("tgi_") {
            return Ok(DetectionReport {
                backend: BackendKind::DirectTgi,
                confidence: 0.98,
                reason: "metrics endpoint contains tgi_*".to_string(),
            });
        }
    }

    if let Ok(info) = probe.get("/get_model_info").await {
        if info.status == 200 && info.body.contains("model_path") {
            return Ok(DetectionReport {
                backend: BackendKind::DirectSglang,
                confidence: 0.95,
                reason: "get_model_info returns model_path".to_string(),
            });
        }
    }

    if let Ok(tokenize) = probe.post_json(
        "/tokenize",
        json!({"model":"default","prompt":"hello world"}),
    ).await
    {
        if tokenize.status == 200 && tokenize.body.contains("token") {
            return Ok(DetectionReport {
                backend: BackendKind::DirectVllm,
                confidence: 0.9,
                reason: "tokenize endpoint returned tokens".to_string(),
            });
        }
    }

    if let Ok(models) = probe.get("/v1/models").await {
        let lower = models.body.to_lowercase();
        if lower.contains("\"owned_by\":\"openai\"") || lower.contains("\"owned_by\":\"anthropic\"")
        {
            return Ok(DetectionReport {
                backend: BackendKind::Cloud,
                confidence: 0.8,
                reason: "model ownership indicates cloud provider".to_string(),
            });
        }
    }

    if let Ok(test) = probe
        .post_json(
            "/v1/chat/completions",
            json!({
                "model": "default",
                "messages": [{"role":"user","content":"hi"}],
                "max_tokens": 1,
                "best_of": 2
            }),
        )
        .await
    {
        if test.status == 200 {
            return Ok(DetectionReport {
                backend: BackendKind::DirectVllm,
                confidence: 0.75,
                reason: "best_of accepted by backend".to_string(),
            });
        }
    }

    Ok(DetectionReport {
        backend: BackendKind::Unknown,
        confidence: 0.3,
        reason: "no definitive direct-engine fingerprints found".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{detect_backend, DetectResult, HttpProbe, ProbeResponse};
    use async_trait::async_trait;
    use latch_core::BackendKind;
    use serde_json::Value;
    use std::collections::HashMap;

    struct MockProbe {
        get_map: HashMap<String, ProbeResponse>,
        post_map: HashMap<String, ProbeResponse>,
    }

    impl MockProbe {
        fn new() -> Self {
            Self {
                get_map: HashMap::new(),
                post_map: HashMap::new(),
            }
        }

        fn with_get(mut self, path: &str, status: u16, body: &str) -> Self {
            self.get_map.insert(
                path.to_string(),
                ProbeResponse {
                    status,
                    headers: Vec::new(),
                    body: body.to_string(),
                },
            );
            self
        }

        fn with_post(mut self, path: &str, status: u16, body: &str) -> Self {
            self.post_map.insert(
                path.to_string(),
                ProbeResponse {
                    status,
                    headers: Vec::new(),
                    body: body.to_string(),
                },
            );
            self
        }
    }

    #[async_trait]
    impl HttpProbe for MockProbe {
        async fn get(&self, path: &str) -> DetectResult<ProbeResponse> {
            Ok(self.get_map.get(path).cloned().unwrap_or(ProbeResponse {
                status: 404,
                headers: Vec::new(),
                body: String::new(),
            }))
        }

        async fn post_json(&self, path: &str, _body: Value) -> DetectResult<ProbeResponse> {
            Ok(self.post_map.get(path).cloned().unwrap_or(ProbeResponse {
                status: 404,
                headers: Vec::new(),
                body: String::new(),
            }))
        }
    }

    #[tokio::test]
    async fn detects_vllm_from_metrics() {
        let probe = MockProbe::new().with_get("/metrics", 200, "vllm:gpu_cache_usage_perc 0.5");
        let report = detect_backend(&probe).await.expect("detect ok");
        assert_eq!(report.backend, BackendKind::DirectVllm);
    }

    #[tokio::test]
    async fn detects_sglang_from_model_info() {
        let probe = MockProbe::new().with_get("/get_model_info", 200, r#"{"model_path":"..."}"#);
        let report = detect_backend(&probe).await.expect("detect ok");
        assert_eq!(report.backend, BackendKind::DirectSglang);
    }

    #[tokio::test]
    async fn detects_cloud_from_models_owned_by() {
        let probe = MockProbe::new()
            .with_get("/v1/models", 200, r#"{"data":[{"owned_by":"openai"}]}"#);
        let report = detect_backend(&probe).await.expect("detect ok");
        assert_eq!(report.backend, BackendKind::Cloud);
    }

    #[tokio::test]
    async fn detects_vllm_from_best_of_acceptance() {
        let probe = MockProbe::new().with_post("/v1/chat/completions", 200, r#"{"id":"ok"}"#);
        let report = detect_backend(&probe).await.expect("detect ok");
        assert_eq!(report.backend, BackendKind::DirectVllm);
    }
}
