use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::transform::TransformPolicy;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_gateway_listen")]
    pub gateway_listen: String,
    #[serde(default = "default_admin_listen")]
    pub admin_listen: String,
    #[serde(default)]
    pub providers: Vec<ProviderProfile>,
    #[serde(default)]
    pub default_provider: Option<String>,
    #[serde(default)]
    pub budget: BudgetConfig,
    #[serde(default)]
    pub transform: TransformPolicy,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub privacy: PrivacyConfig,
    #[serde(default = "default_max_request_bytes")]
    pub max_request_bytes: usize,
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,
    /// Environment variable containing the bearer token required when the
    /// Gateway is configured for a non-loopback address.  The token itself is
    /// never written to the config file.
    #[serde(default)]
    pub gateway_auth_token_env: Option<String>,
    /// Maximum time between streamed upstream chunks.  This is separate from
    /// the total reqwest timeout so a provider cannot hold a slot forever by
    /// sending a byte every few minutes.
    #[serde(default = "default_stream_idle_timeout_seconds")]
    pub stream_idle_timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub id: String,
    pub endpoint: String,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub model_map: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub fallback: Vec<String>,
    #[serde(default)]
    pub input_price_per_million: Option<f64>,
    #[serde(default)]
    pub output_price_per_million: Option<f64>,
    #[serde(default)]
    pub cached_input_price_per_million: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    #[serde(default)]
    pub request_tokens: Option<i64>,
    #[serde(default)]
    pub session_tokens: Option<i64>,
    #[serde(default)]
    pub daily_tokens: Option<i64>,
    /// Optional explicit provider output cap. Never applied unless configured.
    #[serde(default)]
    pub request_output_tokens: Option<i64>,
    #[serde(default)]
    pub request_usd: Option<f64>,
    #[serde(default)]
    pub session_usd: Option<f64>,
    #[serde(default)]
    pub daily_usd: Option<f64>,
    #[serde(default = "default_max_same_fingerprint")]
    pub max_same_fingerprint: u32,
    /// Optional local gateway rate limit. Zero/unset means unlimited.
    #[serde(default)]
    pub requests_per_minute: Option<u32>,
    /// Maximum in-flight upstream requests. Zero/unset means unlimited.
    #[serde(default)]
    pub max_concurrency: Option<usize>,
    /// Optional budgets addressed by request metadata. Keys use the explicit
    /// forms `project:<id>`, `agent:<id>`, `session:<id>` and `model:<id>`.
    /// They are never inferred from prompt content.
    #[serde(default)]
    pub scopes: HashMap<String, BudgetScope>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BudgetScope {
    #[serde(default)]
    pub request_tokens: Option<i64>,
    #[serde(default)]
    pub session_tokens: Option<i64>,
    #[serde(default)]
    pub daily_tokens: Option<i64>,
    #[serde(default)]
    pub request_output_tokens: Option<i64>,
    #[serde(default)]
    pub request_usd: Option<f64>,
    #[serde(default)]
    pub session_usd: Option<f64>,
    #[serde(default)]
    pub daily_usd: Option<f64>,
    #[serde(default)]
    pub max_same_fingerprint: Option<u32>,
    #[serde(default)]
    pub requests_per_minute: Option<u32>,
    #[serde(default)]
    pub max_concurrency: Option<usize>,
}

