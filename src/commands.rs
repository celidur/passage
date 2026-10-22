//! Implementations of the `init`, `check` and `serve` subcommands.

use crate::{
    api,
    certificates::{CertificateManager, Certificates},
    config::Config,
    proxy::PassageProxy,
    routes::Snapshot,
    state::AppState,
    store::Store,
    token::AdminToken,
    upstream::Maintenance,
};
use anyhow::{Context, Result};
use pingora::{
    apps::HttpServerOptions, listeners::tls::TlsSettings, prelude::*,
    services::background::background_service,
};
use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};

const TOKEN_FILE: &str = "admin.token";
const DATABASE_FILE: &str = "passage.sqlite";

pub fn init(config: &Config) -> Result<()> {
    let directory = &config.general.data_dir;
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
    let state = load(config, true)?;
    println!(
        "Configuration valid: {} active routes",
        state.snapshot.load().active_routes()
    );
    Ok(())
}

pub fn serve(config: Config) -> Result<()> {
    let state = Arc::new(load(&config, false)?);
    let admin = api::bind(config.admin.listen).context("cannot bind management listener")?;

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
    proxy.add_tcp(&config.http.listen.to_string());
    // Cleartext HTTP/2 (prior knowledge) is what plain gRPC clients speak.
    let mut options = HttpServerOptions::default();
    options.h2c = true;
    proxy
        .app_logic_mut()
        .expect("proxy logic not shared during startup")
        .server_options = Some(options);
    if let Some(https) = &config.https {
        // Certificates are chosen per server name while handshaking, so new ones apply live.
        let mut settings = TlsSettings::with_callbacks(Box::new(state.certificates.resolver()))?;
        settings.enable_h2();
        proxy.add_tls_with_settings(&https.listen.to_string(), None, settings);
    }
    server.add_service(proxy);
    server.add_service(background_service(
        "management",
        api::Management::new(state.clone(), admin),
    ));
    if state.certificates.features().acme {
        server.add_service(background_service(
            "certificates",
            CertificateManager {
                certificates: state.certificates.clone(),
                http: config.http.listen,
            },
        ));
    }
    server.add_service(background_service(
        "upstream-maintenance",
        Maintenance::new(state, Duration::from_secs(config.general.dns_refresh_secs)),
    ));

    tracing::info!(
        admin = %config.admin.listen,
        http = %config.http.listen,
        https = ?config.https.as_ref().map(|https| https.listen),
        "Passage starting"
    );
    server.run_forever();
}

/// Reads the token and stored routes, resolving every enabled upstream.
fn load(config: &Config, strict: bool) -> Result<AppState> {
    let token = AdminToken::read(&config.general.data_dir.join(TOKEN_FILE))?;
    let store = Arc::new(Store::open(&config.general.data_dir.join(DATABASE_FILE))?);
    let certificates = Arc::new(Certificates::new(store.clone(), config)?);
    let routes = store.load()?;
    let snapshot = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(Snapshot::compile(routes, strict))
        .context("stored routes are invalid")?;
    Ok(AppState::new(store, snapshot, token, certificates))
}
