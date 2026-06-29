mod config;
mod context;
mod executor;
mod repl;
mod scheme;
mod transport;

use std::rc::Rc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use ma_core::config::{Config, MaArgs, SecretBundle};
use ma_core::{IpfsGatewayResolver, RPC_PROTOCOL_ID};
use tokio::task::spawn_local;
use tracing::{info, warn};
use zeroize::Zeroize;

use crate::config::SchemeConfig;
use crate::context::CliCtx;
use crate::scheme::init_session_env;

const ZSCHEME_SLUG: &str = "zscheme";
const DEFAULT_GATEWAY_URL: &str = "https://dweb.link";

// ── CLI ────────────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
#[command(name = "zscheme")]
#[command(about = "zscheme — a Scheme interpreter for the ma actor network")]
struct Cli {
    #[command(flatten)]
    ma: MaArgs,

    /// Script file to execute. If omitted, starts the interactive REPL.
    script: Option<std::path::PathBuf>,

    /// IPFS gateway URL (fallback when local Kubo is unavailable).
    #[arg(long, default_value = DEFAULT_GATEWAY_URL, env = "ZSCHEME_GATEWAY")]
    gateway: String,

    /// How often to drain the iroh RPC inbox for actor-call replies (milliseconds).
    /// Lower values reduce latency for (@actor verb) and (rpc-send …) calls.
    #[arg(long, default_value_t = 50, env = "ZSCHEME_RPC_POLL_MS")]
    rpc_poll_ms: u64,
}

// ── Entry point ────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle --gen-headless-config
    if cli.ma.gen_headless_config {
        Config::gen_headless(&cli.ma, ZSCHEME_SLUG)?;
        return Ok(());
    }

    // ── Config ─────────────────────────────────────────────────────────────
    let bundle_path_check = {
        let cfg_tmp = Config::from_args(&cli.ma, ZSCHEME_SLUG)?;
        cfg_tmp.effective_secret_bundle()?
    };

    let core_config = if !bundle_path_check.exists() {
        warn!("No zscheme identity found — generating a new one.");
        Config::gen_headless(&cli.ma, ZSCHEME_SLUG)?;
        Config::from_args(&cli.ma, ZSCHEME_SLUG)?
    } else {
        Config::from_args(&cli.ma, ZSCHEME_SLUG)?
    };

    // Set up stderr-only tracing (stdout is reserved for script output).
    // Note: --log-level-stdout from MaArgs is a no-op in zscheme; logging
    // is controlled via RUST_LOG or the YAML log_level / log_file settings.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    // ── Secret bundle ───────────────────────────────────────────────────────
    let mut secrets = load_secret_bundle(&core_config)?;

    // ── iroh endpoint ───────────────────────────────────────────────────────
    let mut endpoint = ma_core::new_ma_endpoint(secrets.iroh_secret_key, true).await?;
    let rpc_inbox = endpoint.service(RPC_PROTOCOL_ID);

    // ── DID document ────────────────────────────────────────────────────────
    let ma_ext = endpoint.ma_extension().kind("agent");
    let our_document = secrets
        .build_document(ma_ext)
        .context("failed to build DID document")?;
    let our_did = our_document.id.clone();
    info!(did = %our_did, "zscheme identity ready");

    // Box the endpoint now that service() and ma_extension() are done.
    // (new_ma_endpoint already returns Box<dyn MaEndpoint>; no re-boxing needed.)

    // Zeroize key material we no longer need.
    secrets.ipns_secret_key.zeroize();

    // ── Scheme data config ──────────────────────────────────────────────────
    let data_path = SchemeConfig::default_path()?;
    let scheme_config = SchemeConfig::load(&data_path);

    // ── Build CliCtx ────────────────────────────────────────────────────────
    let resolver = Rc::new(IpfsGatewayResolver::default());
    let signing_key_bytes = secrets.did_signing_key;

    let ctx = CliCtx::new(
        scheme_config,
        our_did.clone(),
        signing_key_bytes,
        endpoint,
        resolver,
        rpc_inbox,
        core_config.kubo_rpc_url.clone(),
        cli.gateway.clone(),
    );

    // Zeroize signing key copy from secrets after it has been stored in ctx.
    secrets.did_signing_key.zeroize();

    // ── Run in LocalSet (required for Rc<…> + LocalBoxFuture) ─────────────
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async_main(ctx, cli.script, cli.rpc_poll_ms))
        .await
}

async fn async_main(
    ctx: crate::context::Ctx,
    script: Option<std::path::PathBuf>,
    poll_ms: u64,
) -> Result<()> {
    // Initialise session environment.
    init_session_env();

    // Start the RPC reply poll loop.
    let ctx_poll = ctx.clone();
    spawn_local(async move {
        let interval = Duration::from_millis(poll_ms);
        loop {
            tokio::time::sleep(interval).await;
            ctx_poll.poll_rpc_replies();
        }
    });

    // Execute script or REPL, then close the endpoint cleanly.
    let result = if let Some(ref path) = script {
        executor::run_file(path, ctx.clone()).await
    } else {
        repl::run_repl(ctx.clone()).await
    };
    ctx.close().await;
    result
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn load_secret_bundle(config: &Config) -> Result<SecretBundle> {
    let passphrase = config
        .secret_bundle_passphrase
        .as_deref()
        .ok_or_else(|| anyhow!("secret_bundle_passphrase is required (set MA_SECRET_BUNDLE_PASSPHRASE or add it to {ZSCHEME_SLUG}.yaml)"))?;
    let bundle_path = config.effective_secret_bundle()?;
    SecretBundle::load(&bundle_path, passphrase)
        .with_context(|| format!("failed to load secret bundle from {}", bundle_path.display()))
}
