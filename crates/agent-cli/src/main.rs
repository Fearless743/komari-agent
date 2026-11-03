//! `komari-agent` command line entry point.

use std::ffi::OsString;
use std::process::ExitCode;

use agent_cli::commands;
use agent_cli::runtime::{RunExit, RuntimeDeps, resolve_config, run_with};
use agent_config::{CliOverrides, Config};
use clap::{Parser, Subcommand};

/// Komari agent command line arguments.
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

    #[command(subcommand)]
    command: Option<Command>,

    /// Positional arguments are accepted and ignored, like the Go root
    /// command, which declares no positional validators.
    #[arg(value_name = "ARGS", hide = true, trailing_var_arg = true)]
    _ignored: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Check memory usage
    CheckMem,
    /// List all physical disks
    ListDisk,
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

    match &cli.command {
        Some(command) => run_subcommand(command, cli.config),
        None => run_agent(cli.config),
    }
}

fn run_subcommand(command: &Command, overrides: CliOverrides) -> ExitCode {
    // The Go subcommands run without the root RunE, but they still use the
    // persistent flags. Load the configuration so `HOST_PROC`,
    // `include_mountpoint` and friends behave the same way.
    let config = match Config::load(overrides) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Error: {error}");
            return ExitCode::FAILURE;
        }
    };

    let root = commands::proc_root(&config.host_proc);
    let result = match command {
        Command::CheckMem => {
            let mut stdout = std::io::stdout();
            commands::check_mem(
                &root,
                config.memory_include_cache,
                config.memory_report_raw_used,
                &mut stdout,
            )
        }
        Command::ListDisk => {
            let mut stdout = std::io::stdout();
            commands::list_disk(&root, &config.include_mountpoints, &mut stdout)
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_agent(overrides: CliOverrides) -> ExitCode {
    agent_cli::init_tracing();
    tracing::info!("Komari Agent {}", agent_cli::full_version());

    let config = match Config::load(overrides) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Error: {error}");
            return ExitCode::FAILURE;
        }
    };
    // `Config`'s Debug implementation masks tokens and secrets.
    tracing::info!("Loaded configuration: {config:#?}");

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Error: failed to start async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(config)) {
        Ok(RunExit::Finished) => ExitCode::SUCCESS,
        Ok(RunExit::Restart(code)) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(config: Config) -> Result<RunExit, agent_cli::RuntimeError> {
    let config = resolve_config(config, None, None).await?;
    let deps = RuntimeDeps::production(&config)?;
    run_with(config, deps).await
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

    #[test]
    fn subcommands_parse_with_global_flags() {
        let cli = Cli::try_parse_from(["komari-agent", "--token", "t", "check-mem"]).unwrap();
        assert!(matches!(cli.command, Some(Command::CheckMem)));

        let cli = Cli::try_parse_from(["komari-agent", "list-disk"]).unwrap();
        assert!(matches!(cli.command, Some(Command::ListDisk)));
    }
}
