//! Shared bounded evaluation transport. Callers own question schemas and answer validation.
use serde::{Deserialize, Serialize};
use std::{io::Read, sync::OnceLock, time::Duration};

#[derive(Clone, Copy)]
pub struct EvaluationPolicy {
    pub deadline: Duration,
    pub response_limit: usize,
    pub label: &'static str,
}
impl EvaluationPolicy {
    pub const DECISION: Self = Self {
        deadline: Duration::from_secs(10),
        response_limit: 128 * 1024,
        label: "Decision evaluation",
    };
    pub const APPRAISAL: Self = Self {
        deadline: Duration::from_millis(1500),
        response_limit: 16 * 1024,
        label: "Appraisal",
    };
    pub const MEMORY: Self = Self {
        deadline: Duration::from_secs(10),
        response_limit: 16 * 1024,
        label: "Memory triage",
    };
}
fn resolve_key(config: &DecisionConfig, lookup: impl Fn(&str) -> String) -> String {
    if !config.session_key.is_empty() {
        return config.session_key.clone();
    }
    let name = config.key_env.trim();
    let key = lookup(name);
    if key.is_empty() && name == "AI_GATEWAY_API_KEY" {
        lookup("VERCEL_OIDC_TOKEN")
    } else {
        key
    }
}
fn decision_client() -> Result<&'static reqwest::blocking::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(10))
                .connect_timeout(Duration::from_secs(5))
                .pool_idle_timeout(Duration::from_secs(90))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "Cannot create decision evaluation client".into())
        })
        .as_ref()
        .map_err(Clone::clone)
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DecisionConfig {
    pub endpoint: String,
    pub model: String,
    pub key_env: String,
    #[serde(skip)]
    pub session_key: String,
}
impl std::fmt::Debug for DecisionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionConfig")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("key_env", &self.key_env)
            .field("session_key", &"[redacted]")
            .finish()
    }
}
impl Default for DecisionConfig {
    fn default() -> Self {
        Self {
            endpoint: "https://ai-gateway.vercel.sh/v4/ai/evaluation-model".into(),
            model: "typesafe-ai/jev".into(),
            key_env: "AI_GATEWAY_API_KEY".into(),
            session_key: String::new(),
        }
    }
}
/// Execute a bounded evaluation request using the shared configured credentials.
/// Must run on a worker; errors never contain response bodies or credentials.
pub fn evaluate(
    config: &DecisionConfig,
    policy: EvaluationPolicy,
    body: &serde_json::Value,
    cancelled: &dyn Fn() -> bool,
) -> Result<serde_json::Value, String> {
    if cancelled() || body.to_string().len() > 64 * 1024 {
        return Err("Decision request cancelled or oversized".into());
    }
    let url = reqwest::Url::parse(&config.endpoint).map_err(|_| "Invalid decision endpoint")?;
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if !(url.scheme() == "https" || (url.scheme() == "http" && local))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || config.model.trim().is_empty()
        || config.model.len() > 200
    {
        return Err(
            "Use an HTTPS decision endpoint (HTTP allowed on localhost) and an evaluation model ID"
                .into(),
        );
    }
    let key = resolve_key(config, |name| std::env::var(name).unwrap_or_default());
    if key.is_empty() && !local {
        return Err("Set a Vercel decision key or its environment variable".into());
    }
    let client = decision_client()?;
    let mut request = client
        .post(url)
        // Request-level timeout also bounds the complete response body.
        .timeout(policy.deadline)
        .header("ai-gateway-protocol-version", "0.0.1")
        .header("ai-evaluation-model-specification-version", "4")
        .header("ai-model-id", &config.model)
        .json(body);
    if !key.is_empty() {
        request = request.bearer_auth(key);
    }
    if cancelled() {
        return Err(format!("{} cancelled", policy.label));
    }
    let response = request
        .send()
        .map_err(|_| format!("{} connection failed or timed out", policy.label))?;
    if !response.status().is_success() {
        return Err(format!(
            "{} returned HTTP {}",
            policy.label,
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    response
        .take(policy.response_limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| format!("Cannot read {} response", policy.label.to_lowercase()))?;
    if cancelled() {
        return Err(format!("{} cancelled", policy.label));
    }
    if bytes.len() > policy.response_limit {
        return Err(format!("{} response exceeds limit", policy.label));
    }
    let data: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| format!("Invalid {} response", policy.label.to_lowercase()))?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
    };

    #[test]
    fn credentials_share_session_environment_and_gateway_oidc_precedence() {
        let mut config = DecisionConfig::default();
        let lookup = |name: &str| match name {
            "AI_GATEWAY_API_KEY" => "api-key".to_owned(),
            "VERCEL_OIDC_TOKEN" => "oidc-token".to_owned(),
            _ => String::new(),
        };
        assert_eq!(resolve_key(&config, lookup), "api-key");
        config.session_key = "session-key".into();
        assert_eq!(
            resolve_key(&config, |_| panic!("session key needs no environment")),
            "session-key"
        );
        config.session_key.clear();
        assert_eq!(
            resolve_key(&config, |name| if name == "VERCEL_OIDC_TOKEN" {
                "oidc-token".into()
            } else {
                String::new()
            }),
            "oidc-token"
        );
        config.key_env = "CUSTOM_DECISION_KEY".into();
        assert_eq!(resolve_key(&config, lookup), "");
    }

    #[test]
    fn transport_caps_bodies_and_never_surfaces_remote_error_text() {
        for (status, body, expected) in [
            (200, "012345678", "exceeds limit"),
            (200, "invalid", "Invalid"),
            (401, "private upstream content", "HTTP 401"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let config = DecisionConfig {
                endpoint: format!("http://{}/evaluation", listener.local_addr().unwrap()),
                session_key: "fixture-key".into(),
                ..Default::default()
            };
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if let Some(n) = line.to_lowercase().strip_prefix("content-length:") {
                        length = n.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                reader.read_exact(&mut vec![0; length]).unwrap();
                write!(stream, "HTTP/1.1 {status} Response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let policy = EvaluationPolicy {
                response_limit: 8,
                ..EvaluationPolicy::MEMORY
            };
            let error = evaluate(&config, policy, &serde_json::json!({}), &|| false).unwrap_err();
            server.join().unwrap();
            assert!(error.contains(expected), "{error}");
            assert!(!error.contains("fixture-key") && !error.contains("private upstream"));
        }
    }

    #[test]
    fn rejected_requests_never_connect() {
        let config = DecisionConfig {
            endpoint: "http://example.invalid/evaluation".into(),
            ..Default::default()
        };
        assert!(
            evaluate(
                &config,
                EvaluationPolicy::DECISION,
                &serde_json::json!({}),
                &|| false
            )
            .is_err()
        );
        assert!(
            evaluate(
                &DecisionConfig::default(),
                EvaluationPolicy::DECISION,
                &serde_json::json!({}),
                &|| true
            )
            .unwrap_err()
            .contains("cancelled")
        );
        assert!(
            evaluate(
                &DecisionConfig::default(),
                EvaluationPolicy::DECISION,
                &serde_json::json!("x".repeat(65536)),
                &|| false
            )
            .unwrap_err()
            .contains("oversized")
        );
    }
}
