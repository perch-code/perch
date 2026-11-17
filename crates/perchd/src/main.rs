//! perchd: Perch's engine with no window, for a machine the Mac reaches
//! over ssh.
//!
//! - `perchd serve` runs the engine and listens on
//!   `$PERCH_HOME/run/perchd.sock` until SIGTERM or SIGINT. With
//!   `--exit-after-idle <seconds>`, also until it has had no client and
//!   no run for that long: how a bridge starts one, so an engine nobody
//!   is using does not stay on the host.
//! - `perchd bridge` joins its stdin and stdout to that socket, starting
//!   an engine first when none answers. The app runs `ssh <host> perchd
//!   bridge`, so ssh does the authentication and nothing listens on a port.
//!   `--replace` and `--replace-if-idle` first stop an engine of another
//!   build.
//! - `perchd status` says whether an engine is running, and what it has.
//! - `perchd --version` prints `perchd <version> (protocol <n>)`.

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
             --exit-after-idle <seconds>  stop once it has had no client
                                and no agent run for that long
  bridge     join stdin and stdout to the engine, starting it if need be
             --replace          first stop an engine of another build
             --replace-if-idle  the same, only if it has no runs or clients;
                                else say perch-busy and exit 98
  status     say whether an engine is running, and what it has
  --version  print perchd's version and wire protocol";

/// Where this `perchd` is, as it can be run again or looked for beside
/// its own. Linux names a running binary whose name has since been given
/// to another file, or taken away, `<path> (deleted)`: the path is still
/// the one to use, since what is there now, if anything, is the same
/// build under the same name.
pub fn own_exe() -> std::io::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(
        match exe
            .to_str()
            .and_then(|path| path.strip_suffix(" (deleted)"))
        {
            Some(path) => path.into(),
            None => exe,
        },
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => match serve::parse(&args[1..]) {
            Some(idle) => serve::run(idle),
            None => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        Some("bridge") => match bridge::parse(&args[1..]) {
            Some(replace) => bridge::run(replace),
            None => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
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
