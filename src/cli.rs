#![allow(private_interfaces)]

use std::path::PathBuf;

use anyhow::Context;
use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use docker_api::{conn::TtyChunk, Docker};
use futures::StreamExt;
use uuid::Uuid;

use crate::{compose::running_containers, LATEST};

#[derive(Parser, Debug)]
#[command(version)]
/// MSDE CLI
///
/// The command line tool to work with the Merigo developer package.
pub struct Command {
    /// Enables verbose output.
    #[arg(short, long)]
    pub debug: bool,

    /// Skip building a local cache of the MSDE image registry.
    #[arg(short, long)]
    pub no_cache: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

impl Command {
    pub fn should_ignore_credentials(&self) -> bool {
        matches!(
            self.command,
            None | Some(
                Commands::Shell { .. }
                    | Commands::Ssh { .. }
                    | Commands::RunHooks { .. }
                    | Commands::CreateGame { .. }
                    | Commands::Run { .. }
                    | Commands::ImportGames { .. }
                    | Commands::Rpc { .. }
                    | Commands::Log { .. }
                    | Commands::Down { .. }
                    | Commands::Up { .. }
                    | Commands::Docs
                    | Commands::Status
                    | Commands::AddProfile { .. }
                    | Commands::SetProject { .. }
                    | Commands::GenerateCompletions { .. }
                    | Commands::UpgradeProject { .. }
                    | Commands::Clean { .. }
                    | Commands::Init { .. }
                    | Commands::BuildCache { .. }
                    | Commands::LegacyLogin { .. }
                    | Commands::Containers { .. }
                    | Commands::UpdateBeamFiles { .. }
                    | Commands::VerifyBeamFiles { .. }
            )
        )
    }
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    #[cfg(all(feature = "local_auth", debug_assertions))]
    RunAuthServer,
    Login {
        #[arg(
            short,
            long,
            conflicts_with = "token_stdin",
            required_unless_present = "token_stdin",
            env = "MERIGO_TOKEN"
        )]
        token: Option<String>,

