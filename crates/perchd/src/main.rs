//! perchd: Perch's engine with no window, for a machine the Mac reaches
//! over ssh.
//!
//! - `perchd serve` runs the engine and listens on
//!   `$PERCH_HOME/run/perchd.sock` until SIGTERM or SIGINT.
//! - `perchd bridge` joins its stdin and stdout to that socket, starting
//!   an engine first when none answers. The app runs `ssh <host> perchd
//!   bridge`, so ssh does the authentication and nothing listens on a port.
//! - `perchd status` says whether an engine is running, and what it has.
//! - `perchd --version` prints `perchd <version> (protocol <n>)`, which the
//!   app reads when it installs one on a host: keep the form.

mod bridge;
mod cleanup;
mod serve;
mod socket;
mod status;

use std::process::ExitCode;

const USAGE: &str = "\
usage: perchd <command>

commands:
  serve      run the engine, listening on $PERCH_HOME/run/perchd.sock
  bridge     join stdin and stdout to the engine, starting it if need be
  status     say whether an engine is running, and what it has
  --version  print perchd's version and wire protocol";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") if args.len() == 1 => serve::run(),
        Some("bridge") if args.len() == 1 => bridge::run(),
        Some("status") if args.len() == 1 => status::run(),
        Some("--version" | "-V" | "version") => {
            println!(
                "perchd {} (protocol {})",
                perch_engine::wire::VERSION,
                perch_engine::wire::PROTOCOL
            );
            ExitCode::SUCCESS
        }
        Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
