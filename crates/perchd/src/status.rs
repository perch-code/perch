//! `perchd status`: whether an engine answers on this home's socket, and
//! what its welcome says. Never starts one.

use std::process::ExitCode;
use std::time::Duration;

use perch_engine::wire::{self, WireError};

use crate::socket;

/// How long the engine gets to say hello back.
const ANSWER: Duration = Duration::from_secs(5);

pub fn run() -> ExitCode {
    let socket = socket::path();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("perchd status: {error}");
            return ExitCode::FAILURE;
        }
    };
    let answer = runtime.block_on(async {
        let Ok(stream) = tokio::net::UnixStream::connect(&socket).await else {
            return None;
        };
        let answer = tokio::time::timeout(ANSWER, wire::connect(stream)).await;
        Some(match answer {
            Ok(connected) => connected.map(|(_backend, _events, welcome)| welcome),
            Err(_) => Err(WireError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("said nothing within {} s", ANSWER.as_secs()),
            ))),
        })
    });
    let Some(answer) = answer else {
        println!("not running: nothing answers on {}", socket.display());
        return ExitCode::from(3);
    };
    match answer {
        Ok(welcome) => {
            println!(
                "running: Perch {} (protocol {}) on {}",
                welcome.engine_version, welcome.protocol, welcome.host
            );
            println!("perch home: {}", welcome.data_root.display());
            println!("clients: {}", welcome.clients);
            println!("runs: {}", welcome.runs);
            ExitCode::SUCCESS
        }
        Err(WireError::Refused(reason)) => {
            println!("running, but it refused this perchd: {reason}");
            ExitCode::FAILURE
        }
        Err(error) => {
            println!(
                "not answering: {} took a connection but {error}",
                socket.display()
            );
            ExitCode::FAILURE
        }
    }
}