        #[arg(long)]
        token_stdin: bool,
    },
    #[cfg(all(feature = "local_auth", debug_assertions))]
    Register {
        #[arg(short, long)]
        name: String,
    },
    /// Create and register a new game from the default template.
    CreateGame {
        /// The name of the game.
        #[arg(short, long)]
        game: String,

        /// The stage name of the game.
        #[arg(short, long)]
        stage: String,

        /// If given, create the game with the given fixed guid, otherwise it'll be random.
        #[arg(long)]
        guid: Option<Uuid>,

        /// If given, create the game with the given fixed suid, otherwise it'll be random.
        #[arg(long)]
        suid: Option<Uuid>,
    },
    /// Import all games from the project directory. This command will look at your active project path in games/stages.yml,
    /// and will import all valid games listed there. For more information how it works, see <https://docs.merigo.co/getting-started/devpackage#using-config-stages.yml>
    ImportGames {
        /// Don't print output to the terminal.
        #[arg(short, long, action = ArgAction::SetTrue)]
        quiet: bool,
    },
    /// Call into the MSDE system with an RPC. The MSDE service must be running.
    ///
    /// Example:
    ///
    /// > msde-cli rpc 'IO.puts("hello")'
    Rpc {
        /// The Elixir command to run as a quoted string.
        #[arg(num_args = 1)]
        cmd: String,
    },
    /// Open the documentation page for this package.
    Docs,
    /// Show the project status. WIP.
    Status,
    /// Sets the project path to the given directory. The directory must contain a valid top-level `metadata.json`.
    SetProject {
        #[arg(index = 1)]
        path: Option<PathBuf>,
    },
    /// Register a new profile for running the developer package.
    AddProfile {
        /// The name of the profile.
        #[arg(short, long)]
        name: String,

        #[arg(short, long, value_delimiter = ',', num_args = 1..)]
        features: Vec<crate::env::Feature>,
    },
    /// Generate shell auto-completions for this CLI tool.
    ///
    /// This command writes auto-completions to stdout, so users are encouraged to pipe it to a file.
    ///
    /// Example:
    ///
    /// > msde-cli generate-completions | sudo tee /usr/share/bash-completion/completions/msde-cli.bash
    GenerateCompletions {
        /// The target shell to generate auto-completions for. If not given, the current shell will be detected.
        #[arg(short, long)]
        shell: Option<Shell>,
    },
    /// Upgrade the active project that was generated with an earlier version of this tool.
    UpgradeProject {
        /// The path of the project. It's automatically detected, but use this option to override.
        #[arg(short, long)]
        path: Option<std::path::PathBuf>,

        /// Only display manual steps, do not perform any action. This is useful if the user missed the printed instructions.
        #[arg(short, long)]
        manual_only: bool,

        /// Proceed without asking for further confirmation.
        #[arg(short, long)]
        allow_overwrite: bool,
    },
    /// Start the services, and wait for the MSDE to be healthy.
    Up {
        /// The features to enable for this run.
        #[arg(short, long, value_delimiter = ',', num_args = 1..)]
        features: Vec<crate::env::Feature>,

        /// The maximum duration in seconds to wait for services to be healthy before exiting.
        #[arg(short, long, default_value_t = 300)]
        timeout: u64,

        /// Do not print anything to the terminal
        #[arg(short, long, action = ArgAction::SetTrue)]
        quiet: bool,

        /// After a successful start attach to MSDE container logs.
        #[arg(long, action = ArgAction::SetTrue)]
        attach: bool,

        /// (Re)build the services (pass --build to docker compose).
        #[arg(long, action = ArgAction::SetTrue)]
        build: bool,

        /// Start all commands in raw mode, meaning all output is transmitted to the calling terminal without changes.
        #[arg(long, action = ArgAction::SetTrue, conflicts_with = "quiet")]
        raw: bool,

        /// The profile to use. This defines which features are enabled. If not given, the minimal profile is used.
        #[arg(short, long, conflicts_with = "features")]
        profile: Option<String>,
    },
    /// Wipe out all config files related to this tool.
    Clean {
        /// Continue without asking for further confirmation.
        #[arg(short = 'y', long, action = ArgAction::SetTrue)]
        always_yes: bool,
    },
    /// Runs the target service(s), imports all valid games from the project folder.
    /// It the same effect as the following commands combined:
    ///
    /// `msde-cli run-hooks --pre && msde-cli up [args] && msde-cli import-games && msde-cli run-hooks --post`
    ///
    /// ## Hooks
    ///
    /// Hooks are custom scripts that integrate into this command's lifecycle: pre_run hooks are executed before spinning up
    /// the developer package, and post_run scripts are after. Hooks are executed in the order they're defined.
    ///
    /// To register a hook, add it to the metadata.json of an active project under the `hooks.pre_run` or `hooks.post_run` arrays.
    /// The only required option is the "cmd" key, that describes which command to execute, but there're also optional keys to
    /// control other aspects of the command.
    ///
    /// An example metadata.json with a pre_run hook:
    ///
    /// {
    ///    "target_msde_version": "3.10.0",
    ///    "self_version": "0.14.0",
    ///    "timestamp": 1717739833,
    ///    "hooks": {
    ///        "pre_run": [
    ///            {
    ///                "cmd": "ls",
    ///                "args": [
    ///                    "-la"
    ///                ],
    ///                "env_overrides": {
    ///                    "MY_KEY": "MY_VALUE"
    ///                },
    ///                "working_directory": "/home/user/merigo",
    ///                "continue_on_error": false,
    ///                "hide_output": false
    ///            }
    ///        ],
    ///        "post_run": []
    ///    }
    /// }
    ///
    /// `args`: Additional arguments to pass to the command.
    ///
    /// `env_overrides`: Environment variables to set for the running command.
    ///
    /// `working_directory`: The directory to execute the command in. Must be an absolute path.
    ///
    /// `continue_on_error`: Don't stop the run if this command failed (exited with non-zero code). [default: false]
    ///
    /// `hide_output`: Don't display the output of this command. [default: false]
    ///
    /// Any script invoked by the MSDE-CLI tool sets the `MSDE_CLI_RUNNER` environment variable to `true`, so you may rely on that
    /// to distinguish executions.
    ///
    /// If you're executing a shell script, it's important that:
    ///
    /// - it's executable
    ///
    /// - it starts with an appropriate shebang, for instance: `#!/usr/bin/env bash`
    Run {
        /// The features to enable for this run.
        #[arg(short, long, value_delimiter = ',', num_args = 1..)]
        features: Vec<crate::env::Feature>,

        /// The maximum duration in seconds to wait for services to be healthy before exiting.
        #[arg(short, long, default_value_t = 300)]
        timeout: u64,

        /// Do not print anything to the terminal
        #[arg(short, long, action = ArgAction::SetTrue)]
        quiet: bool,

        /// After a successful start attach to MSDE container logs.
        #[arg(long, action = ArgAction::SetTrue)]
        attach: bool,

        /// (Re)build the services (pass --build to docker compose).
        #[arg(long, action = ArgAction::SetTrue)]
        build: bool,

        /// Start all commands in raw mode, meaning all output is transmitted to the calling terminal without changes.
        #[arg(long, action = ArgAction::SetTrue, conflicts_with = "quiet")]
        raw: bool,

        /// Skip executing the registered pre and post run hooks.
        #[arg(long, action = ArgAction::SetTrue)]
        no_hooks: bool,

        /// The profile to use. This defines which features are enabled. If not given, the minimal profile is used.
        #[arg(short, long, conflicts_with = "features")]
        profile: Option<String>,
    },
    /// Run the defined hooks, if there are any. This command requires at least one of the --pre of --post flag to define which set of
    /// hooks to execute. This command will run hooks in the order they're defined in (and runs pre before post hooks, obviously).
    ///
    /// See `msde-cli run --help` for further description on hooks.
    RunHooks {
        #[arg(long, action = ArgAction::SetTrue, required_unless_present = "post")]
        pre: bool,

        #[arg(long, action = ArgAction::SetTrue, required_unless_present = "pre")]
        post: bool,
    },
    Stop {
        /// The maximum wait duration in seconds for the stop command to finish before exiting with an error.
        #[arg(short, long, default_value_t = 300)]
        timeout: u64,
    },
    // TODO: This is almost the same as `Up`.
    Start,
    /// Stop all running services and remove stored game data by cleaning associated Docker volumes.
    Down {
        /// The maximum wait duration in seconds for the down command to finish before exiting with an error.
        #[arg(short, long, default_value_t = 300)]
        timeout: u64,
    },
    /// Attach the logs of the target service. This command will not display logs from the past.
    Log {
        #[command(subcommand)]
        target: Target,
    },
    /// Pull the latest docker image of the target service(s).
    Pull {
        #[command(subcommand)]
        target: Option<Target>,

        // Note: the "version" argument in the other subcommand (kind of confusing)
        /// The specific version to pull.
        #[arg(short, long, required_unless_present = "version")]
        version: Option<String>,
    },
    /// SSH into the running container.
    Ssh {
        #[command(subcommand)]
        target: Target,
    },
    /// Attach to the Elixir shell via a remote_console in the running container.
    Shell {
        #[command(subcommand)]
        target: Target,
    },
    /// Initialize the MSDE developer package.
    ///
    /// This command will not delete any files, but will override anything in the target directory if the package content
    /// conflicts with an existing file.
    /// For that exact reason a non-empty directory will be rejected, unless the --force flag is present.
    Init {
        /// The path where to initialize the project.
        #[arg(short, long)]
        path: Option<std::path::PathBuf>,

        /// Allows initializing inside non-empty directories.
        #[arg(long, action = ArgAction::SetTrue)]
        force: bool,

        /// Pull the associated images prematurely. Use it with the `--features` flag to specify which features to pull.
        #[arg(short, long, action = ArgAction::SetTrue)]
        pull_images: bool,

        /// Don't pull any associated images prematurely.
        #[arg(short, long, action = ArgAction::SetTrue, conflicts_with = "pull_images")]
        no_pull_images: bool,

        /// The target features to pull. If no features is required, just pass the empty value like so: `--features `.
        #[arg(short, long, value_delimiter = ',', num_args = 0..)]
        features: Option<Vec<crate::env::Feature>>,
    },
    /// Verify the integrity of BEAM files.
    VerifyBeamFiles {
        /// The version to verify the BEAM files against.
        #[arg(short, long)]
        version: Option<semver::Version>,

        /// The path where the BEAM files are located. By default, this is the `project_folder/merigo_extension`.
        #[arg(short, long)]
        path: Option<std::path::PathBuf>,
    },
    /// Update the BEAM files. `version`, if not given, is determined by the active project `metadata.json`'s version. If that's not present
    /// either, then the upstream Merigo version is used (only updated when this tool is updated).
    UpdateBeamFiles {
        /// The version of the BEAM files to download.
        #[arg(short, long)]
        version: Option<semver::Version>,

        /// The path where the BEAM files should be put. By default, this is the `project_folder/merigo_extension`.
        #[arg(short, long)]
        path: Option<std::path::PathBuf>,

        /// Skip verifying the integrity of the BEAM files.
        #[arg(long, action = ArgAction::SetTrue)]
        no_verify: bool,
    },
    // TODO: This command doesn't really make sense. Maybe as an element of a project upgrade?
    /// Checks and stops all running containers.
    Containers {
        #[arg(short = 'y', long, action = ArgAction::SetTrue)]
        always_yes: bool,
    },
    // TODO: This is broken if auth is not correct. Also it doesn't really make sense?
    /// Build a cache around all available Merigo Docker images in the remote registry.
    BuildCache {
        /// Specifies the expiration duration of the cache in hours.
        #[arg(short, long)]
        duration: Option<i64>,
    },
    /// Check the available versions of the target service.
    Versions {
        #[command(subcommand)]
        target: Target,
    },
    LegacyLogin {
        // The key used for GHCR authentication.
        #[arg(short, long)]
        ghcr_key: Option<String>,
        // The key used for pulling Merigo images.
        #[arg(short, long)]
        pull_key: Option<String>,

        #[arg(short, long)]
        file: Option<std::path::PathBuf>,
    },
}