impl BudgetScope {
    fn overlay(&self, target: &mut BudgetConfig) {
        if self.request_tokens.is_some() {
            target.request_tokens = self.request_tokens;
        }
        if self.session_tokens.is_some() {
            target.session_tokens = self.session_tokens;
        }
        if self.daily_tokens.is_some() {
            target.daily_tokens = self.daily_tokens;
        }
        if self.request_output_tokens.is_some() {
            target.request_output_tokens = self.request_output_tokens;
        }
        if self.request_usd.is_some() {
            target.request_usd = self.request_usd;
        }
        if self.session_usd.is_some() {
            target.session_usd = self.session_usd;
        }
        if self.daily_usd.is_some() {
            target.daily_usd = self.daily_usd;
        }
        if self.max_same_fingerprint.is_some() {
            target.max_same_fingerprint = self
                .max_same_fingerprint
                .unwrap_or(target.max_same_fingerprint);
        }
        if self.requests_per_minute.is_some() {
            target.requests_per_minute = self.requests_per_minute;
        }
        if self.max_concurrency.is_some() {
            target.max_concurrency = self.max_concurrency;
        }
    }
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            request_tokens: None,
            session_tokens: None,
            daily_tokens: None,
            request_output_tokens: None,
            request_usd: None,
            session_usd: None,
            daily_usd: None,
            max_same_fingerprint: default_max_same_fingerprint(),
            requests_per_minute: None,
            max_concurrency: None,
            scopes: HashMap::new(),
        }
    }
}

impl BudgetConfig {
    /// Apply the most specific explicit scope in order. A scope never changes
    /// the global config object and is therefore safe to reuse per request.
    pub fn scoped(&self, keys: &[String]) -> Self {
        let mut result = self.clone();
        result.scopes.clear();
        for key in keys {
            if let Some(scope) = self.scopes.get(key) {
                scope.overlay(&mut result);
            }
        }
        result
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Exact request/response cache. Disabled by default because agent
    /// requests can be stateful; enable it for deterministic read-only work.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_cache_ttl_seconds")]
    pub ttl_seconds: u64,
    #[serde(default = "default_cache_max_entries")]
    pub max_entries: usize,
    #[serde(default = "default_cache_max_entry_bytes")]
    pub max_entry_bytes: usize,
    #[serde(default = "default_cache_max_total_bytes")]
    pub max_total_bytes: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrivacyConfig {
    /// Strict mode disables response caching and keeps only the minimum
    /// receipt metadata. Prompt/code/response bodies are never persisted by
    /// the Gateway in either mode.
    #[serde(default)]
    pub strict: bool,
}

fn default_cache_ttl_seconds() -> u64 {
    300
}
fn default_cache_max_entries() -> usize {
    256
}
fn default_cache_max_entry_bytes() -> usize {
    2 * 1024 * 1024
}
fn default_cache_max_total_bytes() -> usize {
    64 * 1024 * 1024
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ttl_seconds: default_cache_ttl_seconds(),
            max_entries: default_cache_max_entries(),
            max_entry_bytes: default_cache_max_entry_bytes(),
            max_total_bytes: default_cache_max_total_bytes(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// priority preserves the configured order; cost chooses the lowest
    /// explicitly configured input price. No implicit model downgrade occurs.
    #[serde(default = "default_routing_mode")]
    pub mode: String,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: usize,
    /// Optional explicit pool for cost routing. Empty preserves the configured
    /// provider + fallback chain for backwards compatibility.
    #[serde(default)]
    pub pool: Vec<String>,
    /// Unknown protocols may opt into retrying POST/other non-idempotent
    /// requests. Known model protocols remain safe by default because their
    /// request contract is a generation call rather than a user-side effect.
    #[serde(default)]
    pub allow_non_idempotent_fallback: bool,
    /// Number of transient failures before a provider is temporarily removed
    /// from the candidate set. This protects long-running agents from retry
    /// storms while keeping the decision visible in the ledger.
    #[serde(default = "default_circuit_breaker_threshold")]
    pub circuit_breaker_threshold: u32,
    #[serde(default = "default_circuit_breaker_cooldown_seconds")]
    pub circuit_breaker_cooldown_seconds: u64,
}

fn default_routing_mode() -> String {
    "priority".into()
}
fn default_max_attempts() -> usize {
    3
}
fn default_circuit_breaker_threshold() -> u32 {
    3
}
fn default_circuit_breaker_cooldown_seconds() -> u64 {
    30
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            mode: default_routing_mode(),
            max_attempts: default_max_attempts(),
            pool: vec![],
            allow_non_idempotent_fallback: false,
            circuit_breaker_threshold: default_circuit_breaker_threshold(),
            circuit_breaker_cooldown_seconds: default_circuit_breaker_cooldown_seconds(),
        }
    }
}

fn default_gateway_listen() -> String {
    "127.0.0.1:8765".into()
}
fn default_admin_listen() -> String {
    "127.0.0.1:8766".into()
}
fn default_protocol() -> String {
    "openai-responses".into()
}
fn default_max_same_fingerprint() -> u32 {
    3
}
fn default_max_request_bytes() -> usize {
    32 * 1024 * 1024
}
fn default_max_response_bytes() -> usize {
    64 * 1024 * 1024
}
fn default_stream_idle_timeout_seconds() -> u64 {
    120
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            gateway_listen: default_gateway_listen(),
            admin_listen: default_admin_listen(),
            providers: vec![],
            default_provider: None,
            budget: BudgetConfig::default(),
            transform: TransformPolicy::default(),
            cache: CacheConfig::default(),
            routing: RoutingConfig::default(),
            privacy: PrivacyConfig::default(),
            max_request_bytes: default_max_request_bytes(),
            max_response_bytes: default_max_response_bytes(),
            gateway_auth_token_env: None,
            stream_idle_timeout_seconds: default_stream_idle_timeout_seconds(),
        }
    }
}

