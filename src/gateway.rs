use crate::{
    config::{AppConfig, PrivacyConfig, ProviderProfile},
    ledger::{Ledger, ProviderAttempt, ReceiptRecord, RecentRequest, RequestMeta, Stats},
    transform::estimate_tokens,
};
use anyhow::{Context, Result};
use async_stream::stream;
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Json, Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::net::TcpListener;
use tokio::sync::{RwLock, watch};
use tokio::time::timeout;
use tracing::{info, warn};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub ledger: Ledger,
    pub client: reqwest::Client,
    fingerprints: Arc<Mutex<HashMap<String, (Instant, u32)>>>,
    pub bypass_path: PathBuf,
    pub session_started: i64,
    pub budget: Arc<RwLock<crate::config::BudgetConfig>>,
    pub transform: Arc<RwLock<crate::transform::TransformPolicy>>,
    pub cache_config: Arc<RwLock<crate::config::CacheConfig>>,
    pub routing_config: Arc<RwLock<crate::config::RoutingConfig>>,
    pub privacy: Arc<RwLock<PrivacyConfig>>,
    pub config_path: PathBuf,
    runtime_config: Arc<RwLock<AppConfig>>,
    cache: Arc<Mutex<ResponseCache>>,
    rate_window: Arc<Mutex<VecDeque<Instant>>>,
    inflight: Arc<AtomicUsize>,
    token_reservations: Arc<Mutex<TokenReservations>>,
    usd_reservations: Arc<Mutex<UsdReservations>>,
    provider_health: Arc<Mutex<HashMap<String, ProviderHealth>>>,
    cache_metrics: Arc<Mutex<CacheMetrics>>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct CacheMetrics {
    hits: u64,
    misses: u64,
    hash_failures: u64,
    expired: u64,
    capacity_evictions: u64,
}

#[derive(Debug, Clone, Serialize)]
struct CacheStatusPayload {
    enabled: bool,
    entries: usize,
    bytes: usize,
    max_entries: usize,
    max_total_bytes: usize,
    hits: u64,
    misses: u64,
    hash_failures: u64,
    expired: u64,
    capacity_evictions: u64,
}

#[derive(Debug, Deserialize)]
struct BypassPayload {
    enabled: bool,
}

#[derive(Clone)]
struct CachedResponse {
    expires_at: Instant,
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
    body_hash: String,
}

#[derive(Default)]
struct ResponseCache {
    entries: HashMap<String, CachedResponse>,
    order: VecDeque<String>,
    total_bytes: usize,
}

#[derive(Default)]
struct UsdReservations {
    by_scope: HashMap<String, (f64, f64)>,
}

#[derive(Default)]
struct TokenReservations {
    by_scope: HashMap<String, (i64, i64)>,
}

#[derive(Clone, Copy, Debug)]
struct ProviderHealth {
    consecutive_failures: u32,
    opened_until: Option<Instant>,
}

struct TokenReservation {
    reservations: Arc<Mutex<TokenReservations>>,
    scope: String,
    amount: i64,
    session: bool,
    daily: bool,
}

impl Drop for TokenReservation {
    fn drop(&mut self) {
        let Ok(mut reservations) = self.reservations.lock() else {
            return;
        };
        let mut remove = false;
        if let Some((session, daily)) = reservations.by_scope.get_mut(&self.scope) {
            if self.session {
                *session = session.saturating_sub(self.amount).max(0);
            }
            if self.daily {
                *daily = daily.saturating_sub(self.amount).max(0);
            }
            remove = *session == 0 && *daily == 0;
        }
        if remove {
            reservations.by_scope.remove(&self.scope);
        }
    }
}

struct UsdReservation {
    reservations: Arc<Mutex<UsdReservations>>,
    scope: String,
    amount: f64,
    session: bool,
    daily: bool,
}

impl Drop for UsdReservation {
    fn drop(&mut self) {
        let Ok(mut reservations) = self.reservations.lock() else {
            return;
        };
        let mut remove = false;
        if let Some((session, daily)) = reservations.by_scope.get_mut(&self.scope) {
            if self.session {
                *session = (*session - self.amount).max(0.0);
            }
            if self.daily {
                *daily = (*daily - self.amount).max(0.0);
            }
            remove = *session <= 0.0 && *daily <= 0.0;
        }
        if remove {
            reservations.by_scope.remove(&self.scope);
        }
    }
}

#[derive(Default)]
struct StreamProgress {
    streamed_bytes: usize,
    usage: Option<(i64, i64, i64)>,
    finalized: bool,
}

struct StreamLifecycleGuard {
    ledger: Ledger,
    request_id: String,
    attempt: i64,
    provider: ProviderProfile,
    input_tokens: i64,
    started: Instant,
    progress: Arc<Mutex<StreamProgress>>,
}

impl Drop for StreamLifecycleGuard {
    fn drop(&mut self) {
        let Ok(mut progress) = self.progress.lock() else {
            return;
        };
        if progress.finalized {
            return;
        }
        progress.finalized = true;
        let (input, output, cached) = progress.usage.unwrap_or((
            self.input_tokens,
            estimate_tokens(progress.streamed_bytes),
            0,
        ));
        let cost = provider_cost(&self.provider, input, output, cached);
        let _ = self.ledger.record_finish(
            &self.request_id,
            "cancelled",
            progress.usage.map(|(input, _, _)| input),
            output,
            cached,
            cost,
            self.started.elapsed().as_millis() as i64,
        );
        let _ = self.ledger.record_attempt_finish(
            &self.request_id,
            self.attempt,
            "cancelled_by_client",
            Some("downstream connection closed"),
            self.started.elapsed().as_millis() as i64,
        );
    }
}

struct InflightGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, Serialize)]
struct StatusPayload {
    name: &'static str,
    version: &'static str,
    gateway: String,
    provider: Option<String>,
    providers: usize,
    setup_required: bool,
    next_action: Option<String>,
    bypassed: bool,
    request_token_budget: Option<i64>,
    session_token_budget: Option<i64>,
    daily_token_budget: Option<i64>,
    request_output_token_budget: Option<i64>,
    request_budget_usd: Option<f64>,
    session_budget_usd: Option<f64>,
    daily_budget_usd: Option<f64>,
    max_same_fingerprint: u32,
    cache_enabled: bool,
    cache_ttl_seconds: u64,
    cache_max_entries: usize,
    cache_max_entry_bytes: usize,
    routing_mode: String,
    routing_pool_size: usize,
    max_concurrency: Option<usize>,
    requests_per_minute: Option<u32>,
    max_request_bytes: usize,
    max_response_bytes: usize,
    stream_idle_timeout_seconds: u64,
    gateway_auth_enabled: bool,
    cache_total_bytes: usize,
    privacy_strict: bool,
    budget_scope_count: usize,
    circuit_breaker_threshold: u32,
    circuit_breaker_cooldown_seconds: u64,
}

impl AppState {
    pub fn new(config: AppConfig, ledger: Ledger) -> Result<Self> {
        Self::new_with_config_path(config, ledger, AppConfig::path())
    }

    pub fn new_with_config_path(
        config: AppConfig,
        ledger: Ledger,
        config_path: PathBuf,
    ) -> Result<Self> {
        let budget = config.budget.clone();
        let transform = config.transform.clone();
        let cache_config = config.cache.clone();
        let routing_config = config.routing.clone();
        let privacy_config = config.privacy.clone();
        let runtime_config = config.clone();
        let data_dir = AppConfig::data_dir_for_config(&config_path);
        AppConfig::ensure_data_dir(&data_dir)?;
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(1800))
            .build()?;
        Ok(Self {
            config: Arc::new(config),
            ledger,
            client,
            fingerprints: Arc::new(Mutex::new(HashMap::new())),
            bypass_path: data_dir.join("bypass"),
            session_started: chrono::Utc::now().timestamp(),
            budget: Arc::new(RwLock::new(budget)),
            transform: Arc::new(RwLock::new(transform)),
            cache_config: Arc::new(RwLock::new(cache_config)),
            routing_config: Arc::new(RwLock::new(routing_config)),
            privacy: Arc::new(RwLock::new(privacy_config)),
            config_path,
            runtime_config: Arc::new(RwLock::new(runtime_config)),
            cache: Arc::new(Mutex::new(ResponseCache::default())),
            rate_window: Arc::new(Mutex::new(VecDeque::new())),
            inflight: Arc::new(AtomicUsize::new(0)),
            token_reservations: Arc::new(Mutex::new(TokenReservations::default())),
            usd_reservations: Arc::new(Mutex::new(UsdReservations::default())),
            provider_health: Arc::new(Mutex::new(HashMap::new())),
            cache_metrics: Arc::new(Mutex::new(CacheMetrics::default())),
        })
    }
}

pub async fn run(state: Arc<AppState>) -> Result<()> {
    let gateway_addr = state.config.gateway_listen.clone();
    let admin_addr = state.config.admin_listen.clone();
    validate_gateway_listen(
        &gateway_addr,
        state.config.gateway_auth_token_env.as_deref(),
    )?;
    validate_admin_listen(&admin_addr)?;
    let gateway = Router::new()
        .fallback(proxy)
        .layer(DefaultBodyLimit::max(state.config.max_request_bytes))
        .with_state(state.clone());
    let admin = Router::new()
        .route("/", get(dashboard))
        .route("/logo.png", get(logo))
        .route("/healthz", get(healthz))
        .route("/api/reload", post(reload))
        .route("/api/cache/clear", post(clear_cache))
        .route("/api/cache/status", get(cache_status))
        .route("/api/bypass", post(toggle_bypass))
        .route("/api/control-events", get(control_events))
        .route("/api/rules", get(rules))
        .route("/api/status", get(status))
        .route("/api/stats", get(stats))
        .route("/api/requests", get(requests))
        .route("/api/requests/{id}", get(request_detail))
        .route("/api/tasks", get(tasks))
        .route("/api/trends", get(trends))
        .with_state(state);
    let gateway_listener = TcpListener::bind(&gateway_addr)
        .await
        .with_context(|| format!("Gateway 监听失败: {gateway_addr}"))?;
    let admin_listener = TcpListener::bind(&admin_addr)
        .await
        .with_context(|| format!("Admin 监听失败: {admin_addr}"))?;
    info!(%gateway_addr, %admin_addr, "DuoLA AgentCost 已启动");
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let signal_task = async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(true);
        Ok::<(), std::io::Error>(())
    };
    tokio::try_join!(
        axum::serve(gateway_listener, gateway)
            .with_graceful_shutdown(wait_for_shutdown(shutdown_rx.clone())),
        axum::serve(admin_listener, admin).with_graceful_shutdown(wait_for_shutdown(shutdown_rx)),
        signal_task
    )?;
    Ok(())
}