#[derive(Clone, PartialEq, Eq, Debug, Subcommand)]
#[command(subcommand_negates_reqs = true)]
pub enum Target {
    Msde {
        #[arg(short, long)]
        version: Option<String>,
    },
    Bot {
        #[arg(short, long)]
        version: Option<String>,
    },
    Web3 {
        #[arg(short, long)]
        version: Option<String>,

        #[arg(short, long)]
        kind: Option<Web3Kind>,
    },
    Compiler {
        #[arg(short, long)]
        version: Option<String>,
    },
}

#[derive(Clone, PartialEq, Eq, Debug, ValueEnum)]
pub enum Web3Kind {
    All,
    Consumer,
    Producer,
}

impl Target {
    pub async fn attach(&self, docker: &Docker) -> anyhow::Result<()> {
        let id = self.get_id(docker).await?;

        let container = docker.containers().get(id);

        let mut multiplexer = container.attach().await?;
        while let Some(chunk) = multiplexer.next().await {
            if let Ok(TtyChunk::StdOut(chunk) | TtyChunk::StdErr(chunk)) = chunk {
                print!("{}", String::from_utf8_lossy(&chunk));
            }
        }
        Ok(())
    }
    pub fn get_version(&self) -> Option<&String> {
        match self {
            Target::Msde { version }
            | Target::Bot { version }
            | Target::Web3 { version, .. }
            | Target::Compiler { version } => version.as_ref(),
        }
    }