impl AppConfig {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("duola-agentcost")
            .join("config.toml")
    }

    pub fn data_dir() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("duola-agentcost")
    }

    /// Keep custom configuration profiles completely isolated from the
    /// default profile.  The default path retains the historical data
    /// location for backwards compatibility; `/tmp/team.toml` uses
    /// `/tmp/team.data`.
    pub fn data_dir_for_config(path: &Path) -> PathBuf {
        if path == Self::path() {
            Self::data_dir()
        } else {
            path.with_extension("data")
        }
    }

    pub fn ensure_data_dir(path: &Path) -> Result<()> {
        fs::create_dir_all(path)?;
        restrict_dir(path)?;
        Ok(())
    }

    pub fn load(path: Option<&Path>) -> Result<Self> {
        let path = path.map(PathBuf::from).unwrap_or_else(Self::path);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("读取配置失败: {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("解析配置失败: {}", path.display()))
    }

    pub fn save(&self, path: Option<&Path>) -> Result<PathBuf> {
        let path = path.map(PathBuf::from).unwrap_or_else(Self::path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
            restrict_dir(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        let temp = path.with_extension("toml.tmp");
        fs::write(&temp, text)?;
        fs::rename(&temp, &path)?;
        restrict_file(&path)?;
        Ok(path)
    }

    pub fn provider(&self, id: Option<&str>) -> Result<ProviderProfile> {
        let wanted = id
            .or(self.default_provider.as_deref())
            .or_else(|| self.providers.first().map(|p| p.id.as_str()))
            .context("没有配置 Provider，请先执行 provider add")?;
        self.providers
            .iter()
            .find(|p| p.id == wanted)
            .cloned()
            .with_context(|| format!("找不到 Provider: {wanted}"))
    }
}

fn file_hash(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(unix)]
fn restrict_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_dir(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<()> {
    Ok(())
}

pub struct CodexConfigGuard {
    path: PathBuf,
    snapshot: PathBuf,
    managed_hash: PathBuf,
}

impl CodexConfigGuard {
    pub fn install() -> Result<Self> {
        Self::install_with_gateway("http://127.0.0.1:8765/v1")
    }

    pub fn install_if_present(gateway: &str) -> Result<Option<Self>> {
        Self::install_if_present_in_data_dir(gateway, &AppConfig::data_dir())
    }

    pub fn install_if_present_in_data_dir(gateway: &str, data_dir: &Path) -> Result<Option<Self>> {
        let path = dirs::home_dir()
            .context("无法确定用户 Home 目录")?
            .join(".codex")
            .join("config.toml");
        if !path.exists() {
            return Ok(None);
        }
        Self::install_with_gateway_in_data_dir(gateway, data_dir).map(Some)
    }

    pub fn install_with_gateway(gateway: &str) -> Result<Self> {
        Self::install_with_gateway_in_data_dir(gateway, &AppConfig::data_dir())
    }

    pub fn install_with_gateway_in_data_dir(gateway: &str, data_dir: &Path) -> Result<Self> {
        let path = dirs::home_dir()
            .context("无法确定用户 Home 目录")?
            .join(".codex")
            .join("config.toml");
        if !path.exists() {
            anyhow::bail!("未找到 Codex 配置：{}", path.display());
        }
        let snapshot_dir = data_dir.join("config-snapshots");
        AppConfig::ensure_data_dir(&snapshot_dir)?;
        let snapshot = snapshot_dir.join("codex.config.toml");
        let managed_hash = snapshot_dir.join("codex.managed.sha256");
        if managed_hash.exists() {
            Self::restore_existing(&path, &snapshot, &managed_hash)?;
        }
        fs::copy(&path, &snapshot)?;
        let original = fs::read_to_string(&path)?;
        let mut document = original
            .parse::<DocumentMut>()
            .context("Codex config.toml 不是有效 TOML")?;
        document["model_provider"] = value("duola-agentcost");
        let providers = document
            .as_table_mut()
            .entry("model_providers")
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .context("Codex model_providers 不是 TOML table")?;
        let duola = providers
            .entry("duola-agentcost")
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .context("Codex DuoLA Provider 不是 TOML table")?;
        duola["name"] = value("DuoLA AgentCost");
        duola["base_url"] = value(gateway);
        duola["wire_api"] = value("responses");
        let temp = path.with_extension("toml.duola.tmp");
        fs::write(&temp, document.to_string())?;
        fs::rename(&temp, &path)?;
        fs::write(&managed_hash, file_hash(&path)?)?;
        restrict_file(&snapshot)?;
        restrict_file(&managed_hash)?;
        Ok(Self {
            path,
            snapshot,
            managed_hash,
        })
    }

    pub fn restore(&self) -> Result<()> {
        Self::restore_existing(&self.path, &self.snapshot, &self.managed_hash)
    }

    fn restore_existing(path: &Path, snapshot: &Path, managed_hash: &Path) -> Result<()> {
        if !snapshot.exists() {
            anyhow::bail!("Codex 快照不存在：{}", snapshot.display());
        }
        let expected = fs::read_to_string(managed_hash)?;
        let current = file_hash(path)?;
        if expected.trim() != current {
            anyhow::bail!(
                "Codex 配置在 DuoLA 接管期间被用户修改，未自动覆盖：{}",
                path.display()
            );
        }
        let temp = path.with_extension("toml.duola.restore.tmp");
        fs::copy(snapshot, &temp)?;
        fs::rename(&temp, path)?;
        let _ = fs::remove_file(managed_hash);
        Ok(())
    }

    pub fn restore_if_present() -> Result<bool> {
        Self::restore_if_present_in_data_dir(&AppConfig::data_dir())
    }

    pub fn restore_if_present_in_data_dir(data_dir: &Path) -> Result<bool> {
        let data_dir = data_dir.join("config-snapshots");
        let snapshot = data_dir.join("codex.config.toml");
        let managed_hash = data_dir.join("codex.managed.sha256");
        if !managed_hash.exists() {
            return Ok(false);
        }
        let path = dirs::home_dir()
            .context("无法确定用户 Home 目录")?
            .join(".codex")
            .join("config.toml");
        Self::restore_existing(&path, &snapshot, &managed_hash)?;
        Ok(true)
    }
}