async fn status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let runtime = state.runtime_config.read().await.clone();
    let provider = runtime
        .default_provider
        .clone()
        .or_else(|| runtime.providers.first().map(|p| p.id.clone()));
    let setup_required = runtime.providers.is_empty();
    let budget = state.budget.read().await.clone();
    let cache = state.cache_config.read().await.clone();
    let routing = state.routing_config.read().await.clone();
    let privacy = state.privacy.read().await.clone();
    axum::Json(StatusPayload {
        name: "DuoLA AgentCost",
        version: env!("CARGO_PKG_VERSION"),
        gateway: state.config.gateway_listen.clone(),
        provider,
        providers: runtime.providers.len(),
        setup_required,
        next_action: setup_required.then(|| "duola-agentcost setup --agent codex".to_owned()),
        bypassed: state.bypass_path.exists(),
        request_token_budget: budget.request_tokens,
        session_token_budget: budget.session_tokens,
        daily_token_budget: budget.daily_tokens,
        request_output_token_budget: budget.request_output_tokens,
        request_budget_usd: budget.request_usd,
        session_budget_usd: budget.session_usd,
        daily_budget_usd: budget.daily_usd,
        max_same_fingerprint: budget.max_same_fingerprint,
        cache_enabled: cache.enabled,
        cache_ttl_seconds: cache.ttl_seconds,
        cache_max_entries: cache.max_entries,
        cache_max_entry_bytes: cache.max_entry_bytes,
        cache_total_bytes: cache.max_total_bytes,
        routing_mode: routing.mode,
        routing_pool_size: routing.pool.len(),
        max_concurrency: budget.max_concurrency,
        requests_per_minute: budget.requests_per_minute,
        max_request_bytes: state.config.max_request_bytes,
        max_response_bytes: state.config.max_response_bytes,
        stream_idle_timeout_seconds: state.config.stream_idle_timeout_seconds,
        gateway_auth_enabled: state.config.gateway_auth_token_env.is_some(),
        budget_scope_count: budget.scopes.len(),
        circuit_breaker_threshold: routing.circuit_breaker_threshold,
        circuit_breaker_cooldown_seconds: routing.circuit_breaker_cooldown_seconds,
        privacy_strict: privacy.strict,
    })
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::NO_CONTENT, "")
}

