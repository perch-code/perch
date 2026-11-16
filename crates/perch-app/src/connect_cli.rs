//! `perch --connect [host]`
//!
//! Connects to the engine on `host`, or on `[remote] host` with none
//! given, as the app would: through `[remote] command` when the Mac's
//! config.toml sets one, else through the `perchd` the app installs there,
//! installing it first when the host has none, and replacing an older
//! engine with nothing running as the app would. Prints the engine's
//! welcome and exits 0, or says why not and exits 1. An older engine left
//! running for its runs or clients, or a newer one, is said after the
//! welcome, and then what the host lacks, with what to run for each. No
//! window is opened; the engine is left running, as the app leaves it.

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
        |said| eprintln!("{said}"),
    ));
    match connected {
        Ok(perch_engine::wire::remote::Connected {
            welcome,
            other,
            checks,
        }) => {
            println!(
                "Connected to {host}: perchd {} (protocol {}) on {}",
                welcome.engine_version, welcome.protocol, welcome.host
            );
            println!("build: {}", welcome.build);
            println!("home: {}", welcome.home.display());
            println!("data: {}", welcome.data_root.display());
            println!("other clients: {}", welcome.clients);
            println!("runs: {}", welcome.runs);
            if let Some(other) = other {
                println!("{}", perch_engine::wire::remote::other_said(&host, &other));
            }
            match checks {
                Some(checks) => print!("{}", checks_said(&checks)),
                None => println!("host checks: the engine did not send them in time"),
            }
            0
        }
        Err(why) => {
            eprintln!("perch --connect: can't connect to {host}: {why}");
            1
        }
    }
}

/// The host's checks, a line each, with what to run under one that
/// failed.
fn checks_said(checks: &[perch_engine::host::HostCheck]) -> String {
    let mut said = String::from("host checks:\n");
    for check in checks {
        let mark = match (check.ok, check.required) {
            (true, _) => "ok",
            (false, true) => "missing",
            (false, false) => "optional",
        };
        said.push_str(&format!("  {mark:<9}{}: {}\n", check.what, check.detail));
        if let Some(fix) = check.fix.as_deref().filter(|_| !check.ok) {
            said.push_str(&format!("           run: {fix}\n"));
        }
    }
    if perch_engine::host::needs_setup(checks) {
        said.push_str("The host needs setup: run what is under each missing check there.\n");
    }
    said
}

#[cfg(test)]
mod tests {
    use perch_engine::host::HostCheck;

    #[test]
    fn checks_are_said_a_line_each_with_the_fix_under() {
        let check = |what: &str, ok, required, fix: Option<&str>| HostCheck {
            what: what.into(),
            ok,
            required,
            detail: format!("{what} detail."),
            fix: fix.map(str::to_string),
        };
        let said = super::checks_said(&[
            check("git", true, true, None),
            check("GitHub token", false, true, Some("gh auth login")),
            check("Codex", false, false, Some("npm install -g @openai/codex")),
        ]);
        assert_eq!(
            said,
            "host checks:\n  ok       git: git detail.\n  missing  GitHub token: GitHub token detail.\n           run: gh auth login\n  optional Codex: Codex detail.\n           run: npm install -g @openai/codex\nThe host needs setup: run what is under each missing check there.\n"
        );
        assert!(!super::checks_said(&[check("git", true, true, None)]).contains("needs setup"));
    }
}
