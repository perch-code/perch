//! `perch --connect [host]`
//!
//! Connects to the engine on `host`, or on `[remote] host` with none
//! given, as the app would: through `[remote] command` when the Mac's
//! config.toml sets one, else through the `perchd` the app installs there,
//! installing it first when the host has none. Prints the engine's
//! welcome and exits 0, or says why not and exits 1. No window is opened;
//! the engine is left running, as the app leaves it.

use std::time::Duration;

/// How long the engine has to answer once ssh is up: a bridge starting an
/// engine waits up to 10 s for it.
const ANSWER: Duration = Duration::from_secs(30);

pub fn run(args: &[String]) -> i32 {
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into());
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .try_init();
    let remote = perch_github::Config::load().remote;
    let host = match args {
        [] => match remote.host() {
            Some(host) => host.to_string(),
            None => {
                eprintln!("usage: perch --connect <host>\n\nNo host given, and config.toml has no [remote] host.");
                return 2;
            }
        },
        [host] => host.clone(),
        _ => {
            eprintln!("usage: perch --connect <host>");
            return 2;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("perch --connect: no runtime: {error}");
            return 1;
        }
    };
    let bridge = crate::bridge(&remote);
    let connected = runtime.block_on(perch_engine::wire::remote::connect_once(
        &host,
        bridge,
        ANSWER,
        |build| eprintln!("Installing perchd {build} on {host}…"),
    ));
    match connected {
        Ok(welcome) => {
            println!(
                "Connected to {host}: perchd {} (protocol {}) on {}",
                welcome.engine_version, welcome.protocol, welcome.host
            );
            println!("home: {}", welcome.home.display());
            println!("data: {}", welcome.data_root.display());
            println!("other clients: {}", welcome.clients);
            println!("runs: {}", welcome.runs);
            0
        }
        Err(why) => {
            eprintln!("perch --connect: can't connect to {host}: {why}");
            1
        }
    }
}
