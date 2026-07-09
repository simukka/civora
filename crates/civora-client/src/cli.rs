//! Command-line lobby flags. Three flags don't justify a parser dependency.
//!
//! ```text
//! civora-client                    offline single player (unchanged)
//! civora-client --host             host: listen, print the join address
//! civora-client --join [ADDR]      join ADDR, or the first mDNS peer if omitted
//! civora-client --key-file PATH    identity key override (two instances on
//!                                  one machine need distinct identities)
//! civora-client --ledger-file PATH accepted-proposal ledger override (two
//!                                  instances need distinct ledgers, like keys)
//! ```

use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum NetMode {
    #[default]
    Offline,
    Host,
    Join {
        dial: Option<String>,
    },
}

#[derive(Default)]
pub struct CliArgs {
    pub net: NetMode,
    pub key_file: Option<PathBuf>,
    pub ledger_file: Option<PathBuf>,
}

pub fn parse() -> Result<CliArgs, String> {
    let mut cli = CliArgs::default();
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--host" => {
                if cli.net != NetMode::Offline {
                    return Err("--host and --join are mutually exclusive".into());
                }
                cli.net = NetMode::Host;
            }
            "--join" => {
                if cli.net != NetMode::Offline {
                    return Err("--host and --join are mutually exclusive".into());
                }
                // The address is optional: a bare --join waits for mDNS.
                let takes_addr = args.peek().is_some_and(|next| !next.starts_with("--"));
                let dial = takes_addr.then(|| args.next().expect("peeked"));
                cli.net = NetMode::Join { dial };
            }
            "--key-file" => {
                let path = args
                    .next()
                    .ok_or_else(|| "--key-file needs a path".to_owned())?;
                cli.key_file = Some(PathBuf::from(path));
            }
            "--ledger-file" => {
                let path = args
                    .next()
                    .ok_or_else(|| "--ledger-file needs a path".to_owned())?;
                cli.ledger_file = Some(PathBuf::from(path));
            }
            other => return Err(format!("unknown argument {other:?} (see --host/--join)")),
        }
    }
    Ok(cli)
}
