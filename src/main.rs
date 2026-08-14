use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use duola_agentcost::{
    config::{AppConfig, BudgetScope, CodexConfigGuard, ProviderProfile},
    gateway::{AppState, run},
    ledger::Ledger,
};
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};
use tokio::time::sleep;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "duola-agentcost",
    version,
    about = "DuoLA AgentCost local Agent Gateway"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Serve(ServeArgs),
    Install(InstallArgs),
    Setup(SetupArgs),
    Launch(LaunchArgs),
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    Budget {
        #[command(subcommand)]
        command: BudgetCommand,
    },
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
    Routing {
        #[command(subcommand)]
        command: RoutingCommand,
    },
    Privacy {
        #[command(subcommand)]
        command: PrivacyCommand,
    },
    Data {
        #[command(subcommand)]
        command: DataCommand,
    },
    Status {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Stats {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Dashboard {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Doctor {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Bypass {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Restore {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Export {
        output: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
        /// json/csv exports脱敏账本；sqlite exports the local SQLite file.
        #[arg(long, default_value = "json")]
        format: String,
    },
    Uninstall {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Pause {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Resume {
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Args, Debug)]
struct ServeArgs {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    gateway_listen: Option<String>,
    #[arg(long)]
    admin_listen: Option<String>,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    upstream_url: Option<String>,
    #[arg(long)]
    protocol: Option<String>,
    #[arg(long)]
    api_key_env: Option<String>,
}

#[derive(Args, Debug)]
struct InstallArgs {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    provider_id: Option<String>,
    #[arg(long)]
    endpoint: Option<String>,
    #[arg(long, default_value = "openai-responses")]
    protocol: String,
    #[arg(long)]
    api_key_env: Option<String>,
}

#[derive(Args, Debug)]
struct SetupArgs {
    /// Use a custom configuration profile instead of the default user profile.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Agent command to prepare. If omitted, AgentCost detects codex/claude/
    /// cursor/opencode in PATH.
    #[arg(long)]
    agent: Option<String>,
    /// Optional upstream endpoint. Without it, a safe default is selected
    /// from the detected Agent protocol.
    #[arg(long)]
    endpoint: Option<String>,
    #[arg(long)]
    provider_id: Option<String>,
    #[arg(long)]
    protocol: Option<String>,
    /// Environment variable containing a Provider key. The key itself is never
    /// written to the config file.
    #[arg(long)]
    api_key_env: Option<String>,
    /// Never prompt or require an Agent to be installed. Useful for packaging.
    #[arg(long)]
    non_interactive: bool,
}

#[derive(Args, Debug)]
struct LaunchArgs {
    #[arg(long)]
    config: Option<PathBuf>,
    agent: String,
    /// Open the local Dashboard after the Gateway is ready.
    #[arg(long)]
    open_dashboard: bool,
    #[arg(last = true)]
    args: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum ProviderCommand {
    Add {
        #[arg(long)]
        config: Option<PathBuf>,
        id: String,
        endpoint: String,
        #[arg(long, default_value = "openai-responses")]
        protocol: String,
        #[arg(long)]
        api_key_env: Option<String>,
        #[arg(long, value_delimiter = ',')]
        fallback: Vec<String>,
        #[arg(long)]
        input_price: Option<f64>,
        #[arg(long)]
        output_price: Option<f64>,
        #[arg(long)]
        cached_input_price: Option<f64>,
        /// Explicit incoming-model=upstream-model mappings. Repeat the flag.
        #[arg(long = "model-map", value_name = "INCOMING=UPSTREAM")]
        model_map: Vec<String>,
    },
    List {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Remove {
        #[arg(long)]
        config: Option<PathBuf>,
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum BudgetCommand {
    Set {
        #[arg(long)]
        config: Option<PathBuf>,
        /// Apply to an explicit scope such as project:backend, agent:codex,
        /// session:task-123 or model:gpt-4o. Omit to change global defaults.
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        admin_listen: Option<String>,
        /// Maximum input tokens allowed for one request.
        #[arg(long)]
        request_tokens: Option<i64>,
        /// Maximum input tokens allowed during the current Gateway session.
        #[arg(long)]
        session_tokens: Option<i64>,
        /// Maximum input tokens allowed since local midnight.
        #[arg(long)]
        daily_tokens: Option<i64>,
        /// Explicit maximum output tokens inserted only when configured.
        #[arg(long)]
        request_output_tokens: Option<i64>,
        #[arg(long)]
        request_usd: Option<f64>,
        #[arg(long)]
        session_usd: Option<f64>,
        #[arg(long)]
        daily_usd: Option<f64>,
        #[arg(long)]
        max_same_fingerprint: Option<u32>,
        #[arg(long)]
        requests_per_minute: Option<u32>,
        #[arg(long)]
        max_concurrency: Option<usize>,
    },
    Show {
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum CacheCommand {
    Set {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long)]
        ttl_seconds: Option<u64>,
        #[arg(long)]
        max_entries: Option<usize>,
        #[arg(long)]
        max_entry_bytes: Option<usize>,
        #[arg(long)]
        max_total_bytes: Option<usize>,
        #[arg(long)]
        admin_listen: Option<String>,
    },
    Clear {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        admin_listen: Option<String>,
    },
    Show {
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum RoutingCommand {
    Set {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        mode: Option<String>,
        #[arg(long)]
        max_attempts: Option<usize>,
        #[arg(long, value_delimiter = ',')]
        pool: Option<Vec<String>>,
        /// Allow fallback retries for unknown-protocol POST/non-idempotent requests.
        #[arg(long)]
        allow_non_idempotent_fallback: Option<bool>,
        #[arg(long)]
        circuit_breaker_threshold: Option<u32>,
        #[arg(long)]
        circuit_breaker_cooldown_seconds: Option<u64>,
        #[arg(long)]
        admin_listen: Option<String>,
    },
    Show {
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum PrivacyCommand {
    Set {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        strict: bool,
        #[arg(long)]
        relaxed: bool,
        #[arg(long)]
        admin_listen: Option<String>,
    },
    Show {
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum DataCommand {
    Purge {
        #[arg(long)]
        config: Option<PathBuf>,
        /// Delete local ledger/receipt records older than this many days.
        #[arg(long)]
        older_than_days: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();
    let cli = Cli::parse();
    match cli.command {
        Commands::Serve(args) => serve(args).await,
        Commands::Install(args) => install(args),
        Commands::Setup(args) => setup(args),
        Commands::Launch(args) => launch(args).await,
        Commands::Provider { command } => provider(command),
        Commands::Budget { command } => budget(command).await,
        Commands::Cache { command } => cache(command).await,
        Commands::Routing { command } => routing(command).await,
        Commands::Privacy { command } => privacy(command).await,
        Commands::Data { command } => data(command),
        Commands::Status { config } => remote_config_json("/api/status", config.as_deref()).await,
        Commands::Stats { config } => remote_config_json("/api/stats", config.as_deref()).await,
        Commands::Dashboard { config: config_arg } => {
            let config = AppConfig::load(config_arg.as_deref())?;
            println!("DuoLA AgentCost Dashboard: http://{}/", config.admin_listen);
            Ok(())
        }
        Commands::Doctor { config } => doctor(config.as_deref()),
        Commands::Bypass { config } => set_bypass(true, config.as_deref()),
        Commands::Restore { config } => {
            let data_dir = config
                .as_deref()
                .map(AppConfig::data_dir_for_config)
                .unwrap_or_else(AppConfig::data_dir);
            if CodexConfigGuard::restore_if_present_in_data_dir(&data_dir)? {
                println!("Codex 原始配置已恢复。");
            }
            set_bypass(false, config.as_deref())
        }
        Commands::Export {
            output,
            config,
            format,
        } => export(output, config.as_deref(), &format),
        Commands::Uninstall { config } => {
            let data_dir = config
                .as_deref()
                .map(AppConfig::data_dir_for_config)
                .unwrap_or_else(AppConfig::data_dir);
            if let Err(error) = CodexConfigGuard::restore_if_present_in_data_dir(&data_dir) {
                eprintln!("Codex 配置未自动恢复：{error}");
            }
            set_bypass(true, config.as_deref())?;
            println!(
                "已停止 AgentCost 接管。原有 Agent 配置未删除；如需重新启用，执行 duola-agentcost restore。"
            );
            Ok(())
        }
        Commands::Pause { config } => set_bypass(true, config.as_deref()),
        Commands::Resume { config } => set_bypass(false, config.as_deref()),
    }
}

async fn serve(args: ServeArgs) -> Result<()> {
    let config_path = args.config.clone().unwrap_or_else(AppConfig::path);
    let mut config = AppConfig::load(args.config.as_deref())?;
    if let Some(listen) = args.gateway_listen {
        config.gateway_listen = listen;
    }
    if let Some(listen) = args.admin_listen {
        config.admin_listen = listen;
    }
    if let Some(url) = args.upstream_url {
        let id = args.provider.unwrap_or_else(|| "default".into());
        config.providers.retain(|p| p.id != id);
        config.providers.push(ProviderProfile {
            id: id.clone(),
            endpoint: url,
            protocol: args.protocol.unwrap_or_else(|| "openai-responses".into()),
            api_key_env: args.api_key_env,
            model_map: Default::default(),
            fallback: vec![],
            input_price_per_million: None,
            output_price_per_million: None,
            cached_input_price_per_million: None,
        });
        config.default_provider = Some(id);
    }
    let data_dir = AppConfig::data_dir_for_config(&config_path);
    AppConfig::ensure_data_dir(&data_dir)?;
    let ledger = Ledger::open(&data_dir.join("ledger.sqlite"))?;
    let state = AppState::new_with_config_path(config, ledger, config_path)?;
    run(std::sync::Arc::new(state)).await
}

#[derive(Debug, Clone, Copy)]
struct AgentPreset {
    provider_id: &'static str,
    endpoint: &'static str,
    protocol: &'static str,
    key_env: &'static str,
}

fn agent_preset(agent: &str) -> Option<AgentPreset> {
    match agent.trim().to_ascii_lowercase().as_str() {
        "codex" => Some(AgentPreset {
            provider_id: "openai",
            endpoint: "https://api.openai.com",
            protocol: "openai-responses",
            key_env: "OPENAI_API_KEY",
        }),
        "claude" | "claude-code" => Some(AgentPreset {
            provider_id: "anthropic",
            endpoint: "https://api.anthropic.com",
            protocol: "anthropic-messages",
            key_env: "ANTHROPIC_API_KEY",
        }),
        "cursor" => Some(AgentPreset {
            provider_id: "openai",
            endpoint: "https://api.openai.com",
            protocol: "openai-responses",
            key_env: "OPENAI_API_KEY",
        }),
        "opencode" => Some(AgentPreset {
            provider_id: "openai",
            endpoint: "https://api.openai.com",
            protocol: "openai-responses",
            key_env: "OPENAI_API_KEY",
        }),
        _ => None,
    }
}

fn command_available(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn detect_agent() -> Option<String> {
    ["codex", "claude", "cursor", "opencode"]
        .into_iter()
        .find(|command| command_available(command))
        .map(str::to_owned)
}

/// Populate the minimum Provider profile needed to start a supported Agent.
/// This deliberately does not invent a key: existing Agent authentication is
/// forwarded, and a key is only read when the user explicitly names an env var.
fn ensure_provider_for_agent(
    config: &mut AppConfig,
    agent: &str,
    endpoint: Option<&str>,
    provider_id: Option<&str>,
    protocol: Option<&str>,
    api_key_env: Option<&str>,
) -> Result<bool> {
    if !config.providers.is_empty() {
        if config.default_provider.is_none() {
            config.default_provider = config.providers.first().map(|provider| provider.id.clone());
        }
        return Ok(false);
    }

    let preset = agent_preset(agent).with_context(|| {
        format!(
            "无法从 Agent `{agent}` 推断协议。请使用 --endpoint、--protocol 和 --provider-id 明确配置 Provider"
        )
    })?;
    let key_env = api_key_env.map(str::to_owned).or_else(|| {
        std::env::var(preset.key_env)
            .ok()
            .map(|_| preset.key_env.to_owned())
    });
    let profile = ProviderProfile {
        id: provider_id.unwrap_or(preset.provider_id).to_owned(),
        endpoint: endpoint.unwrap_or(preset.endpoint).to_owned(),
        protocol: protocol.unwrap_or(preset.protocol).to_owned(),
        api_key_env: key_env,
        model_map: Default::default(),
        fallback: vec![],
        input_price_per_million: None,
        output_price_per_million: None,
        cached_input_price_per_million: None,
    };
    profile
        .endpoint
        .parse::<reqwest::Url>()
        .with_context(|| format!("Provider endpoint 不是有效 URL：{}", profile.endpoint))?;
    config.providers.push(profile.clone());
    config.default_provider = Some(profile.id);
    Ok(true)
}

fn setup(args: SetupArgs) -> Result<()> {
    let config_path = args.config.clone().unwrap_or_else(AppConfig::path);
    let mut config = AppConfig::load(Some(&config_path))?;
    let agent = args.agent.clone().or_else(detect_agent);

    if config.providers.is_empty() {
        let Some(agent) = agent.as_deref() else {
            if args.non_interactive {
                let path = config.save(Some(&config_path))?;
                AppConfig::ensure_data_dir(&AppConfig::data_dir_for_config(&config_path))?;
                println!("已创建本机配置：{}", path.display());
                println!(
                    "尚未检测到 Codex、Claude Code、Cursor 或 OpenCode；安装 Agent 后执行 duola-agentcost setup --agent codex。\n"
                );
                return Ok(());
            };
            anyhow::bail!(
                "未检测到支持的 Agent。请先安装 Codex/Claude Code/Cursor/OpenCode，或执行 setup --agent <name>"
            );
        };
        ensure_provider_for_agent(
            &mut config,
            agent,
            args.endpoint.as_deref(),
            args.provider_id.as_deref(),
            args.protocol.as_deref(),
            args.api_key_env.as_deref(),
        )?;
        let path = config.save(Some(&config_path))?;
        AppConfig::ensure_data_dir(&AppConfig::data_dir_for_config(&config_path))?;
        let provider = config.provider(None)?;
        println!("首次设置完成（本机）");
        println!(
            "Agent：{}{}",
            agent,
            if command_available(agent) {
                "（已检测）"
            } else {
                "（由参数指定）"
            }
        );
        println!(
            "Provider：{} · {} · {}",
            provider.id, provider.protocol, provider.endpoint
        );
        println!("配置：{}", path.display());
        println!("Dashboard：http://{}/", config.admin_listen);
        println!("下一步：duola-agentcost launch {} --open-dashboard", agent);
        match provider.api_key_env.as_deref() {
            Some(env) => println!("认证：使用本机环境变量 {env}；Key 不写入配置。\n"),
            None => println!(
                "认证：不新增 API Key，优先复用 Agent 自己的登录态；独立 API 才需要 --api-key-env。\n"
            ),
        }
    } else {
        let path = config.save(Some(&config_path))?;
        AppConfig::ensure_data_dir(&AppConfig::data_dir_for_config(&config_path))?;
        let provider = config.provider(None)?;
        println!("AgentCost 已完成本机设置");
        println!(
            "Provider：{} · {} · {}",
            provider.id, provider.protocol, provider.endpoint
        );
        println!(
            "下一步：duola-agentcost launch {} --open-dashboard",
            agent.as_deref().unwrap_or("codex")
        );
        println!("配置：{}", path.display());
    }
    Ok(())
}

fn install(args: InstallArgs) -> Result<()> {
    let config_path = args.config.clone().unwrap_or_else(AppConfig::path);
    let mut config = AppConfig::load(Some(&config_path))?;
    if let Some(endpoint) = args.endpoint {
        let id = args.provider_id.unwrap_or_else(|| "default".into());
        config.providers.retain(|p| p.id != id);
        config.providers.push(ProviderProfile {
            id: id.clone(),
            endpoint,
            protocol: args.protocol,
            api_key_env: args.api_key_env,
            model_map: Default::default(),
            fallback: vec![],
            input_price_per_million: None,
            output_price_per_million: None,
            cached_input_price_per_million: None,
        });
        config.default_provider = Some(id);
    }
    if config.providers.is_empty()
        && let Some(agent) = detect_agent()
    {
        let _ = ensure_provider_for_agent(&mut config, &agent, None, None, None, None)?;
        println!("已检测到 Agent `{agent}`，已自动准备默认 Provider。");
    }
    let path = config.save(Some(&config_path))?;
    AppConfig::ensure_data_dir(&AppConfig::data_dir_for_config(&config_path))?;
    println!("安装完成。配置：{}", path.display());
    println!("Gateway：{}", config.gateway_listen);
    println!("Dashboard：http://{}", config.admin_listen);
    if config.providers.is_empty() {
        println!(
            "尚未检测到 Agent/Provider：执行 duola-agentcost setup --agent codex，或用 serve --upstream-url 临时启动。"
        );
    } else {
        println!(
            "Provider：{}",
            config.default_provider.as_deref().unwrap_or("未选择")
        );
    }
    Ok(())
}

async fn launch(args: LaunchArgs) -> Result<()> {
    let config_path = args.config.clone().unwrap_or_else(AppConfig::path);
    let mut config = AppConfig::load(Some(&config_path))?;
    if config.providers.is_empty() {
        ensure_provider_for_agent(&mut config, &args.agent, None, None, None, None)?;
        config.save(Some(&config_path))?;
        AppConfig::ensure_data_dir(&AppConfig::data_dir_for_config(&config_path))?;
        eprintln!(
            "已自动完成首次 Provider 设置：{}。如需自有 API Key，请配置环境变量后重新运行。",
            config.default_provider.as_deref().unwrap_or("default")
        );
    }
    config.provider(None)?;
    let admin_url = format!("http://{}/healthz", config.admin_listen);
    let gateway_base = format!("http://{}", config.gateway_listen);
    let data_dir = AppConfig::data_dir_for_config(&config_path);
    let codex_guard = if matches!(args.agent.to_ascii_lowercase().as_str(), "codex") {
        CodexConfigGuard::install_if_present_in_data_dir(&format!("{gateway_base}/v1"), &data_dir)?
    } else {
        None
    };
    let exe = std::env::current_exe()?;
    let client = reqwest::Client::new();
    let mut server = spawn_gateway(&exe, &config_path)?;
    if !wait_gateway_ready(&client, &admin_url).await {
        let _ = server.kill();
        if let Some(guard) = codex_guard.as_ref() {
            guard.restore()?;
        }
        anyhow::bail!("Gateway 未能启动，请执行 doctor");
    }
    if args.open_dashboard {
        open_dashboard(&format!("http://{}/", config.admin_listen));
    }
    let mut command = Command::new(&args.agent);
    command.args(&args.args);
    command.env("DUOLA_AGENTCOST", "1");
    command.env("DUOLA_AGENTCOST_GATEWAY", &gateway_base);
    match args.agent.to_ascii_lowercase().as_str() {
        "claude" | "claude-code" => {
            command.env("ANTHROPIC_BASE_URL", &gateway_base);
        }
        "codex" | "opencode" | "cursor" => {
            command.env("OPENAI_BASE_URL", format!("{gateway_base}/v1"));
        }
        _ => {}
    }
    let mut agent = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = server.kill();
            if let Some(guard) = codex_guard.as_ref() {
                let _ = guard.restore();
            }
            return Err(error).context("启动 Agent 失败");
        }
    };
    let mut restarts = 0_u8;
    let status = loop {
        if let Some(status) = agent.try_wait()? {
            break status;
        }
        if server.try_wait()?.is_some() {
            restarts += 1;
            if restarts > 3 {
                let _ = agent.kill();
                let _ = agent.wait();
                if let Some(guard) = codex_guard.as_ref() {
                    let _ = guard.restore();
                }
                anyhow::bail!(
                    "Gateway 在 Agent 运行期间连续退出，已停止接管；原始 Agent 配置已恢复"
                );
            }
            warn_gateway_restart(restarts);
            server = match spawn_gateway(&exe, &config_path) {
                Ok(child) => child,
                Err(error) => {
                    let _ = agent.kill();
                    let _ = agent.wait();
                    if let Some(guard) = codex_guard.as_ref() {
                        let _ = guard.restore();
                    }
                    return Err(error);
                }
            };
            if !wait_gateway_ready(&client, &admin_url).await {
                let _ = agent.kill();
                let _ = agent.wait();
                if let Some(guard) = codex_guard.as_ref() {
                    let _ = guard.restore();
                }
                anyhow::bail!("Gateway 重启后未恢复，已停止接管；原始 Agent 配置已恢复");
            }
        }
        sleep(Duration::from_millis(250)).await;
    };
    let _ = server.kill();
    if let Some(guard) = codex_guard {
        guard.restore()?;
    }
    std::process::exit(status.code().unwrap_or(1));
}

fn open_dashboard(url: &str) {
    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg(url).status();
    #[cfg(target_os = "linux")]
    let result = Command::new("xdg-open").arg(url).status();
    #[cfg(target_os = "windows")]
    let result = Command::new("cmd").args(["/C", "start", "", url]).status();
    if result.is_err() {
        eprintln!("无法自动打开 Dashboard，请手动访问 {url}");
    }
}

fn spawn_gateway(
    exe: &std::path::Path,
    config_path: &std::path::Path,
) -> Result<std::process::Child> {
    Command::new(exe)
        .arg("serve")
        .arg("--config")
        .arg(config_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("启动本地 Gateway 失败")
}

async fn wait_gateway_ready(client: &reqwest::Client, admin_url: &str) -> bool {
    for _ in 0..30 {
        if client.get(admin_url).send().await.is_ok() {
            return true;
        }
        sleep(Duration::from_millis(100)).await;
    }
    false
}

fn warn_gateway_restart(attempt: u8) {
    eprintln!("DuoLA AgentCost Gateway 异常退出，正在自动恢复（第 {attempt}/3 次）…");
}

fn provider(command: ProviderCommand) -> Result<()> {
    let config_path = match &command {
        ProviderCommand::Add { config, .. }
        | ProviderCommand::List { config }
        | ProviderCommand::Remove { config, .. } => config.clone().unwrap_or_else(AppConfig::path),
    };
    let mut config = AppConfig::load(Some(&config_path))?;
    match command {
        ProviderCommand::Add {
            config: _,
            id,
            endpoint,
            protocol,
            api_key_env,
            fallback,
            input_price,
            output_price,
            cached_input_price,
            model_map,
        } => {
            let model_map = model_map
                .into_iter()
                .map(|entry| {
                    entry
                        .split_once('=')
                        .map(|(from, to)| (from.to_owned(), to.to_owned()))
                        .filter(|(from, to)| !from.is_empty() && !to.is_empty())
                        .ok_or_else(|| {
                            anyhow::anyhow!("model-map 必须使用 incoming=upstream 格式: {entry}")
                        })
                })
                .collect::<Result<std::collections::HashMap<_, _>>>()?;
            config.providers.retain(|p| p.id != id);
            config.providers.push(ProviderProfile {
                id: id.clone(),
                endpoint,
                protocol,
                api_key_env,
                model_map,
                fallback,
                input_price_per_million: input_price,
                output_price_per_million: output_price,
                cached_input_price_per_million: cached_input_price,
            });
            if config.default_provider.is_none() {
                config.default_provider = Some(id);
            }
            config.save(Some(&config_path))?;
            println!("Provider 已保存");
        }
        ProviderCommand::List { config: _ } => {
            if config.providers.is_empty() {
                println!("没有 Provider");
            }
            for p in config.providers {
                let mappings = if p.model_map.is_empty() {
                    "no model map".to_owned()
                } else {
                    format!("{} model map(s)", p.model_map.len())
                };
                println!("{}  {}  {}  {}", p.id, p.protocol, p.endpoint, mappings);
            }
        }
        ProviderCommand::Remove { config: _, id } => {
            config.providers.retain(|p| p.id != id);
            if config.default_provider.as_deref() == Some(id.as_str()) {
                config.default_provider = config.providers.first().map(|p| p.id.clone());
            }
            config.save(Some(&config_path))?;
            println!("Provider 已移除：{id}");
        }
    }
    Ok(())
}

async fn budget(command: BudgetCommand) -> Result<()> {
    match command {
        BudgetCommand::Set {
            config: config_arg,
            scope,
            admin_listen,
            request_tokens,
            session_tokens,
            daily_tokens,
            request_output_tokens,
            request_usd,
            session_usd,
            daily_usd,
            max_same_fingerprint,
            requests_per_minute,
            max_concurrency,
        } => {
            let config_path = config_arg.unwrap_or_else(AppConfig::path);
            let mut config = AppConfig::load(Some(&config_path))?;
            if let Some(scope_id) = scope {
                if scope_id.trim().is_empty() {
                    anyhow::bail!("--scope 不能为空");
                }
                let target: &mut BudgetScope = config.budget.scopes.entry(scope_id).or_default();
                if request_tokens.is_some() {
                    target.request_tokens = request_tokens;
                }
                if session_tokens.is_some() {
                    target.session_tokens = session_tokens;
                }
                if daily_tokens.is_some() {
                    target.daily_tokens = daily_tokens;
                }
                if request_output_tokens.is_some() {
                    target.request_output_tokens = request_output_tokens;
                }
                if request_usd.is_some() {
                    target.request_usd = request_usd;
                }
                if session_usd.is_some() {
                    target.session_usd = session_usd;
                }
                if daily_usd.is_some() {
                    target.daily_usd = daily_usd;
                }
                if max_same_fingerprint.is_some() {
                    target.max_same_fingerprint = max_same_fingerprint.map(|v| v.max(1));
                }
                if requests_per_minute.is_some() {
                    target.requests_per_minute = requests_per_minute;
                }
                if max_concurrency.is_some() {
                    target.max_concurrency = max_concurrency;
                }
            } else {
                if request_tokens.is_some() {
                    config.budget.request_tokens = request_tokens;
                }
                if session_tokens.is_some() {
                    config.budget.session_tokens = session_tokens;
                }
                if daily_tokens.is_some() {
                    config.budget.daily_tokens = daily_tokens;
                }
                if request_output_tokens.is_some() {
                    config.budget.request_output_tokens = request_output_tokens;
                }
                if request_usd.is_some() {
                    config.budget.request_usd = request_usd;
                }
                if session_usd.is_some() {
                    config.budget.session_usd = session_usd;
                }
                if daily_usd.is_some() {
                    config.budget.daily_usd = daily_usd;
                }
                if let Some(value) = max_same_fingerprint {
                    config.budget.max_same_fingerprint = value.max(1);
                }
                if requests_per_minute.is_some() {
                    config.budget.requests_per_minute = requests_per_minute;
                }
                if max_concurrency.is_some() {
                    config.budget.max_concurrency = max_concurrency;
                }
            }
            config.save(Some(&config_path))?;
            println!("预算和循环策略已保存：{}", config_path.display());
            let reload_url = format!(
                "http://{}/api/reload",
                admin_listen.as_deref().unwrap_or(&config.admin_listen)
            );
            match reqwest::Client::new().post(reload_url).send().await {
                Ok(response) if response.status().is_success() => {
                    println!("运行中的 Gateway 已重新加载预算。");
                }
                _ => println!("Gateway 当前未运行；下次 serve/launch 时生效。"),
            }
        }
        BudgetCommand::Show { config: config_arg } => {
            let config_path = config_arg.unwrap_or_else(AppConfig::path);
            let config = AppConfig::load(Some(&config_path))?;
            println!("{}", toml::to_string_pretty(&config.budget)?);
        }
    }
    Ok(())
}

async fn cache(command: CacheCommand) -> Result<()> {
    match command {
        CacheCommand::Set {
            config: config_arg,
            enabled,
            ttl_seconds,
            max_entries,
            max_entry_bytes,
            max_total_bytes,
            admin_listen,
        } => {
            let config_path = config_arg.unwrap_or_else(AppConfig::path);
            let mut config = AppConfig::load(Some(&config_path))?;
            if let Some(value) = enabled {
                config.cache.enabled = value;
            }
            if let Some(value) = ttl_seconds {
                config.cache.ttl_seconds = value;
            }
            if let Some(value) = max_entries {
                config.cache.max_entries = value;
            }
            if let Some(value) = max_entry_bytes {
                config.cache.max_entry_bytes = value;
            }
            if let Some(value) = max_total_bytes {
                config.cache.max_total_bytes = value;
            }
            config.save(Some(&config_path))?;
            let reload_url = format!(
                "http://{}/api/reload",
                admin_listen.as_deref().unwrap_or(&config.admin_listen)
            );
            match reqwest::Client::new().post(reload_url).send().await {
                Ok(response) if response.status().is_success() => {
                    println!("缓存策略已保存并重新加载。")
                }
                _ => println!("缓存策略已保存；Gateway 当前未运行，下次启动生效。"),
            }
        }
        CacheCommand::Show { config: config_arg } => {
            let config_path = config_arg.unwrap_or_else(AppConfig::path);
            let config = AppConfig::load(Some(&config_path))?;
            println!("{}", toml::to_string_pretty(&config.cache)?);
        }
        CacheCommand::Clear {
            config: config_arg,
            admin_listen,
        } => {
            let config_path = config_arg.unwrap_or_else(AppConfig::path);
            let config = AppConfig::load(Some(&config_path))?;
            let url = format!(
                "http://{}/api/cache/clear",
                admin_listen.as_deref().unwrap_or(&config.admin_listen)
            );
            let response = reqwest::Client::new().post(url).send().await?;
            if response.status().is_success() {
                println!("本地响应缓存已清空。")
            } else {
                anyhow::bail!("Gateway 当前未运行或清空失败：{}", response.status())
            }
        }
    }
    Ok(())
}

async fn routing(command: RoutingCommand) -> Result<()> {
    match command {
        RoutingCommand::Set {
            config: config_arg,
            mode,
            max_attempts,
            pool,
            allow_non_idempotent_fallback,
            circuit_breaker_threshold,
            circuit_breaker_cooldown_seconds,
            admin_listen,
        } => {
            let config_path = config_arg.unwrap_or_else(AppConfig::path);
            let mut config = AppConfig::load(Some(&config_path))?;
            if let Some(mode) = mode {
                if !matches!(mode.as_str(), "priority" | "cost") {
                    anyhow::bail!("routing mode 只能是 priority 或 cost");
                }
                config.routing.mode = mode;
            }
            if let Some(value) = max_attempts {
                config.routing.max_attempts = value.max(1);
            }
            if let Some(pool) = pool {
                config.routing.pool = pool;
            }
            if let Some(value) = allow_non_idempotent_fallback {
                config.routing.allow_non_idempotent_fallback = value;
            }
            if let Some(value) = circuit_breaker_threshold {
                config.routing.circuit_breaker_threshold = value;
            }
            if let Some(value) = circuit_breaker_cooldown_seconds {
                config.routing.circuit_breaker_cooldown_seconds = value.max(1);
            }
            config.save(Some(&config_path))?;
            println!(
                "路由策略已保存：{}，最多 {} 次尝试，成本池 {} 个 Provider。",
                config.routing.mode,
                config.routing.max_attempts,
                config.routing.pool.len()
            );
            let url = format!(
                "http://{}/api/reload",
                admin_listen.as_deref().unwrap_or(&config.admin_listen)
            );
            match reqwest::Client::new().post(url).send().await {
                Ok(response) if response.status().is_success() => {
                    println!("运行中的 Gateway 已重新加载路由策略。")
                }
                _ => println!("Gateway 当前未运行；下次启动时生效。"),
            }
        }
        RoutingCommand::Show { config: config_arg } => {
            let config_path = config_arg.unwrap_or_else(AppConfig::path);
            let config = AppConfig::load(Some(&config_path))?;
            println!("{}", toml::to_string_pretty(&config.routing)?);
        }
    }
    Ok(())
}

async fn privacy(command: PrivacyCommand) -> Result<()> {
    match command {
        PrivacyCommand::Set {
            config: config_arg,
            strict,
            relaxed,
            admin_listen,
        } => {
            if strict && relaxed {
                anyhow::bail!("--strict 与 --relaxed 不能同时使用");
            }
            let config_path = config_arg.unwrap_or_else(AppConfig::path);
            let mut config = AppConfig::load(Some(&config_path))?;
            if strict {
                config.privacy.strict = true;
            } else if relaxed {
                config.privacy.strict = false;
            } else {
                anyhow::bail!("请指定 --strict 或 --relaxed");
            }
            config.save(Some(&config_path))?;
            let url = format!(
                "http://{}/api/reload",
                admin_listen.as_deref().unwrap_or(&config.admin_listen)
            );
            match reqwest::Client::new().post(url).send().await {
                Ok(response) if response.status().is_success() => {
                    println!("隐私策略已保存并重新加载。")
                }
                _ => println!("隐私策略已保存；Gateway 当前未运行，下次启动生效。"),
            }
        }
        PrivacyCommand::Show { config: config_arg } => {
            let config_path = config_arg.unwrap_or_else(AppConfig::path);
            let config = AppConfig::load(Some(&config_path))?;
            println!("{}", toml::to_string_pretty(&config.privacy)?);
        }
    }
    Ok(())
}

async fn remote_json(url: &str) -> Result<()> {
    let response = reqwest::get(url)
        .await
        .context("Admin API 不可用，请先启动 serve")?;
    let text = response.text().await?;
    println!("{text}");
    Ok(())
}

async fn remote_config_json(path: &str, config_path: Option<&std::path::Path>) -> Result<()> {
    let config = AppConfig::load(config_path)?;
    remote_json(&format!("http://{}{}", config.admin_listen, path)).await
}

fn doctor(config_path: Option<&std::path::Path>) -> Result<()> {
    let config = AppConfig::load(config_path)?;
    let display_path = config_path
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(AppConfig::path);
    println!("配置：{}", display_path.display());
    println!("Gateway：{}", config.gateway_listen);
    println!("Admin：{}", config.admin_listen);
    println!("Provider 数量：{}", config.providers.len());
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    if config
        .gateway_listen
        .parse::<std::net::SocketAddr>()
        .is_err()
    {
        errors.push(format!("Gateway 监听地址无效：{}", config.gateway_listen));
    }
    if config.admin_listen.parse::<std::net::SocketAddr>().is_err() {
        errors.push(format!("Admin 监听地址无效：{}", config.admin_listen));
    }
    if let Err(error) = duola_agentcost::gateway::validate_config_addresses(&config) {
        errors.push(error.to_string());
    }
    if config.providers.is_empty() {
        errors.push("未配置 Provider".into());
    } else {
        for p in &config.providers {
            println!("✓ {} ({}) -> {}", p.id, p.protocol, p.endpoint);
            if p.endpoint.parse::<reqwest::Url>().is_err() {
                errors.push(format!("Provider {} 的 endpoint 不是有效 URL", p.id));
            }
            if let Some(env) = &p.api_key_env {
                println!("  Key 来源：环境变量 {env}");
                if std::env::var(env).is_err() {
                    warnings.push(format!(
                        "Provider {} 的 Key 环境变量 {} 当前未设置（也可能由客户端直接提供）",
                        p.id, env
                    ));
                }
            }
        }
    }
    let mut provider_ids = std::collections::HashSet::new();
    for provider in &config.providers {
        if !provider_ids.insert(&provider.id) {
            errors.push(format!("Provider ID 重复：{}", provider.id));
        }
    }
    for scope in config.budget.scopes.keys() {
        if !matches!(
            scope
                .split_once(':')
                .map(|(kind, value)| (kind, !value.is_empty())),
            Some(("project" | "agent" | "session" | "model", true))
        ) {
            errors.push(format!(
                "预算 scope 无效：{}（应为 project:<id>、agent:<id>、session:<id> 或 model:<id>）",
                scope
            ));
        }
    }
    let data_dir = config_path
        .map(AppConfig::data_dir_for_config)
        .unwrap_or_else(AppConfig::data_dir);
    println!("本地数据：{}", data_dir.display());
    for warning in warnings {
        println!("⚠ {warning}");
    }
    if !errors.is_empty() {
        for error in &errors {
            println!("✗ {error}");
        }
        anyhow::bail!("doctor 发现 {} 个配置问题", errors.len());
    }
    println!(
        "结果：配置可启动；未连接 Provider 发送真实请求。执行 doctor 只做本地检查，不会消耗模型额度。"
    );
    Ok(())
}

fn set_bypass(enabled: bool, config_path: Option<&std::path::Path>) -> Result<()> {
    let data_dir = config_path
        .map(AppConfig::data_dir_for_config)
        .unwrap_or_else(AppConfig::data_dir);
    let path = data_dir.join("bypass");
    if enabled {
        AppConfig::ensure_data_dir(&data_dir)?;
        std::fs::write(&path, b"enabled")?;
        println!("已启用 bypass：请求原样转发，不执行 AgentCost 规则和循环暂停。");
    } else {
        let _ = std::fs::remove_file(&path);
        println!("已恢复 AgentCost 接管。");
    }
    let ledger_path = data_dir.join("ledger.sqlite");
    if ledger_path.exists()
        && let Ok(ledger) = Ledger::open_maintenance(&ledger_path)
    {
        let _ = ledger.record_control_event(
            if enabled { "bypass" } else { "restore" },
            enabled,
            "CLI action",
        );
    }
    Ok(())
}

fn data(command: DataCommand) -> Result<()> {
    match command {
        DataCommand::Purge {
            config: config_arg,
            older_than_days,
        } => {
            if older_than_days == 0 {
                anyhow::bail!("older-than-days 必须大于 0，避免误删今天的账本");
            }
            let config_path = config_arg.unwrap_or_else(AppConfig::path);
            let data_dir = AppConfig::data_dir_for_config(&config_path);
            let source = data_dir.join("ledger.sqlite");
            if !source.exists() {
                println!("尚无账本，无需清理：{}", source.display());
                return Ok(());
            }
            let ledger = Ledger::open_maintenance(&source)?;
            let cutoff = chrono::Utc::now().timestamp()
                - (older_than_days as i64).saturating_mul(24 * 60 * 60);
            let deleted = ledger.purge_before(cutoff)?;
            println!("已清理 {} 条本地请求及其 receipt/attempt 记录。", deleted);
        }
    }
    Ok(())
}

fn export(output: PathBuf, config_path: Option<&std::path::Path>, format: &str) -> Result<()> {
    let data_dir = config_path
        .map(AppConfig::data_dir_for_config)
        .unwrap_or_else(AppConfig::data_dir);
    let source = data_dir.join("ledger.sqlite");
    if !source.exists() {
        anyhow::bail!("尚无账本：{}", source.display());
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match format {
        "sqlite" => {
            std::fs::copy(&source, &output)?;
            println!("SQLite 账本已导出：{}", output.display());
        }
        "json" => {
            let ledger = Ledger::open_read_only(&source)?;
            let records = ledger.recent(1_000_000)?;
            std::fs::write(&output, serde_json::to_vec_pretty(&records)?)?;
            println!(
                "脱敏 JSON 账本已导出（不含 Prompt/代码/完整响应）：{}",
                output.display()
            );
        }
        "csv" => {
            let ledger = Ledger::open_read_only(&source)?;
            let records = ledger.recent(1_000_000)?;
            let mut csv = String::from(
                "id,provider,path,status,input_bytes,sent_bytes,input_tokens,output_tokens,measured_input_tokens,cached_input_tokens,usage_estimated,saved_input_tokens,cost,latency_ms,created_at,session_id,project_id,agent,model,transform_status,transform_rule_count,original_hash,sent_hash,reason\n",
            );
            for record in records {
                let fields = [
                    record.id,
                    record.provider,
                    record.path,
                    record.status,
                    record.input_bytes.to_string(),
                    record.sent_bytes.to_string(),
                    record.input_tokens.to_string(),
                    record.output_tokens.to_string(),
                    record
                        .measured_input_tokens
                        .map(|value| value.to_string())
                        .unwrap_or_default(),
                    record.cached_input_tokens.to_string(),
                    record.usage_estimated.to_string(),
                    record.saved_input_tokens.to_string(),
                    format!("{:.8}", record.cost),
                    record.latency_ms.to_string(),
                    record.created_at.to_string(),
                    record.session_id,
                    record.project_id.unwrap_or_default(),
                    record.agent.unwrap_or_default(),
                    record.model.unwrap_or_default(),
                    record.transform_status,
                    record.transform_rule_count.to_string(),
                    record.original_hash.unwrap_or_default(),
                    record.sent_hash.unwrap_or_default(),
                    record.reason.unwrap_or_default(),
                ];
                csv.push_str(
                    &fields
                        .iter()
                        .map(|field| csv_field(field))
                        .collect::<Vec<_>>()
                        .join(","),
                );
                csv.push('\n');
            }
            std::fs::write(&output, csv)?;
            println!(
                "脱敏 CSV 账本已导出（不含 Prompt/代码/完整响应）：{}",
                output.display()
            );
        }
        other => anyhow::bail!("不支持的导出格式：{other}（可选 json、csv 或 sqlite）"),
    }
    Ok(())
}

fn csv_field(value: &str) -> String {
    if value
        .chars()
        .any(|character| matches!(character, ',' | '"' | '\n' | '\r'))
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}