async fn reload(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match AppConfig::load(Some(&state.config_path)) {
        Ok(config) => {
            *state.runtime_config.write().await = config.clone();
            *state.budget.write().await = config.budget;
            *state.transform.write().await = config.transform;
            *state.cache_config.write().await = config.cache;
            *state.routing_config.write().await = config.routing;
            *state.privacy.write().await = config.privacy;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn clear_cache(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut cache = state.cache.lock().unwrap_or_else(|e| e.into_inner());
    cache.entries.clear();
    cache.order.clear();
    cache.total_bytes = 0;
    StatusCode::NO_CONTENT
}

async fn cache_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let config = state.cache_config.read().await.clone();
    let (entries, bytes) = state
        .cache
        .lock()
        .map(|cache| (cache.entries.len(), cache.total_bytes))
        .unwrap_or((0, 0));
    let metrics = state
        .cache_metrics
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    axum::Json(CacheStatusPayload {
        enabled: config.enabled,
        entries,
        bytes,
        max_entries: config.max_entries,
        max_total_bytes: config.max_total_bytes,
        hits: metrics.hits,
        misses: metrics.misses,
        hash_failures: metrics.hash_failures,
        expired: metrics.expired,
        capacity_evictions: metrics.capacity_evictions,
    })
}

async fn toggle_bypass(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BypassPayload>,
) -> impl IntoResponse {
    if payload.enabled {
        if let Err(error) = AppConfig::ensure_data_dir(
            state
                .bypass_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new(".")),
        ) {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
        if let Err(error) = std::fs::write(&state.bypass_path, b"enabled") {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
    } else if let Err(error) = std::fs::remove_file(&state.bypass_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
    }
    let action = if payload.enabled { "bypass" } else { "restore" };
    if let Err(error) =
        state
            .ledger
            .record_control_event(action, payload.enabled, "dashboard action")
    {
        warn!(error = %error, "旁路控制事件写入失败");
    }
    axum::Json(serde_json::json!({
        "bypassed": payload.enabled,
        "message": if payload.enabled { "已启用原样透传" } else { "已恢复 AgentCost 接管" }
    }))
    .into_response()
}

async fn control_events(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.ledger.control_events(50) {
        Ok(events) => axum::Json(events).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn rules() -> impl IntoResponse {
    axum::Json(crate::transform::rule_registry())
}

async fn stats(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.ledger.stats() {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn requests(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.ledger.recent(50) {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn tasks(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.ledger.tasks(50) {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct TrendQuery {
    days: Option<u64>,
}

async fn trends(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TrendQuery>,
) -> impl IntoResponse {
    let days = query.days.unwrap_or(30).clamp(1, 366);
    let since = chrono::Utc::now().timestamp() - (days as i64) * 24 * 60 * 60;
    match state.ledger.trends(since) {
        Ok(points) => axum::Json(points).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

#[derive(Debug, Serialize)]
struct RequestDetail {
    request_id: String,
    attempts: Vec<ProviderAttempt>,
    receipts: Vec<ReceiptRecord>,
}

async fn request_detail(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.ledger.attempts(&id) {
        Ok(attempts) => match state.ledger.request_exists(&id) {
            Ok(true) => {
                let receipts = state.ledger.receipts(&id).unwrap_or_default();
                axum::Json(RequestDetail {
                    request_id: id,
                    attempts,
                    receipts,
                })
                .into_response()
            }
            Ok(false) => (StatusCode::NOT_FOUND, "request not found").into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn dashboard() -> Html<&'static str> {
    Html(include_str!("../ui/index.html"))
}

async fn logo() -> Response {
    let mut response = Response::new(Body::from(include_bytes!("../ui/logo.png").as_slice()));
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static("image/png"));
    response.headers_mut().insert(
        "cache-control",
        HeaderValue::from_static("public, max-age=86400"),
    );
    response
}

fn request_hash(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

/// Provider errors can include a URL query or a header-like fragment.  Keep
/// the diagnostic useful while ensuring the local ledger and dashboard never
/// become a secret sink.
fn redact_sensitive_error(input: &str) -> String {
    let keys = [
        "api_key",
        "apikey",
        "access_token",
        "token",
        "secret",
        "password",
        "signature",
        "private_key",
        "authorization",
        "cookie",
    ];
    let mut output = input.to_owned();
    for key in keys {
        loop {
            let lower = output.to_ascii_lowercase();
            let Some(position) = lower.find(key) else {
                break;
            };
            let after_key = position + key.len();
            let rest = &output[after_key..];
            let separator_len = rest
                .chars()
                .next()
                .filter(|character| matches!(character, '=' | ':'))
                .map(char::len_utf8)
                .unwrap_or(0);
            if separator_len == 0 {
                break;
            }
            let value_start = after_key + separator_len;
            let leading_whitespace = output[value_start..]
                .chars()
                .take_while(|character| character.is_whitespace())
                .map(char::len_utf8)
                .sum::<usize>();
            let value_start = value_start + leading_whitespace;
            let suffix = &output[value_start..];
            if suffix.starts_with("[REDACTED]") {
                break;
            }
            let value_len = suffix
                .char_indices()
                .find(|(_, character)| {
                    matches!(character, '&' | ',' | '\n' | '\r' | '"' | '\'' | ')' | ']')
                        || (*character == ' ' && !matches!(key, "authorization" | "cookie"))
                })
                .map(|(index, _)| index)
                .unwrap_or(suffix.len());
            if value_len == 0 {
                break;
            }
            output.replace_range(value_start..value_start + value_len, "[REDACTED]");
        }
    }
    output
}

fn cache_key(method: &Method, uri: &Uri, provider: &str, body: &[u8]) -> String {
    let canonical = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
        .unwrap_or_else(|| body.to_vec());
    request_hash(
        format!(
            "{}\n{}\n{}\n{}",
            method,
            uri,
            provider,
            request_hash(&canonical)
        )
        .as_bytes(),
    )
}

fn cache_key_with_headers(
    method: &Method,
    uri: &Uri,
    provider: &ProviderProfile,
    headers: &HeaderMap,
    body: &[u8],
) -> String {
    let mut material = format!(
        "{}\n{}\n{}\n{}",
        method,
        uri,
        provider.id,
        cache_key(method, uri, &provider.id, body)
    );
    // Credential and tenant headers must participate in the key, but only their
    // hashes are retained. This prevents cross-account/cross-credential reuse
    // without putting secrets into the in-memory cache key.
    for name in [
        "authorization",
        "x-api-key",
        "anthropic-version",
        "anthropic-beta",
        "openai-organization",
        "openai-project",
        "content-type",
        "accept",
    ] {
        if let Some(value) = headers.get(name) {
            material.push('\n');
            material.push_str(name);
            material.push('=');
            material.push_str(&request_hash(value.as_bytes()));
        }
    }
    if let Some(env_name) = &provider.api_key_env {
        material.push_str("\napi-key-env=");
        material.push_str(&request_hash(
            std::env::var(env_name).unwrap_or_default().as_bytes(),
        ));
    }
    request_hash(material.as_bytes())
}

/// Cache only deterministic-looking, non-streaming read requests. Tool calls,
/// tool definitions and state-changing request markers are deliberately not
/// cached because replaying them can change an agent's behavior.
fn cache_eligible(body: &[u8], content_type: &str) -> bool {
    if !content_type.contains("json") {
        return false;
    }
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let Some(map) = value.as_object() else {
        return false;
    };
    if map.get("stream").and_then(Value::as_bool).unwrap_or(false)
        || map.contains_key("tools")
        || map.contains_key("tool_choice")
        || map.contains_key("tool_calls")
        || map.contains_key("function_call")
    {
        return false;
    }
    let has_state_marker = ["live", "realtime", "subscribe", "execute", "transaction"]
        .iter()
        .any(|key| map.contains_key(*key));
    if has_state_marker {
        return false;
    }
    // Exact replay is only safe for deterministic-looking requests. A caller
    // can still opt out simply by setting any sampling/penalty parameter.
    for key in ["temperature", "presence_penalty", "frequency_penalty"] {
        if map
            .get(key)
            .and_then(Value::as_f64)
            .is_some_and(|value| value != 0.0)
        {
            return false;
        }
    }
    if map
        .get("top_p")
        .and_then(Value::as_f64)
        .is_some_and(|value| value < 1.0)
        || map
            .get("n")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > 1)
        || map
            .get("logprobs")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || map
            .get("parallel_tool_calls")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || map.contains_key("modalities")
    {
        return false;
    }
    true
}

#[allow(dead_code)]
fn cache_get(cache: &Mutex<ResponseCache>, key: &str) -> Option<CachedResponse> {
    cache_get_observed(cache, key, None)
}

fn cache_get_observed(
    cache: &Mutex<ResponseCache>,
    key: &str,
    metrics: Option<&Mutex<CacheMetrics>>,
) -> Option<CachedResponse> {
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let expired: Vec<String> = cache
        .entries
        .iter()
        .filter(|(_, entry)| entry.expires_at <= now)
        .map(|(key, _)| key.clone())
        .collect();
    let expired_count = expired.len();
    for expired_key in expired {
        remove_cache_entry(&mut cache, &expired_key);
    }
    if let Some(metrics) = metrics
        && let Ok(mut value) = metrics.lock()
    {
        value.expired = value.expired.saturating_add(expired_count as u64);
    }
    let live_keys: HashSet<String> = cache.entries.keys().cloned().collect();
    cache.order.retain(|entry| live_keys.contains(entry));
    let entry = cache
        .entries
        .get(key)
        .cloned()
        .and_then(|entry| (request_hash(&entry.body) == entry.body_hash).then_some(entry));
    if entry.is_none() {
        if cache.entries.contains_key(key)
            && let Some(metrics) = metrics
            && let Ok(mut value) = metrics.lock()
        {
            value.hash_failures = value.hash_failures.saturating_add(1);
        }
        remove_cache_entry(&mut cache, key);
    }
    if entry.is_some() {
        cache.order.retain(|entry| entry != key);
        cache.order.push_back(key.to_owned());
    }
    entry
}

#[allow(dead_code)]
fn cache_put(
    cache: &Mutex<ResponseCache>,
    key: String,
    response: CachedResponse,
    max_entries: usize,
    max_total_bytes: usize,
) {
    cache_put_observed(cache, key, response, max_entries, max_total_bytes, None);
}

fn cache_put_observed(
    cache: &Mutex<ResponseCache>,
    key: String,
    response: CachedResponse,
    max_entries: usize,
    max_total_bytes: usize,
    metrics: Option<&Mutex<CacheMetrics>>,
) {
    if max_entries == 0 || max_total_bytes == 0 || response.body.len() > max_total_bytes {
        return;
    }
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let expired: Vec<String> = cache
        .entries
        .iter()
        .filter(|(_, entry)| entry.expires_at <= now)
        .map(|(key, _)| key.clone())
        .collect();
    for expired_key in expired {
        remove_cache_entry(&mut cache, &expired_key);
    }
    remove_cache_entry(&mut cache, &key);
    cache.total_bytes = cache.total_bytes.saturating_add(response.body.len());
    cache.entries.insert(key.clone(), response);
    cache.order.push_back(key);
    while cache.order.len() > max_entries || cache.total_bytes > max_total_bytes {
        if let Some(oldest) = cache.order.pop_front()
            && let Some(removed) = cache.entries.remove(&oldest)
        {
            cache.total_bytes = cache.total_bytes.saturating_sub(removed.body.len());
            if let Some(metrics) = metrics
                && let Ok(mut value) = metrics.lock()
            {
                value.capacity_evictions = value.capacity_evictions.saturating_add(1);
            }
        }
    }
}

fn remove_cache_entry(cache: &mut ResponseCache, key: &str) {
    if let Some(removed) = cache.entries.remove(key) {
        cache.total_bytes = cache.total_bytes.saturating_sub(removed.body.len());
    }
    cache.order.retain(|entry| entry != key);
}

fn take_rate_slot(rate_window: &Mutex<VecDeque<Instant>>, limit: Option<u32>) -> bool {
    let Some(limit) = limit.filter(|limit| *limit > 0) else {
        return true;
    };
    let mut window = rate_window.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    while window
        .front()
        .is_some_and(|started| now.duration_since(*started) >= std::time::Duration::from_secs(60))
    {
        window.pop_front();
    }
    if window.len() >= limit as usize {
        return false;
    }
    window.push_back(now);
    true
}

fn try_acquire_concurrency(
    counter: &Arc<AtomicUsize>,
    limit: Option<usize>,
) -> Option<InflightGuard> {
    let Some(limit) = limit.filter(|limit| *limit > 0) else {
        return Some(InflightGuard {
            counter: counter.clone(),
        });
    };
    loop {
        let current = counter.load(Ordering::Acquire);
        if current >= limit {
            return None;
        }
        if counter
            .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Some(InflightGuard {
                counter: counter.clone(),
            });
        }
    }
}

fn provider_is_available(health: &Mutex<HashMap<String, ProviderHealth>>, provider: &str) -> bool {
    let Ok(mut health) = health.lock() else {
        return true;
    };
    let Some(state) = health.get_mut(provider) else {
        return true;
    };
    if let Some(until) = state.opened_until {
        if until > Instant::now() {
            return false;
        }
        state.opened_until = None;
        state.consecutive_failures = 0;
    }
    true
}

fn provider_record_success(health: &Mutex<HashMap<String, ProviderHealth>>, provider: &str) {
    if let Ok(mut health) = health.lock() {
        health.remove(provider);
    }
}

fn provider_record_failure(
    health: &Mutex<HashMap<String, ProviderHealth>>,
    provider: &str,
    threshold: u32,
    cooldown_seconds: u64,
) {
    if threshold == 0 {
        return;
    }
    if let Ok(mut health) = health.lock() {
        let state = health.entry(provider.to_owned()).or_insert(ProviderHealth {
            consecutive_failures: 0,
            opened_until: None,
        });
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if state.consecutive_failures >= threshold {
            state.opened_until =
                Some(Instant::now() + Duration::from_secs(cooldown_seconds.max(1)));
        }
    }
}

fn upstream_url(provider: &ProviderProfile, path: &str, query: Option<&str>) -> String {
    let endpoint = provider.endpoint.trim_end_matches('/');
    let mut path = path.to_owned();
    if endpoint.ends_with("/v1") && path.starts_with("/v1") {
        path = path.trim_start_matches("/v1").to_owned();
        if path.is_empty() {
            path = "/".into();
        }
    }
    match query {
        Some(q) => format!("{endpoint}{path}?{q}"),
        None => format!("{endpoint}{path}"),
    }
}

fn fallback_allowed(
    method: &Method,
    provider: &ProviderProfile,
    routing: &crate::config::RoutingConfig,
    headers: &HeaderMap,
) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    if headers.contains_key("idempotency-key") {
        return true;
    }
    let protocol = provider.protocol.to_ascii_lowercase();
    if protocol.contains("openai")
        || protocol.contains("responses")
        || protocol.contains("anthropic")
    {
        return true;
    }
    routing.allow_non_idempotent_fallback
}

fn copy_request_headers(
    input: &HeaderMap,
    provider: &ProviderProfile,
    strip_incoming_auth: bool,
) -> HeaderMap {
    let mut output = HeaderMap::new();
    for (name, value) in input {
        if matches!(
            name.as_str(),
            "host"
                | "content-length"
                | "connection"
                | "x-duola-gateway-token"
                | "x-duola-agent-session"
                | "x-duola-project"
                | "x-duola-agent"
                | "x-duola-transform"
        ) {
            continue;
        }
        output.insert(name.clone(), value.clone());
    }
    if strip_incoming_auth || provider.api_key_env.is_some() {
        output.remove("authorization");
        output.remove("x-api-key");
    }
    if !output.contains_key("authorization")
        && !output.contains_key("x-api-key")
        && let Some(env_name) = &provider.api_key_env
        && let Ok(key) = std::env::var(env_name)
    {
        if provider.protocol.to_ascii_lowercase().contains("anthropic") {
            if let Ok(value) = HeaderValue::from_str(&key) {
                output.insert(HeaderName::from_static("x-api-key"), value);
            }
        } else if let Ok(value) = HeaderValue::from_str(&format!("Bearer {key}")) {
            output.insert(HeaderName::from_static("authorization"), value);
        }
    }
    output
}

fn copy_response_headers(input: &reqwest::header::HeaderMap) -> HeaderMap {
    let mut output = HeaderMap::new();
    for (name, value) in input {
        if matches!(
            name.as_str(),
            "content-length" | "connection" | "transfer-encoding"
        ) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_str().as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            output.insert(name, value);
        }
    }
    output
}

fn cache_response_headers(input: &HeaderMap) -> HeaderMap {
    let mut output = input.clone();
    for name in [
        "date",
        "server",
        "set-cookie",
        "x-request-id",
        "request-id",
        "retry-after",
    ] {
        output.remove(name);
    }
    output
}

fn validate_admin_listen(value: &str) -> Result<()> {
    let address: SocketAddr = value
        .parse()
        .with_context(|| format!("Admin 监听地址必须是 host:port，当前为 {value}"))?;
    if !address.ip().is_loopback() {
        anyhow::bail!("Admin 只允许绑定回环地址（127.0.0.1/[::1]）；Gateway 可按需对外暴露")
    }
    Ok(())
}

/// Local, side-effect-free configuration validation used by `doctor` and
/// tests. It never contacts a Provider or spends model quota.
pub fn validate_config_addresses(config: &AppConfig) -> Result<()> {
    validate_gateway_listen(
        &config.gateway_listen,
        config.gateway_auth_token_env.as_deref(),
    )?;
    validate_admin_listen(&config.admin_listen)
}

fn validate_gateway_listen(value: &str, token_env: Option<&str>) -> Result<()> {
    let address: SocketAddr = value
        .parse()
        .with_context(|| format!("Gateway 监听地址必须是明确的 IP:端口，当前为 {value}"))?;
    let token = token_env
        .map(std::env::var)
        .transpose()
        .with_context(|| "读取 Gateway 认证 Token 环境变量失败")?;
    let token_missing = token.as_deref().is_none_or(|value| value.trim().is_empty());
    if !address.ip().is_loopback() && token_missing {
        anyhow::bail!(
            "Gateway 监听在非回环地址 {value} 时必须配置 gateway_auth_token_env，并设置对应环境变量"
        );
    }
    if token_env.is_some() && token_missing {
        anyhow::bail!("Gateway 认证 Token 环境变量为空，拒绝启动");
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                    return;
                }
            };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn wait_for_shutdown(mut receiver: watch::Receiver<bool>) {
    if *receiver.borrow() {
        return;
    }
    while receiver.changed().await.is_ok() {
        if *receiver.borrow() {
            return;
        }
    }
}

fn limit_display_mb(bytes: usize) -> usize {
    bytes.saturating_add(1024 * 1024 - 1) / (1024 * 1024)
}

async fn read_response_limited(response: reqwest::Response, max_bytes: usize) -> Result<Bytes> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        anyhow::bail!("Provider 响应超过 {} MB 限制", limit_display_mb(max_bytes));
    }
    let mut output = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk?;
        if output.len().saturating_add(chunk.len()) > max_bytes {
            anyhow::bail!("Provider 响应超过 {} MB 限制", limit_display_mb(max_bytes));
        }
        output.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(output))
}

fn requested_model(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}

fn rewrite_model(body: &[u8], model: &str) -> Vec<u8> {
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return body.to_vec();
    };
    if let Value::Object(map) = &mut value {
        map.insert("model".into(), Value::String(model.into()));
        serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec())
    } else {
        body.to_vec()
    }
}

fn apply_output_budget(
    body: &[u8],
    provider: &ProviderProfile,
    limit: Option<i64>,
) -> Option<(Vec<u8>, String)> {
    let limit = limit.filter(|value| *value >= 0)?;
    let mut value = serde_json::from_slice::<Value>(body).ok()?;
    let map = value.as_object_mut()?;
    let protocol = provider.protocol.to_ascii_lowercase();
    let key = if protocol.contains("anthropic") {
        "max_tokens"
    } else if protocol.contains("responses") {
        "max_output_tokens"
    } else if map.contains_key("max_completion_tokens") {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };
    let should_update = match map.get(key).and_then(Value::as_i64) {
        Some(existing) => existing > limit,
        None => true,
    };
    if !should_update {
        return None;
    }
    map.insert(key.into(), Value::Number(limit.into()));
    Some((serde_json::to_vec(&value).ok()?, key.into()))
}

fn resolve_provider_and_model(
    config: &AppConfig,
    body: &[u8],
) -> Result<(ProviderProfile, Vec<u8>)> {
    let Some(model) = requested_model(body) else {
        return Ok((config.provider(None)?, body.to_vec()));
    };
    // Model maps are explicit user routing. They never silently choose a
    // cheaper model; the mapped provider/model is recorded by the ledger.
    let default = config.provider(None)?;
    if let Some(mapped) = default.model_map.get(&model).cloned() {
        return Ok((default, rewrite_model(body, &mapped)));
    }
    for candidate in &config.providers {
        if candidate.protocol == default.protocol
            && let Some(mapped) = candidate.model_map.get(&model)
        {
            return Ok((candidate.clone(), rewrite_model(body, mapped)));
        }
    }
    Ok((default, body.to_vec()))
}

fn usage_from_json(value: &Value) -> Option<(i64, i64, i64)> {
    if let Value::Object(map) = value {
        if let Some(Value::Object(usage)) = map.get("usage") {
            let input = usage
                .get("input_tokens")
                .or_else(|| usage.get("prompt_tokens"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let output = usage
                .get("output_tokens")
                .or_else(|| usage.get("completion_tokens"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let cached = usage
                .get("cache_read_input_tokens")
                .or_else(|| usage.get("cached_tokens"))
                .and_then(Value::as_i64)
                .or_else(|| {
                    usage
                        .get("prompt_tokens_details")
                        .and_then(Value::as_object)
                        .and_then(|details| details.get("cached_tokens"))
                        .and_then(Value::as_i64)
                })
                .or_else(|| {
                    usage
                        .get("input_tokens_details")
                        .and_then(Value::as_object)
                        .and_then(|details| details.get("cached_tokens"))
                        .and_then(Value::as_i64)
                })
                .unwrap_or(0);
            return Some((input, output, cached));
        }
        for child in map.values() {
            if let Some(found) = usage_from_json(child) {
                return Some(found);
            }
        }
    } else if let Value::Array(items) = value {
        for child in items {
            if let Some(found) = usage_from_json(child) {
                return Some(found);
            }
        }
    }
    None
}

fn usage_from_bytes(data: &[u8]) -> Option<(i64, i64, i64)> {
    if let Ok(value) = serde_json::from_slice::<Value>(data) {
        return usage_from_json(&value);
    }
    let mut found = None;
    for line in data.split(|b| *b == b'\n') {
        let line = line.strip_prefix(b"data:").unwrap_or(line);
        if let Ok(value) = serde_json::from_slice::<Value>(line)
            && let Some(usage) = usage_from_json(&value)
        {
            found = merge_usage(found, usage);
        }
    }
    found
}

fn merge_usage(current: Option<(i64, i64, i64)>, next: (i64, i64, i64)) -> Option<(i64, i64, i64)> {
    Some(match current {
        None => next,
        // Provider streams commonly report input usage at the start and
        // output usage at the end. They also sometimes repeat cumulative
        // usage, so max preserves the complete observation without double
        // counting repeated events.
        Some((input, output, cached)) => {
            (input.max(next.0), output.max(next.1), cached.max(next.2))
        }
    })
}

fn stream_has_terminal_marker(data: &[u8]) -> bool {
    data.windows(b"[DONE]".len())
        .any(|window| window == b"[DONE]")
        || data
            .windows(b"response.completed".len())
            .any(|window| window == b"response.completed")
        || data
            .windows(b"message_stop".len())
            .any(|window| window == b"message_stop")
        || data
            .windows(b"response.failed".len())
            .any(|window| window == b"response.failed")
}

fn stream_has_failure_marker(data: &[u8]) -> bool {
    data.windows(b"response.failed".len())
        .any(|window| window == b"response.failed")
        || data
            .windows(b"message_error".len())
            .any(|window| window == b"message_error")
}

fn provider_cost(provider: &ProviderProfile, input: i64, output: i64, cached: i64) -> f64 {
    let in_price = provider.input_price_per_million.unwrap_or(0.0);
    let out_price = provider.output_price_per_million.unwrap_or(0.0);
    let cache_price = provider.cached_input_price_per_million.unwrap_or(in_price);
    ((input.saturating_sub(cached)) as f64 / 1_000_000.0) * in_price
        + (cached as f64 / 1_000_000.0) * cache_price
        + (output as f64 / 1_000_000.0) * out_price
}

fn has_usd_budget(budget: &crate::config::BudgetConfig) -> bool {
    budget.request_usd.is_some() || budget.session_usd.is_some() || budget.daily_usd.is_some()
}

fn has_token_budget(budget: &crate::config::BudgetConfig) -> bool {
    budget.session_tokens.is_some() || budget.daily_tokens.is_some()
}

fn output_budget_field(body: &Value, provider: &ProviderProfile) -> Option<&'static str> {
    let map = body.as_object()?;
    let protocol = provider.protocol.to_ascii_lowercase();
    Some(if protocol.contains("anthropic") {
        "max_tokens"
    } else if protocol.contains("responses") {
        "max_output_tokens"
    } else if map.contains_key("max_completion_tokens") {
        "max_completion_tokens"
    } else {
        "max_tokens"
    })
}

fn output_limit_from_body(body: &[u8], provider: &ProviderProfile) -> Option<i64> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    let field = output_budget_field(&value, provider)?;
    value
        .as_object()?
        .get(field)
        .and_then(Value::as_i64)
        .filter(|value| *value >= 0)
}

fn worst_case_cost(
    config: &AppConfig,
    provider: &ProviderProfile,
    input_tokens: i64,
    output_tokens: i64,
) -> f64 {
    let mut input_price = provider.input_price_per_million.unwrap_or(0.0);
    let mut output_price = provider.output_price_per_million.unwrap_or(0.0);
    for candidate in &config.providers {
        if candidate.protocol == provider.protocol {
            input_price = input_price.max(candidate.input_price_per_million.unwrap_or(0.0));
            output_price = output_price.max(candidate.output_price_per_million.unwrap_or(0.0));
        }
    }
    ((input_tokens.max(0) as f64) / 1_000_000.0) * input_price
        + ((output_tokens.max(0) as f64) / 1_000_000.0) * output_price
}

fn worst_case_output_price(config: &AppConfig, provider: &ProviderProfile) -> f64 {
    let mut price = provider.output_price_per_million.unwrap_or(0.0);
    for candidate in &config.providers {
        if candidate.protocol == provider.protocol {
            price = price.max(candidate.output_price_per_million.unwrap_or(0.0));
        }
    }
    price
}

fn derived_usd_output_cap(
    state: &AppState,
    runtime_config: &AppConfig,
    config: &crate::config::BudgetConfig,
    provider: &ProviderProfile,
    input_tokens: i64,
    scope: &str,
) -> Option<i64> {
    if !has_usd_budget(config) {
        return None;
    }
    let output_price = worst_case_output_price(runtime_config, provider);
    if output_price <= 0.0 {
        return None;
    }
    let scope_filter = (!scope.is_empty()).then_some(scope);
    let session_spend = state
        .ledger
        .cost_since_scope(state.session_started, scope_filter)
        .unwrap_or(0.0);
    let daily_spend = state
        .ledger
        .cost_since_scope(day_start(), scope_filter)
        .unwrap_or(0.0);
    let reservations = state
        .usd_reservations
        .lock()
        .map(|value| value.by_scope.get(scope).copied().unwrap_or((0.0, 0.0)))
        .unwrap_or((0.0, 0.0));
    let mut remaining = f64::MAX;
    if let Some(limit) = config.request_usd {
        remaining = remaining.min(limit);
    }
    if let Some(limit) = config.session_usd {
        remaining = remaining.min(limit - session_spend - reservations.0);
    }
    if let Some(limit) = config.daily_usd {
        remaining = remaining.min(limit - daily_spend - reservations.1);
    }
    let input_cost = worst_case_cost(runtime_config, provider, input_tokens, 0);
    if remaining <= input_cost {
        return Some(0);
    }
    Some(((remaining - input_cost) * 1_000_000.0 / output_price).floor() as i64)
}

fn try_reserve_usd(
    state: &AppState,
    runtime_config: &AppConfig,
    budget: &crate::config::BudgetConfig,
    provider: &ProviderProfile,
    input_tokens: i64,
    output_tokens: i64,
    scope: &str,
) -> Result<Option<UsdReservation>> {
    if !has_usd_budget(budget) {
        return Ok(None);
    }
    let amount = worst_case_cost(runtime_config, provider, input_tokens, output_tokens);
    let scope_filter = (!scope.is_empty()).then_some(scope);
    let session_spend = state
        .ledger
        .cost_since_scope(state.session_started, scope_filter)?;
    let daily_spend = state.ledger.cost_since_scope(day_start(), scope_filter)?;
    let mut reservations = state
        .usd_reservations
        .lock()
        .map_err(|_| anyhow::anyhow!("美元预算预留锁已损坏"))?;
    let (reserved_session, reserved_daily) = reservations
        .by_scope
        .get(scope)
        .copied()
        .unwrap_or((0.0, 0.0));
    if budget
        .request_usd
        .is_some_and(|limit| amount > limit && amount > 0.0)
        || budget
            .session_usd
            .is_some_and(|limit| session_spend + reserved_session + amount > limit && amount > 0.0)
        || budget
            .daily_usd
            .is_some_and(|limit| daily_spend + reserved_daily + amount > limit && amount > 0.0)
    {
        return Ok(None);
    }
    let session = budget.session_usd.is_some();
    let daily = budget.daily_usd.is_some();
    let entry = reservations.by_scope.entry(scope.to_owned()).or_default();
    if session {
        entry.0 += amount;
    }
    if daily {
        entry.1 += amount;
    }
    Ok(Some(UsdReservation {
        reservations: state.usd_reservations.clone(),
        scope: scope.to_owned(),
        amount,
        session,
        daily,
    }))
}

fn try_reserve_tokens(
    state: &AppState,
    budget: &crate::config::BudgetConfig,
    input_tokens: i64,
    scope: &str,
) -> Result<Option<TokenReservation>> {
    if !has_token_budget(budget) {
        return Ok(None);
    }
    let scope_filter = (!scope.is_empty()).then_some(scope);
    let session_tokens = state
        .ledger
        .input_tokens_since_scope(state.session_started, scope_filter)?;
    let daily_tokens = state
        .ledger
        .input_tokens_since_scope(day_start(), scope_filter)?;
    let mut reservations = state
        .token_reservations
        .lock()
        .map_err(|_| anyhow::anyhow!("Token 预算预留锁已损坏"))?;
    let (reserved_session, reserved_daily) =
        reservations.by_scope.get(scope).copied().unwrap_or((0, 0));
    if budget
        .session_tokens
        .is_some_and(|limit| session_tokens + reserved_session + input_tokens > limit && limit >= 0)
        || budget
            .daily_tokens
            .is_some_and(|limit| daily_tokens + reserved_daily + input_tokens > limit && limit >= 0)
    {
        return Ok(None);
    }
    let session = budget.session_tokens.is_some();
    let daily = budget.daily_tokens.is_some();
    let entry = reservations.by_scope.entry(scope.to_owned()).or_default();
    if session {
        entry.0 = entry.0.saturating_add(input_tokens.max(0));
    }
    if daily {
        entry.1 = entry.1.saturating_add(input_tokens.max(0));
    }
    Ok(Some(TokenReservation {
        reservations: state.token_reservations.clone(),
        scope: scope.to_owned(),
        amount: input_tokens.max(0),
        session,
        daily,
    }))
}

fn day_start() -> i64 {
    let now = chrono::Utc::now();
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|value| value.and_utc().timestamp())
        .unwrap_or_else(|| now.timestamp())
}

async fn proxy(
    State(state): State<Arc<AppState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(env_name) = state.config.gateway_auth_token_env.as_deref() {
        let expected = std::env::var(env_name).unwrap_or_default();
        let supplied = headers
            .get("x-duola-gateway-token")
            .and_then(|value| value.to_str().ok());
        if expected.is_empty() || supplied != Some(expected.as_str()) {
            return (StatusCode::UNAUTHORIZED, "DuoLA Gateway 认证失败").into_response();
        }
    }
    if method == Method::OPTIONS {
        return StatusCode::NO_CONTENT.into_response();
    }
    if body.len() > state.config.max_request_bytes {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "请求超过 {} MB 限制",
                limit_display_mb(state.config.max_request_bytes)
            ),
        )
            .into_response();
    }
    let runtime_config = state.runtime_config.read().await.clone();
    let request_id = format!(
        "req_{}",
        request_hash(
            format!(
                "{}:{}:{}",
                method,
                uri,
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            )
            .as_bytes(),
        )[..16]
            .to_owned()
    );
    let session_id = headers
        .get("x-duola-agent-session")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("")
        .to_owned();
    let session_id = if session_id.is_empty() {
        format!("gateway-{}", state.session_started)
    } else {
        session_id
    };
    let project_id = headers
        .get("x-duola-project")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned);
    let agent_id = headers
        .get("x-duola-agent")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned);
    let request_bypass = headers
        .get("x-duola-transform")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| matches!(value, "off" | "bypass" | "passthrough"));
    let bypassed = state.bypass_path.exists() || request_bypass;
    let (provider, routed_body) = if bypassed {
        match runtime_config.provider(None) {
            Ok(provider) => (provider, body.to_vec()),
            Err(e) => return (StatusCode::SERVICE_UNAVAILABLE, e.to_string()).into_response(),
        }
    } else {
        match resolve_provider_and_model(&runtime_config, &body) {
            Ok(v) => v,
            Err(e) => return (StatusCode::SERVICE_UNAVAILABLE, e.to_string()).into_response(),
        }
    };
    let transform_policy = state.transform.read().await.clone();
    let original_len = body.len();
    let transformed = if bypassed {
        crate::transform::TransformResult {
            body: routed_body.clone(),
            ..Default::default()
        }
    } else if headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("json"))
    {
        crate::transform::transform_json_with_policy(&routed_body, &transform_policy)
    } else {
        crate::transform::TransformResult {
            body: routed_body.clone(),
            ..Default::default()
        }
    };
    let mut transformed = transformed;
    if routed_body != body {
        transformed.receipts.insert(
            0,
            crate::transform::Receipt {
                path: "$/model".into(),
                rule_id: "routing.model-map.v1".into(),
                original_hash: request_hash(&body),
                result_hash: request_hash(&routed_body),
                original_bytes: body.len(),
                result_bytes: routed_body.len(),
                status: "applied".into(),
            },
        );
        transformed.changed = true;
    }
    let model_id = requested_model(&body);
    let scope_keys = [
        project_id
            .as_deref()
            .map(|value| format!("project:{value}")),
        agent_id.as_deref().map(|value| format!("agent:{value}")),
        Some(format!("session:{session_id}")),
        model_id.as_deref().map(|value| format!("model:{value}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let global_budget = state.budget.read().await.clone();
    let budget_scope = scope_keys
        .iter()
        .rev()
        .find(|key| global_budget.scopes.contains_key(*key))
        .cloned()
        .unwrap_or_default();
    let budget_snapshot = global_budget.scoped(&scope_keys);
    let preliminary_input_tokens = estimate_tokens(transformed.body.len());
    let dynamic_output_cap = (!bypassed).then(|| {
        derived_usd_output_cap(
            &state,
            &runtime_config,
            &budget_snapshot,
            &provider,
            preliminary_input_tokens,
            &budget_scope,
        )
    });
    let output_cap = match (
        budget_snapshot.request_output_tokens,
        dynamic_output_cap.flatten(),
    ) {
        (Some(explicit), Some(dynamic)) => Some(explicit.min(dynamic)),
        (Some(explicit), None) => Some(explicit),
        (None, Some(dynamic)) => Some(dynamic),
        (None, None) => None,
    };
    if let Some((limited_body, field)) =
        apply_output_budget(&transformed.body, &provider, output_cap)
    {
        transformed.receipts.push(crate::transform::Receipt {
            path: format!("$/{field}"),
            rule_id: "budget.output-cap.v1".into(),
            original_hash: request_hash(&transformed.body),
            result_hash: request_hash(&limited_body),
            original_bytes: transformed.body.len(),
            result_bytes: limited_body.len(),
            status: "applied".into(),
        });
        transformed.body = limited_body;
        transformed.changed = true;
    }
    let sent_len = transformed.body.len();
    let input_tokens = estimate_tokens(sent_len);
    let request_meta = RequestMeta {
        session_id: session_id.clone(),
        project_id: project_id.clone(),
        agent: agent_id.clone(),
        model: model_id.clone(),
        transform_status: if bypassed {
            "bypassed".into()
        } else if transformed.changed {
            "applied".into()
        } else {
            "pass_through".into()
        },
        transform_rule_count: transformed.receipts.len() as i64,
        original_hash: Some(request_hash(&body)),
        sent_hash: Some(request_hash(&transformed.body)),
        reason: if transformed.changed {
            None
        } else if bypassed {
            Some("bypass 已启用，原样透传".into())
        } else {
            transformed
                .reason
                .clone()
                .or_else(|| Some("原样透传".into()))
        },
    };
    let request_content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let cache_config = state.cache_config.read().await.clone();
    let privacy = state.privacy.read().await.clone();
    let cacheable = !bypassed
        && !privacy.strict
        && cache_config.enabled
        && cache_eligible(&transformed.body, &request_content_type);
    let cache_key_value = cacheable
        .then(|| cache_key_with_headers(&method, &uri, &provider, &headers, &transformed.body));
    let inflight_guard = if bypassed {
        None
    } else {
        match try_acquire_concurrency(&state.inflight, budget_snapshot.max_concurrency) {
            Some(guard) => Some(guard),
            None => {
                let _ = state.ledger.record_blocked_with_meta(
                    &request_id,
                    &provider.id,
                    uri.path(),
                    original_len as i64,
                    sent_len as i64,
                    input_tokens,
                    "concurrency_blocked",
                    &RequestMeta {
                        reason: Some("并发上限".into()),
                        ..request_meta.clone()
                    },
                );
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    "DuoLA AgentCost 已达到并发上限，请稍后重试",
                )
                    .into_response();
            }
        }
    };
    if !bypassed {
        let fingerprint = request_hash(
            format!(
                "{}:{}:{}:{}:{}:{}",
                session_id,
                provider.id,
                method,
                uri.path(),
                uri.query().unwrap_or_default(),
                request_hash(&body)
            )
            .as_bytes(),
        );
        let count = {
            let mut counts = state.fingerprints.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let count = {
                let entry = counts.entry(fingerprint).or_insert((now, 0));
                if now.duration_since(entry.0) > std::time::Duration::from_secs(60) {
                    *entry = (now, 0);
                }
                entry.1 += 1;
                entry.1
            };
            if counts.len() > 10_000 {
                counts.retain(|_, (started, _)| {
                    now.duration_since(*started) < std::time::Duration::from_secs(60)
                });
            }
            count
        };
        let max_same_fingerprint = budget_snapshot.max_same_fingerprint.max(1);
        if count > max_same_fingerprint {
            warn!(%uri, count, "重复请求已暂停");
            let _ = state.ledger.record_blocked_with_meta(
                &request_id,
                &provider.id,
                uri.path(),
                original_len as i64,
                sent_len as i64,
                input_tokens,
                "loop_blocked",
                &RequestMeta {
                    reason: Some("重复请求循环".into()),
                    ..request_meta.clone()
                },
            );
            return (
                StatusCode::TOO_MANY_REQUESTS,
                "DuoLA AgentCost 已暂停重复请求，请检查 Agent 循环或执行 bypass",
            )
                .into_response();
        }
    }
    if !bypassed {
        let output_tokens_limit = output_limit_from_body(&transformed.body, &provider).unwrap_or(0);
        let estimated_cost = worst_case_cost(
            &runtime_config,
            &provider,
            input_tokens,
            output_tokens_limit,
        );
        let budget = budget_snapshot.clone();
        let scope_filter = (!budget_scope.is_empty()).then_some(budget_scope.as_str());
        let session_tokens = state
            .ledger
            .input_tokens_since_scope(state.session_started, scope_filter)
            .unwrap_or(0);
        let daily_tokens = state
            .ledger
            .input_tokens_since_scope(day_start(), scope_filter)
            .unwrap_or(0);
        let reserved_tokens = state
            .token_reservations
            .lock()
            .map(|value| value.by_scope.get(&budget_scope).copied().unwrap_or((0, 0)))
            .unwrap_or((0, 0));
        let token_blocked = budget
            .request_tokens
            .is_some_and(|limit| input_tokens > limit && limit >= 0)
            || budget.session_tokens.is_some_and(|limit| {
                session_tokens + reserved_tokens.0 + input_tokens > limit && limit >= 0
            })
            || budget.daily_tokens.is_some_and(|limit| {
                daily_tokens + reserved_tokens.1 + input_tokens > limit && limit >= 0
            });
        let request_limit = budget.request_usd;
        let session_limit = budget.session_usd;
        let daily_limit = budget.daily_usd;
        let session_spend = state
            .ledger
            .cost_since_scope(state.session_started, scope_filter)
            .unwrap_or(0.0);
        let daily_spend = state
            .ledger
            .cost_since_scope(day_start(), scope_filter)
            .unwrap_or(0.0);
        let reserved_spend = state
            .usd_reservations
            .lock()
            .map(|value| {
                value
                    .by_scope
                    .get(&budget_scope)
                    .copied()
                    .unwrap_or((0.0, 0.0))
            })
            .unwrap_or((0.0, 0.0));
        let unbounded_output = has_usd_budget(&budget)
            && worst_case_output_price(&runtime_config, &provider) > 0.0
            && output_limit_from_body(&transformed.body, &provider).is_none();
        let blocked = token_blocked
            || unbounded_output
            || request_limit.is_some_and(|limit| estimated_cost > limit && estimated_cost > 0.0)
            || session_limit.is_some_and(|limit| {
                session_spend + reserved_spend.0 + estimated_cost > limit && estimated_cost > 0.0
            })
            || daily_limit.is_some_and(|limit| {
                daily_spend + reserved_spend.1 + estimated_cost > limit && estimated_cost > 0.0
            });
        if blocked {
            let block_reason = if token_blocked {
                "Token 预算"
            } else if unbounded_output {
                "输出上限未知"
            } else if request_limit
                .is_some_and(|limit| estimated_cost > limit && estimated_cost > 0.0)
            {
                "单次美元预算"
            } else if session_limit.is_some_and(|limit| {
                session_spend + reserved_spend.0 + estimated_cost > limit && estimated_cost > 0.0
            }) {
                "会话美元预算"
            } else {
                "每日美元预算"
            };
            let _ = state.ledger.record_blocked_with_meta(
                &request_id,
                &provider.id,
                uri.path(),
                original_len as i64,
                sent_len as i64,
                input_tokens,
                "budget_blocked",
                &RequestMeta {
                    reason: Some(block_reason.into()),
                    ..request_meta.clone()
                },
            );
            return (
                StatusCode::TOO_MANY_REQUESTS,
                format!(
                    "DuoLA AgentCost 已按{block_reason}暂停：本次约 {input_tokens} input tokens，会话已用 {session_tokens}，今日已用 {daily_tokens}；美元预估 ${estimated_cost:.6}，会话 ${session_spend:.6}，今日 ${daily_spend:.6}。请降低请求、设置输出上限或执行 duola-agentcost bypass 临时放行。"
                ),
            )
                .into_response();
        }
    }
    // Cache hits are deliberately checked only after loop and budget gates.
    // Rate slots are charged only for an actual upstream request, so cache
    // hits cannot consume the request window.
    if let Some(cache_key_value) = cache_key_value.as_deref() {
        let cached = cache_get_observed(&state.cache, cache_key_value, Some(&state.cache_metrics));
        if cached.is_none()
            && let Ok(mut metrics) = state.cache_metrics.lock()
        {
            metrics.misses = metrics.misses.saturating_add(1);
        }
        if let Some(cached) = cached {
            if let Ok(mut metrics) = state.cache_metrics.lock() {
                metrics.hits = metrics.hits.saturating_add(1);
            }
            let started = Instant::now();
            let output_tokens = usage_from_bytes(&cached.body)
                .map(|(_, output, _)| output)
                .unwrap_or(0);
            let _ = state.ledger.record_cache_hit_with_meta(
                &request_id,
                &provider.id,
                uri.path(),
                original_len as i64,
                sent_len as i64,
                input_tokens,
                output_tokens,
                started.elapsed().as_millis() as i64,
                &request_meta,
            );
            let _ = state
                .ledger
                .record_receipts(&request_id, &transformed.receipts);
            let mut response = (cached.status, cached.headers, cached.body).into_response();
            response
                .headers_mut()
                .insert("x-duola-cache", HeaderValue::from_static("HIT"));
            if let Ok(value) = HeaderValue::from_str(&request_id) {
                response.headers_mut().insert("x-duola-request-id", value);
            }
            return response;
        }
    }
    let token_reservation = if bypassed {
        None
    } else {
        match try_reserve_tokens(&state, &budget_snapshot, input_tokens, &budget_scope) {
            Ok(reservation) => reservation,
            Err(error) => {
                warn!(error = %error, "Token 预算预留失败");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "DuoLA AgentCost 无法确认 Token 预算",
                )
                    .into_response();
            }
        }
    };
    if !bypassed && has_token_budget(&budget_snapshot) && token_reservation.is_none() {
        let _ = state.ledger.record_blocked_with_meta(
            &request_id,
            &provider.id,
            uri.path(),
            original_len as i64,
            sent_len as i64,
            input_tokens,
            "budget_blocked",
            &RequestMeta {
                reason: Some("Token 预算预留".into()),
                ..request_meta.clone()
            },
        );
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "DuoLA AgentCost 已按 Token 硬预算暂停：并发请求预留后没有足够余额",
        )
            .into_response();
    }
    let usd_reservation = if bypassed {
        None
    } else {
        let output_tokens_limit = output_limit_from_body(&transformed.body, &provider).unwrap_or(0);
        match try_reserve_usd(
            &state,
            &runtime_config,
            &budget_snapshot,
            &provider,
            input_tokens,
            output_tokens_limit,
            &budget_scope,
        ) {
            Ok(reservation) => reservation,
            Err(error) => {
                warn!(error = %error, "美元预算预留失败");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "DuoLA AgentCost 无法确认美元预算",
                )
                    .into_response();
            }
        }
    };
    if !bypassed && has_usd_budget(&budget_snapshot) && usd_reservation.is_none() {
        let _ = state.ledger.record_blocked_with_meta(
            &request_id,
            &provider.id,
            uri.path(),
            original_len as i64,
            sent_len as i64,
            input_tokens,
            "budget_blocked",
            &RequestMeta {
                reason: Some("美元预算预留".into()),
                ..request_meta.clone()
            },
        );
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "DuoLA AgentCost 已按美元硬预算暂停：并发请求预留后没有足够余额",
        )
            .into_response();
    }
    if !bypassed && !take_rate_slot(&state.rate_window, budget_snapshot.requests_per_minute) {
        let _ = state.ledger.record_blocked_with_meta(
            &request_id,
            &provider.id,
            uri.path(),
            original_len as i64,
            sent_len as i64,
            input_tokens,
            "rate_limited",
            &RequestMeta {
                reason: Some("速率限制".into()),
                ..request_meta.clone()
            },
        );
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "DuoLA AgentCost 已达到每分钟请求上限，请稍后重试",
        )
            .into_response();
    }
    if let Err(e) = state.ledger.record_start_with_meta(
        &request_id,
        &provider.id,
        uri.path(),
        original_len as i64,
        sent_len as i64,
        input_tokens,
        &request_meta,
    ) {
        warn!(error = %e, "账本写入失败，继续放行请求");
    }
    if let Err(e) = state
        .ledger
        .record_receipts(&request_id, &transformed.receipts)
    {
        warn!(error = %e, "receipt 写入失败，继续放行请求");
    }
    let started = Instant::now();
    let mut candidates = vec![provider.clone()];
    for fallback_id in &provider.fallback {
        if let Some(fallback) = runtime_config
            .providers
            .iter()
            .find(|p| &p.id == fallback_id && p.protocol == provider.protocol)
        {
            candidates.push(fallback.clone());
        } else {
            warn!(provider = %fallback_id, "跳过协议不兼容或不存在的 fallback Provider");
        }
    }
    candidates.dedup_by(|left, right| left.id == right.id);
    let routing = state.routing_config.read().await.clone();
    if routing.mode == "cost" && !routing.pool.is_empty() {
        let pool: HashSet<&str> = routing.pool.iter().map(String::as_str).collect();
        let pooled = runtime_config
            .providers
            .iter()
            .filter(|candidate| {
                candidate.protocol == provider.protocol && pool.contains(candidate.id.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        if !pooled.is_empty() {
            candidates = pooled;
        }
    }
    if routing.mode == "cost" {
        candidates.sort_by(|left, right| {
            left.input_price_per_million
                .unwrap_or(f64::MAX)
                .partial_cmp(&right.input_price_per_million.unwrap_or(f64::MAX))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    candidates.truncate(routing.max_attempts.max(1));
    let mut routed_provider: Option<ProviderProfile> = None;
    let mut upstream = None;
    let mut last_error = None;
    let mut selected_attempt = 0_i64;
    for (index, candidate) in candidates.into_iter().enumerate() {
        let attempt = index as i64 + 1;
        let attempt_started = Instant::now();
        let _ = state
            .ledger
            .record_attempt_start(&request_id, attempt, &candidate.id);
        if !provider_is_available(&state.provider_health, &candidate.id) {
            let _ = state.ledger.record_attempt_finish(
                &request_id,
                attempt,
                "circuit_open",
                Some("Provider 暂时熔断，等待健康探测窗口"),
                0,
            );
            last_error = Some(format!("Provider {} 暂时熔断", candidate.id));
            continue;
        }
        let url = upstream_url(&candidate, uri.path(), uri.query());
        let request = state
            .client
            .request(method.clone(), &url)
            .headers(copy_request_headers(&headers, &candidate, attempt > 1))
            .body(transformed.body.clone());
        match request.send().await {
            Ok(response) => {
                let transient = response.status() == StatusCode::TOO_MANY_REQUESTS
                    || response.status().is_server_error();
                let response_status = response.status();
                routed_provider = Some(candidate.clone());
                upstream = Some(response);
                selected_attempt = attempt;
                if !transient {
                    provider_record_success(&state.provider_health, &candidate.id);
                    let _ = state.ledger.record_attempt_finish(
                        &request_id,
                        attempt,
                        "received",
                        None,
                        attempt_started.elapsed().as_millis() as i64,
                    );
                    break;
                }
                provider_record_failure(
                    &state.provider_health,
                    &candidate.id,
                    routing.circuit_breaker_threshold,
                    routing.circuit_breaker_cooldown_seconds,
                );
                if !fallback_allowed(&method, &provider, &routing, &headers) {
                    let _ = state.ledger.record_attempt_finish(
                        &request_id,
                        attempt,
                        "transient_no_retry",
                        Some("non-idempotent fallback disabled"),
                        attempt_started.elapsed().as_millis() as i64,
                    );
                    break;
                }
                let _ = state.ledger.record_attempt_finish(
                    &request_id,
                    attempt,
                    "transient",
                    Some(&format!("http {response_status}")),
                    attempt_started.elapsed().as_millis() as i64,
                );
                warn!(provider = %routed_provider.as_ref().map(|p| p.id.as_str()).unwrap_or("unknown"), "Provider transient error; checking fallback");
            }
            Err(error) => {
                provider_record_failure(
                    &state.provider_health,
                    &candidate.id,
                    routing.circuit_breaker_threshold,
                    routing.circuit_breaker_cooldown_seconds,
                );
                let error_text = redact_sensitive_error(&error.to_string());
                let _ = state.ledger.record_attempt_finish(
                    &request_id,
                    attempt,
                    "transport_error",
                    Some(&error_text),
                    attempt_started.elapsed().as_millis() as i64,
                );
                warn!(provider = %candidate.id, error = %error_text, "Provider connection failed; checking fallback");
                last_error = Some(error_text);
            }
        }
    }
    let routed_provider = routed_provider.unwrap_or_else(|| provider.clone());
    if routed_provider.id != provider.id {
        let _ = state.ledger.set_provider(&request_id, &routed_provider.id);
    }
    let upstream = match upstream {
        Some(response) => response,
        None => {
            let message = last_error.unwrap_or_else(|| "没有可用的 Provider".into());
            let _ = state.ledger.record_finish(
                &request_id,
                "upstream_error",
                None,
                0,
                0,
                0.0,
                started.elapsed().as_millis() as i64,
            );
            return (
                StatusCode::BAD_GATEWAY,
                format!("上游 Provider 请求失败: {message}"),
            )
                .into_response();
        }
    };
    let status = upstream.status();
    let response_headers = copy_response_headers(upstream.headers());
    let content_type = upstream
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let is_stream = content_type.contains("text/event-stream");
    if !is_stream {
        let bytes = match timeout(
            Duration::from_secs(state.config.stream_idle_timeout_seconds.max(1)),
            read_response_limited(upstream, state.config.max_response_bytes),
        )
        .await
        {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                let error_text = redact_sensitive_error(&e.to_string());
                let final_status = if error_text.contains("超过") {
                    "response_too_large"
                } else {
                    "response_error"
                };
                let _ = state.ledger.record_finish(
                    &request_id,
                    final_status,
                    None,
                    0,
                    0,
                    0.0,
                    started.elapsed().as_millis() as i64,
                );
                let _ = state.ledger.record_attempt_finish(
                    &request_id,
                    selected_attempt,
                    "response_error",
                    Some(&error_text),
                    started.elapsed().as_millis() as i64,
                );
                return (StatusCode::BAD_GATEWAY, error_text).into_response();
            }
            Err(_) => {
                let message = format!(
                    "Provider 响应空闲超过 {} 秒",
                    state.config.stream_idle_timeout_seconds.max(1)
                );
                let _ = state.ledger.record_finish(
                    &request_id,
                    "response_idle_timeout",
                    None,
                    0,
                    0,
                    0.0,
                    started.elapsed().as_millis() as i64,
                );
                let _ = state.ledger.record_attempt_finish(
                    &request_id,
                    selected_attempt,
                    "response_idle_timeout",
                    Some(&message),
                    started.elapsed().as_millis() as i64,
                );
                return (StatusCode::GATEWAY_TIMEOUT, message).into_response();
            }
        };
        let usage = usage_from_bytes(&bytes);
        let (input, output, cached) =
            usage.unwrap_or((input_tokens, estimate_tokens(bytes.len()), 0));
        let cost = provider_cost(&routed_provider, input, output, cached);
        let final_status = if status.is_success() && !stream_has_failure_marker(&bytes) {
            "completed"
        } else {
            "provider_error"
        };
        let _ = state.ledger.record_finish(
            &request_id,
            final_status,
            usage.map(|(input, _, _)| input),
            output,
            cached,
            cost,
            started.elapsed().as_millis() as i64,
        );
        let mut cache_stored = false;
        if cacheable
            && routed_provider.id == provider.id
            && status.is_success()
            && bytes.len() <= cache_config.max_entry_bytes
            && content_type.contains("json")
            && let Some(cache_key_value) = cache_key_value
        {
            cache_put_observed(
                &state.cache,
                cache_key_value,
                CachedResponse {
                    expires_at: Instant::now()
                        + std::time::Duration::from_secs(cache_config.ttl_seconds),
                    status,
                    headers: cache_response_headers(&response_headers),
                    body: bytes.clone(),
                    body_hash: request_hash(&bytes),
                },
                cache_config.max_entries,
                cache_config.max_total_bytes,
                Some(&state.cache_metrics),
            );
            cache_stored = true;
        }
        let mut response = (status, response_headers, bytes).into_response();
        if cacheable {
            response.headers_mut().insert(
                "x-duola-cache",
                HeaderValue::from_static(if cache_stored { "MISS" } else { "BYPASS" }),
            );
        }
        if let Ok(value) = HeaderValue::from_str(&request_id) {
            response.headers_mut().insert("x-duola-request-id", value);
        }
        return response;
    }
    let mut stream_body = upstream.bytes_stream();
    let ledger = state.ledger.clone();
    let provider_for_stream = routed_provider.clone();
    let request_id_for_stream = request_id.clone();
    let selected_attempt_for_stream = selected_attempt;
    let sent_input_tokens = input_tokens;
    let stream_progress = Arc::new(Mutex::new(StreamProgress::default()));
    let stream_lifecycle_guard = StreamLifecycleGuard {
        ledger: ledger.clone(),
        request_id: request_id_for_stream.clone(),
        attempt: selected_attempt_for_stream,
        provider: provider_for_stream.clone(),
        input_tokens: sent_input_tokens,
        started,
        progress: stream_progress.clone(),
    };
    let stream = stream! {
        let _inflight_guard = inflight_guard;
        let _token_reservation = token_reservation;
        let _usd_reservation = usd_reservation;
        let _stream_lifecycle_guard = stream_lifecycle_guard;
        let mut captured = Vec::new();
        let mut marker_tail = Vec::new();
        let mut stream_usage = None;
        let mut streamed_bytes = 0usize;
        let mut finalized = false;
        loop {
            let next_chunk = timeout(
                Duration::from_secs(state.config.stream_idle_timeout_seconds.max(1)),
                stream_body.next(),
            ).await;
            let chunk = match next_chunk {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(_) => {
                    let message = format!(
                        "Provider 流式响应空闲超过 {} 秒",
                        state.config.stream_idle_timeout_seconds.max(1)
                    );
                    if let Ok(mut progress) = stream_progress.lock() {
                        progress.finalized = true;
                    }
                    let _ = ledger.record_finish(&request_id_for_stream, "stream_idle_timeout", None, 0, 0, 0.0, started.elapsed().as_millis() as i64);
                    let _ = ledger.record_attempt_finish(&request_id_for_stream, selected_attempt_for_stream, "stream_idle_timeout", Some(&message), started.elapsed().as_millis() as i64);
                    yield Err(std::io::Error::other(message));
                    return;
                }
            };
            match chunk {
                Ok(bytes) => {
                    streamed_bytes = streamed_bytes.saturating_add(bytes.len());
                    if let Ok(mut progress) = stream_progress.lock() {
                        progress.streamed_bytes = streamed_bytes;
                    }
                    if streamed_bytes > state.config.max_response_bytes {
                        let message = format!(
                            "Provider 流式响应超过 {} MB 限制",
                            limit_display_mb(state.config.max_response_bytes)
                        );
                        if let Ok(mut progress) = stream_progress.lock() {
                            progress.finalized = true;
                        }
                        let _ = ledger.record_finish(&request_id_for_stream, "response_too_large", None, 0, 0, 0.0, started.elapsed().as_millis() as i64);
                        let _ = ledger.record_attempt_finish(&request_id_for_stream, selected_attempt_for_stream, "stream_too_large", Some(&message), started.elapsed().as_millis() as i64);
                        yield Err(std::io::Error::other(message));
                        return;
                    }
                    if captured.len() < 2 * 1024 * 1024 {
                        captured.extend_from_slice(&bytes[..bytes.len().min(2 * 1024 * 1024 - captured.len())]);
                    }
                    let mut scan = Vec::with_capacity(marker_tail.len() + bytes.len());
                    scan.extend_from_slice(&marker_tail);
                    scan.extend_from_slice(&bytes);
                    if let Some(usage) = usage_from_bytes(&scan) {
                        stream_usage = merge_usage(stream_usage, usage);
                        if let Ok(mut progress) = stream_progress.lock() {
                            progress.usage = stream_usage;
                        }
                    }
                    if scan.len() > 64 * 1024 {
                        marker_tail = scan[scan.len() - 64 * 1024..].to_vec();
                    } else {
                        marker_tail = scan;
                    }
                    if !finalized && stream_has_terminal_marker(&marker_tail) {
                        let usage = stream_usage;
                        let (input, output, cached) = usage
                            .unwrap_or((sent_input_tokens, estimate_tokens(streamed_bytes), 0));
                        let cost = provider_cost(&provider_for_stream, input, output, cached);
                        let failure = stream_has_failure_marker(&marker_tail);
                        let final_status = if status.is_success() && !failure { "completed" } else { "provider_error" };
                        let attempt_status = if failure { "stream_provider_error" } else { "stream_complete" };
                        let _ = ledger.record_finish(&request_id_for_stream, final_status, usage.map(|(input, _, _)| input), output, cached, cost, started.elapsed().as_millis() as i64);
                        let attempt_error = failure.then_some("provider response.failed");
                        let _ = ledger.record_attempt_finish(&request_id_for_stream, selected_attempt_for_stream, attempt_status, attempt_error, started.elapsed().as_millis() as i64);
                        if let Ok(mut progress) = stream_progress.lock() {
                            progress.finalized = true;
                        }
                        finalized = true;
                    }
                    yield Ok::<Bytes, std::io::Error>(bytes);
                }
                Err(e) => {
                    if let Ok(mut progress) = stream_progress.lock() {
                        progress.finalized = true;
                    }
                    let _ = ledger.record_finish(&request_id_for_stream, "stream_error", None, 0, 0, 0.0, started.elapsed().as_millis() as i64);
                    let error_text = redact_sensitive_error(&e.to_string());
                    let _ = ledger.record_attempt_finish(&request_id_for_stream, selected_attempt_for_stream, "stream_error", Some(&error_text), started.elapsed().as_millis() as i64);
                    yield Err(std::io::Error::other(error_text));
                    return;
                }
            }
        }
        if !finalized {
            let usage = stream_usage.or_else(|| usage_from_bytes(&captured));
            let (input, output, cached) = usage
                .unwrap_or((sent_input_tokens, estimate_tokens(streamed_bytes), 0));
            let cost = provider_cost(&provider_for_stream, input, output, cached);
            let final_status = if status.is_success() {
                "stream_incomplete"
            } else {
                "provider_error"
            };
            let _ = ledger.record_finish(&request_id_for_stream, final_status, usage.map(|(input, _, _)| input), output, cached, cost, started.elapsed().as_millis() as i64);
            let _ = ledger.record_attempt_finish(&request_id_for_stream, selected_attempt_for_stream, "stream_incomplete", Some("missing terminal stream event"), started.elapsed().as_millis() as i64);
            if let Ok(mut progress) = stream_progress.lock() {
                progress.finalized = true;
            }
        }
    };
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    *response.headers_mut() = response_headers;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("x-duola-request-id", value);
    }
    response
}

pub async fn dashboard_json(state: &AppState) -> Result<(Stats, Vec<RecentRequest>)> {
    Ok((state.ledger.stats()?, state.ledger.recent(50)?))
}

#[cfg(test)]
mod tests {
    use super::{
        CachedResponse, ResponseCache, apply_output_budget, cache_eligible, cache_get, cache_put,
        fallback_allowed, provider_is_available, provider_record_failure, provider_record_success,
        redact_sensitive_error, stream_has_failure_marker, stream_has_terminal_marker,
        validate_gateway_listen,
    };
    use crate::config::{ProviderProfile, RoutingConfig};
    use axum::body::Bytes;
    use axum::http::{HeaderMap, Method};

    #[test]
    fn recognizes_chat_completion_done_marker() {
        assert!(stream_has_terminal_marker(
            b"data: {\"delta\":{}}\n\ndata: [DONE]\n\n"
        ));
    }

    #[test]
    fn recognizes_responses_completed_marker() {
        assert!(stream_has_terminal_marker(
            b"event: response.completed\ndata: {}\n\n"
        ));
    }

    #[test]
    fn recognizes_responses_failed_marker() {
        let data = b"event: response.failed\ndata: {}\n\n";
        assert!(stream_has_terminal_marker(data));
        assert!(stream_has_failure_marker(data));
    }

    #[test]
    fn recognizes_anthropic_stream_terminal_and_failure_markers() {
        assert!(stream_has_terminal_marker(
            b"event: message_stop\ndata: {}\n\n"
        ));
        assert!(stream_has_failure_marker(
            b"event: message_error\ndata: {}\n\n"
        ));
    }

    #[test]
    fn ignores_incomplete_stream() {
        assert!(!stream_has_terminal_marker(
            b"data: {\"type\":\"response.output_text.delta\"}\n\n"
        ));
    }

    #[test]
    fn redacts_provider_error_secrets() {
        let value = redact_sensitive_error(
            "request failed https://provider.test/?api_key=secret123&x=1 token:abc cookie=hello authorization: Bearer supersecret",
        );
        assert!(!value.contains("secret123"));
        assert!(!value.contains("abc"));
        assert!(!value.contains("hello"));
        assert!(!value.contains("supersecret"));
        assert!(value.contains("[REDACTED]"));
    }

    #[test]
    fn cache_rejects_stateful_and_tool_requests() {
        assert!(cache_eligible(
            br#"{"model":"x","messages":[{"role":"user","content":"hello"}]}"#,
            "application/json"
        ));
        assert!(!cache_eligible(
            br#"{"model":"x","stream":true,"messages":[]}"#,
            "application/json"
        ));
        assert!(!cache_eligible(
            br#"{"model":"x","tools":[],"messages":[]}"#,
            "application/json"
        ));
        assert!(!cache_eligible(
            br#"{"model":"x","execute":true,"messages":[]}"#,
            "application/json"
        ));
        assert!(!cache_eligible(
            br#"{"model":"x","temperature":0.2,"messages":[]}"#,
            "application/json"
        ));
        assert!(!cache_eligible(
            br#"{"model":"x","top_p":0.8,"messages":[]}"#,
            "application/json"
        ));
        assert!(!cache_eligible(
            br#"{"model":"x","n":2,"messages":[]}"#,
            "application/json"
        ));
    }

    #[test]
    fn explicit_output_budget_uses_provider_protocol_field() {
        let provider = ProviderProfile {
            id: "openai".into(),
            endpoint: "http://localhost".into(),
            protocol: "openai-responses".into(),
            api_key_env: None,
            model_map: Default::default(),
            fallback: vec![],
            input_price_per_million: None,
            output_price_per_million: None,
            cached_input_price_per_million: None,
        };
        let (body, field) =
            apply_output_budget(br#"{"model":"x","messages":[]}"#, &provider, Some(256)).unwrap();
        assert_eq!(field, "max_output_tokens");
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["max_output_tokens"], 256);
    }

    #[test]
    fn remote_gateway_requires_token() {
        assert!(validate_gateway_listen("127.0.0.1:8765", None).is_ok());
        assert!(validate_gateway_listen("0.0.0.0:8765", None).is_err());
    }

    #[test]
    fn provider_circuit_opens_after_transient_failures_and_recovers_on_success() {
        let health = std::sync::Mutex::new(std::collections::HashMap::new());
        assert!(provider_is_available(&health, "primary"));
        provider_record_failure(&health, "primary", 2, 60);
        assert!(provider_is_available(&health, "primary"));
        provider_record_failure(&health, "primary", 2, 60);
        assert!(!provider_is_available(&health, "primary"));
        provider_record_success(&health, "primary");
        assert!(provider_is_available(&health, "primary"));
    }

    #[test]
    fn unknown_protocol_non_idempotent_fallback_is_opt_in() {
        let provider = ProviderProfile {
            id: "custom".into(),
            endpoint: "http://localhost".into(),
            protocol: "custom-json".into(),
            api_key_env: None,
            model_map: Default::default(),
            fallback: vec![],
            input_price_per_million: None,
            output_price_per_million: None,
            cached_input_price_per_million: None,
        };
        let routing = RoutingConfig::default();
        assert!(!fallback_allowed(
            &Method::POST,
            &provider,
            &routing,
            &HeaderMap::new()
        ));
        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", "test".parse().unwrap());
        assert!(fallback_allowed(
            &Method::POST,
            &provider,
            &routing,
            &headers
        ));
    }

    #[test]
    fn cache_respects_total_bytes_limit() {
        let cache = std::sync::Mutex::new(ResponseCache::default());
        let response = |body: &'static [u8]| CachedResponse {
            expires_at: std::time::Instant::now() + std::time::Duration::from_secs(60),
            status: http::StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from_static(body),
            body_hash: super::request_hash(body),
        };
        cache_put(&cache, "one".into(), response(b"123456"), 10, 10);
        cache_put(&cache, "two".into(), response(b"abcdef"), 10, 10);
        assert!(cache_get(&cache, "one").is_none());
        assert!(cache_get(&cache, "two").is_some());
    }
}
