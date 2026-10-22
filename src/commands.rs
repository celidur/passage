//! Implementations of the `init`, `check` and `serve` subcommands.

use crate::{
    api, config::Config, proxy::PassageProxy, routes::Snapshot, state::AppState, store::Store,
    token::AdminToken,
};
use anyhow::{Context, Result};
use pingora::{prelude::*, services::background::background_service};
use std::{os::unix::fs::PermissionsExt, sync::Arc};

const TOKEN_FILE: &str = "admin.token";
const DATABASE_FILE: &str = "passage.sqlite";

pub fn init(config: &Config) -> Result<()> {
    let directory = &config.data_dir;
    std::fs::create_dir_all(directory)?;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    let token_file = directory.join(TOKEN_FILE);
    AdminToken::generate(&token_file)?;
    Store::open(&directory.join(DATABASE_FILE))?;
    println!(
        "Initialized {}\nAdministrator token: {} (read locally; never commit it)",
        directory.display(),
        token_file.display()
    );
    Ok(())
}

pub fn check(config: &Config) -> Result<()> {
    let state = load(config)?;
    println!(
        "Configuration valid: {} active routes",
        state.snapshot.load().active_routes()
    );
    Ok(())
}

pub fn serve(config: Config) -> Result<()> {
    let state = Arc::new(load(&config)?);
    let admin = api::bind(config.admin_listen).context("cannot bind management listener")?;

    let mut server = Server::new(None)?;
    let server_config =
        Arc::get_mut(&mut server.configuration).expect("server not shared during startup");
    server_config.grace_period_seconds = Some(1);
    server_config.graceful_shutdown_timeout_seconds = Some(10);
    server.bootstrap();

    let mut proxy = pingora::proxy::http_proxy_service(
        &server.configuration,
        PassageProxy::new(state.clone())?,
    );
    proxy.add_tcp(&config.proxy_listen.to_string());
    if let Some(tls) = &config.tls {
        proxy.add_tls(
            &tls.listen.to_string(),
            tls.certificate
                .to_str()
                .context("invalid certificate path")?,
            tls.private_key.to_str().context("invalid key path")?,
        )?;
    }
    server.add_service(proxy);
    server.add_service(background_service(
        "management",
        api::Management::new(state, admin),
    ));

    tracing::info!(
        admin = %config.admin_listen,
        proxy = %config.proxy_listen,
        tls = ?config.tls.as_ref().map(|t| t.listen),
        "Passage starting"
    );
    server.run_forever();
}

/// Reads the token and stored routes, resolving every enabled upstream.
fn load(config: &Config) -> Result<AppState> {
    let token = AdminToken::read(&config.data_dir.join(TOKEN_FILE))?;
    let store = Store::open(&config.data_dir.join(DATABASE_FILE))?;
    let routes = store.load()?;
    let snapshot = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(Snapshot::compile(routes))
        .context("stored routes are invalid")?;
    Ok(AppState::new(store, snapshot, token))
}