    pub async fn get_id(&self, docker: &Docker) -> anyhow::Result<String> {
        let target = match self {
            Target::Msde { .. } => "/msde-vm-dev",
            Target::Bot { .. } => "/bot-vm-dev",
            Target::Web3 { .. } => "/web3-vm-dev",
            Target::Compiler { .. } => "/compiler-vm-dev",
        };
        let containers = running_containers(docker).await?;
        let container_id = containers
            .get(target)
            .context("Target container is not running")?;
        Ok(container_id.clone())
    }

    pub fn container_name(&self) -> Option<&str> {
        match self {
            Target::Msde { .. } => Some("msde-vm-dev"),
            Target::Bot { .. } => Some("bot-vm-dev"),
            Target::Web3 { .. } => None,
            Target::Compiler { .. } => Some("compiler-vm-dev"),
        }
    }

    pub fn container_remote_console_path(&self) -> Option<&str> {
        match self {
            Target::Msde { .. } => Some("/usr/local/bin/merigo/msde/bin/msde"),
            Target::Bot { .. } => Some("/usr/local/bin/merigo/bot/bin/bot"),
            Target::Web3 { .. } => None,
            Target::Compiler { .. } => Some("usr/local/bin/merigo/compiler/bin/compiler"),
        }
    }

    pub fn images_and_tags(&self) -> Vec<(String, String)> {
        match self {
            Target::Msde { version } | Target::Bot { version } | Target::Compiler { version } => {
                let tag = match version {
                    Some(version) => format!("{self}-vm-dev-docker-{version}"),
                    None => "latest".to_owned(),
                };
                tracing::trace!(%tag, "assembled tag is");

                vec![(
                    format!("docker.pkg.github.com/merigo-co/merigo_dev_packages/{self}-vm-dev"),
                    tag,
                )]
            }
            Target::Web3 { version, .. } => {
                let tag = match version {
                    Some(version) => version.to_string(),
                    None => LATEST.to_owned(),
                };
                tracing::trace!(%tag, "assembled tag is");

                vec![
                    (
                        "docker.pkg.github.com/merigo-co/web3_services/web3_services_dev"
                            .to_string(),
                        tag.clone(),
                    ),
                    (
                        "docker.pkg.github.com/merigo-co/web3_services/web3_consumer_dev"
                            .to_string(),
                        tag,
                    ),
                ]
            }
        }
    }
}

// FIXME: These just discard the version information.. not really intuitive
impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let repr = match self {
            Target::Msde { .. } => "msde",
            Target::Bot { .. } => "bot",
            Target::Web3 { .. } => "web3",
            Target::Compiler { .. } => "compiler",
        };

        write!(f, "{repr}")
    }
}

impl AsRef<str> for Target {
    fn as_ref(&self) -> &str {
        match self {
            Target::Msde { .. } => "msde",
            Target::Bot { .. } => "bot",
            Target::Web3 { .. } => "web3",
            Target::Compiler { .. } => "compiler",
        }
    }
}
