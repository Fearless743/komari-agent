//! `komari-agent` command line entry point.

use std::ffi::OsString;
use std::process::ExitCode;

use agent_config::{CliOverrides, Config};
use clap::Parser;

/// Komari agent command line arguments.
///
/// Flags mirror the Go agent's root command. Business subcommands
/// (`check-mem`, `list-disk`) will be added together with the monitoring
/// implementation.
#[derive(Parser)]
#[command(
    name = "komari-agent",
    version = agent_cli::full_version(),
    about = "komari agent",
    long_about = "komari agent"
)]
struct Cli {
    #[command(flatten)]
    config: CliOverrides,
}

fn main() -> ExitCode {
    let args = sanitize_args(std::env::args_os());
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        // Clap prints help/version itself and uses exit code 0 for them.
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1));
        }
    };

    agent_cli::init_tracing();
    tracing::info!("Komari Agent {}", agent_cli::full_version());

    match Config::load(cli.config) {
        Ok(config) => {
            // `Config`'s Debug implementation masks tokens and secrets.
            tracing::info!("Loaded configuration: {config:#?}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Drop flags that the Go agent removed but still accepts with a warning.
fn sanitize_args<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
    args.into_iter()
        .filter(|arg| match arg.to_str() {
            Some("-autoUpdate" | "--autoUpdate") => {
                eprintln!(
                    "WARNING: The -autoUpdate flag is deprecated in version 0.0.9 and later. \
                     Use --disable-auto-update to configure auto-update behavior."
                );
                false
            }
            Some("-memory-mode-available" | "--memory-mode-available") => {
                eprintln!(
                    "WARNING: The --memory-mode-available flag is deprecated in version 1.0.70 \
                     and later. Use --memory-include-cache to report memory usage including \
                     cache/buffer."
                );
                false
            }
            _ => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deprecated_flags_are_removed() {
        let args = sanitize_args(
            ["komari-agent", "--autoUpdate", "--token", "abc"]
                .into_iter()
                .map(OsString::from),
        );

        assert_eq!(
            args,
            vec![
                OsString::from("komari-agent"),
                OsString::from("--token"),
                OsString::from("abc"),
            ]
        );
    }

    #[test]
    fn regular_arguments_are_kept() {
        let args = sanitize_args(
            ["komari-agent", "--interval", "2.5"]
                .into_iter()
                .map(OsString::from),
        );

        assert_eq!(args.len(), 3);
    }
}
