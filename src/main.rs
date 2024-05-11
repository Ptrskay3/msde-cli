use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use clap::ValueEnum;
use clap::{ArgAction, Parser, Subcommand};
use clap_complete::{generate, shells::Shell};
// May work better!
// https://github.com/fussybeaver/bollard
use dialoguer::Input;
use docker_api::opts::ContainerListOpts;
use docker_api::opts::ContainerStopOpts;
use docker_api::Docker;
use flate2::bufread::GzDecoder;
use futures::StreamExt;
use indicatif::ProgressBar;
use indicatif::ProgressStyle;
use msde_cli::env::PackageLocalConfig;
use msde_cli::init::ensure_valid_project_path;
use secrecy::ExposeSecret;
use secrecy::Secret;
use sysinfo::System;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct MetadataResponse {
    name: String,
    tags: Vec<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ParsedMetadataResponse {
    org: String,
    repository: String,
    tags: Vec<String>,
    parsed_versions: Vec<String>,
    image: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Index {
    valid_until: i64,
    content: Vec<ParsedMetadataResponse>,
}

impl ParsedMetadataResponse {
    fn for_target(&self, target: &Target) -> bool {
        self.image.starts_with(target.as_ref())
    }

    fn contains_version(&self, version: &str) -> bool {
        self.parsed_versions.iter().any(|v| v == version)
    }
}

#[derive(Parser, Debug)]
#[command(version)]
struct Command {
    /// Enables verbose output.
    #[arg(short, long)]
    debug: bool,

    /// Skip building a local cache of the MSDE image registry.
    #[arg(short, long)]
    no_cache: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

impl Command {
    fn should_ignore_credentials(&self) -> bool {
        matches!(
            self.command,
            None | Some(
                Commands::Docs
                    | Commands::Status
                    | Commands::AddProfile { .. }
                    | Commands::SetProject { .. }
                    | Commands::GenerateCompletions { .. }
                    | Commands::UpgradeProject { .. }
                    | Commands::Clean { .. }
                    | Commands::Init { .. }
                    | Commands::BuildCache { .. }
                    | Commands::Login { .. }
                    | Commands::Containers { .. }
                    | Commands::Exec { .. }
                    | Commands::UpdateBeamFiles { .. }
                    | Commands::VerifyBeamFiles { .. }
            )
        )
    }
}

const LATEST: &str = "latest";
const USER: &str = "merigo-client";
const DEFAULT_DURATION: i64 = 12;

const REPOS_AND_IMAGES: &[&str; 5] = &[
    "merigo_dev_packages/compiler-vm-dev",
    "merigo_dev_packages/msde-vm-dev",
    "merigo_dev_packages/bot-vm-dev",
    "web3_services/web3_services_dev",
    "web3_services/web3_consumer_dev",
];

#[derive(Debug, Clone)]
struct ListedContainer {
    id: String,
    names: Option<Vec<String>>,
    image: String,
}

#[derive(serde::Deserialize, Clone)]
struct SecretCredentials {
    ghcr_key: Secret<String>,
    pull_key: Secret<String>,
}

#[derive(serde::Serialize)]
struct UnsafeCredentials {
    ghcr_key: String,
    pull_key: String,
}

fn login(
    context: &msde_cli::env::Context,
    ghcr_key: Option<String>,
    pull_key: Option<String>,
    file: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    if let Some(path_buf) = file {
        // TODO: Maybe open it, and check whether this file makes sense?
        std::fs::copy(path_buf, context.config_dir.join("credentials.json"))?;
    } else {
        let ghcr_key = ghcr_key.context("ghrc-key is required")?;
        let pull_key = pull_key.context("pull-key is required")?;
        let credentials = UnsafeCredentials { ghcr_key, pull_key };
        let file = File::create(context.config_dir.join("credentials.json"))?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, &credentials)?;
        writer.flush()?;
    }
    tracing::info!(
        "stored *unencrypted* credentials in `{:?}`",
        context.config_dir.join("credentials.json")
    );
    Ok(())
}

fn try_login(ctx: &msde_cli::env::Context) -> anyhow::Result<SecretCredentials> {
    let f = std::fs::read_to_string(ctx.config_dir.join("credentials.json"))?;
    let credentials: SecretCredentials =
        serde_json::from_str(&f).context("invalid credentials file")?;
    Ok(credentials)
}

async fn create_index(
    client: &reqwest::Client,
    duration: i64,
    credentials: SecretCredentials,
) -> anyhow::Result<()> {
    let version_re = regex::Regex::new(r"\d+\.\d+\.\d+$").unwrap();
    // TODO: Take the config struct into account.
    std::fs::create_dir_all(".msde").context("Failed to create cache directory")?;

    let key = credentials.ghcr_key.expose_secret();
    let registry_requests = REPOS_AND_IMAGES.iter().map(|repo_and_image| {
        let client = &client;
        async move {
            let url = format!("https://ghcr.io/v2/merigo-co/{repo_and_image}/tags/list?n=1000");
            client
                .get(&url)
                .bearer_auth(key)
                .send()
                .await?
                // TODO: the error response is not considered here. That can happen when you don't have sufficient permissions.
                .json::<MetadataResponse>()
                .await
        }
    });

    let responses = futures::future::try_join_all(registry_requests).await?;

    let content = responses
        .into_iter()
        .map(|metadata| {
            let parsed_versions = metadata
                .tags
                .iter()
                .filter_map(|tag| {
                    version_re
                        .captures(tag)
                        .and_then(|cap| cap.get(0).map(|m| m.as_str().to_owned()))
                })
                .collect::<Vec<_>>();

            tracing::trace!(name = %metadata.name, numbered_versions = ?parsed_versions.len(), "indexing done");
            let (org, rest) =  metadata.name.split_once('/').unwrap();
            let (repository, image) =  rest.split_once('/').unwrap();
            ParsedMetadataResponse {
                org: org.to_owned(),
                repository: repository.to_owned(),
                tags: metadata.tags,
                parsed_versions,
                image: image.to_owned(),
            }
        })
        .collect::<Vec<_>>();

    let index = Index {
        valid_until: (time::OffsetDateTime::now_utc() + time::Duration::hours(duration))
            .unix_timestamp(),
        content,
    };

    let file = File::create(".msde/index.json")?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, &index)?;
    writer.flush()?;
    tracing::debug!("local cache built");
    Ok(())
}

#[cfg(debug_assertions)]
static LOGLEVEL: &str = "msde_cli=trace";

#[cfg(not(debug_assertions))]
static LOGLEVEL: &str = "msde_cli=debug"; // TODO: this should be info level probably

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| LOGLEVEL.into()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .without_time()
                .with_target(false),
        )
        .init();
    let theme = dialoguer::theme::ColorfulTheme {
        checked_item_prefix: console::style("  [x]".to_string()).for_stderr().green(),
        unchecked_item_prefix: console::style("  [ ]".to_string()).for_stderr().dim(),
        active_item_style: console::Style::new().for_stderr().cyan().bold(),
        ..dialoguer::theme::ColorfulTheme::default()
    };

    let current_shell = Shell::from_env().unwrap_or(Shell::Bash);
    let mut ctx = msde_cli::env::Context::from_env()?;
    tracing::trace!(?ctx, "context");

    let cmd = Command::parse();
    let self_version = <Command as clap::CommandFactory>::command()
        .get_version()
        .map(|s| semver::Version::parse(s).unwrap())
        .unwrap();

    if !matches!(
        &cmd.command,
        // TODO: don't run this on some other commands. Probably refactor this whole block..
        Some(Commands::Init { .. } | Commands::UpgradeProject { .. })
    ) {
        match (ctx.msde_dir.as_ref(), std::env::var("MERIGO_NOWARN_INIT")) {
            (Some(msde_dir), _) => {
                tracing::info!(path = %msde_dir.display(), "Active project is at");
                if let Err(e) = &ctx.run_project_checks(self_version.clone()) {
                    tracing::warn!(error = %e, "project is invalid"); // TODO: suggest upgrade if version mismatch.. probably thiserror is required here
                }
            }
            (None, Ok(_)) => {}
            (None, _) => {
                // TODO: If we generate completions, it's very confusing still to see this output on stderr, since it may hide the sudo password prompt..
                tracing::warn!("The developer package is not yet configured.");
                tracing::warn!("To configure, you may use the `init` or `set-project` command, or set the project path to the `MERIGO_DEV_PACKAGE_DIR` environment variable.");
                tracing::warn!("You may also install auto-completions by running:");
                if let Some(completion_path) = completions_path(current_shell) {
                    tracing::warn!(
                        "`msde-cli generate-completions | sudo tee {} > /dev/null`",
                        completion_path
                    );
                } else {
                    tracing::warn!("`msde-cli generate-completions`, then redirect its output to your shell's completion path.");
                }
            }
        }
    }

    tracing::trace!(?cmd, "arguments parsed");
    tracing::trace!("attempting to connect to Docker daemon..");
    let docker = new_docker()?;
    msde_cli::init::ensure_docker(&docker).await?;
    tracing::trace!("connected");
    let client = reqwest::Client::new();

    if !&cmd.no_cache {
        match (
            cmd.should_ignore_credentials(),
            std::fs::File::open(".msde/index.json"),
        ) {
            (true, _) => {}
            (_, Ok(content)) => {
                let reader = BufReader::new(content);
                let index: Index = serde_json::from_reader(reader)?;

                if time::OffsetDateTime::now_utc().unix_timestamp() > index.valid_until {
                    tracing::debug!("image registry cache is too old, rebuilding now.");
                    let credentials = try_login(&ctx)
                        .context("No credentials found, run `msde_cli login` first.")?;
                    create_index(&client, DEFAULT_DURATION, credentials).await?;
                }
            }
            (_, Err(_)) => {
                tracing::debug!("image registry cache is not built, building now.");
                let credentials =
                    try_login(&ctx).context("No credentials found, run `msde_cli login` first.")?;
                create_index(&client, DEFAULT_DURATION, credentials).await?;
            }
        }
    }

    match cmd.command {
        Some(Commands::UpdateBeamFiles {
            version, no_verify, ..
        }) => {
            // TODO: Do not hardcode 3.10.0
            let version = version.unwrap_or_else(|| semver::Version::parse("3.10.0").unwrap());

            msde_cli::updater::update_beam_files(version.clone(), no_verify).await?;
            tracing::info!("BEAM files updated to version `{version}`.");
        }
        Some(Commands::VerifyBeamFiles { version, path }) => {
            // TODO: Do not hardcode these values
            let version = version.unwrap_or_else(|| semver::Version::parse("3.10.0").unwrap());
            let path = path.unwrap_or_else(|| {
                PathBuf::from(
                    "/home/leehpeter-zengo/work/merigo/docker_dev/package/merigo_extension/priv",
                )
            });
            msde_cli::updater::verify_beam_files(version, path)?;
            tracing::info!("BEAM files verified.");
        }
        Some(Commands::Exec { .. }) => {
            todo!();
        }
        Some(Commands::Versions { target }) => {
            let file = File::open(".msde/index.json")
                .context("local cache not found, please omit the `--no-build-cache` flag")?;
            let reader = BufReader::new(file);
            let index: Index = serde_json::from_reader(reader)?;

            let entry = index
                .content
                .iter()
                .find(|metadata| metadata.for_target(&target))
                .unwrap();

            println!(
                "available versions for `{}` are {:?}",
                target, entry.parsed_versions
            );
        }
        Some(Commands::BuildCache { duration }) => {
            let credentials =
                try_login(&ctx).context("No credentials found, run `msde_cli login` first.")?;
            create_index(&client, duration.unwrap_or(DEFAULT_DURATION), credentials).await?
        }
        Some(Commands::Containers { always_yes }) => {
            let opts = ContainerListOpts::builder().all(true).build();
            let containers = docker.containers().list(&opts).await?;
            let running: Vec<_> = containers
                .into_iter()
                .filter_map(|container| {
                    if container.state? == "running" {
                        let names = container.names;
                        let id = container.id.unwrap_or_default();
                        Some(ListedContainer {
                            names,
                            id,
                            image: container.image.unwrap_or_default(),
                        })
                    } else {
                        None
                    }
                })
                .collect();

            let running_length = running.len();
            if running_length > 0 {
                println!(
                    "There are {} containers running.. These are",
                    running_length
                );
                for container in &running {
                    println!("id: {} | image: {}", container.id, container.image);
                }

                println!("To update, these containers must be stopped.");

                let should_exit = if !always_yes {
                    !handle_yes_no_prompt()
                } else {
                    false
                };

                if should_exit {
                    println!("exiting");
                    return Ok(());
                }

                println!("Stopping all running containers..");
                let opts = ContainerStopOpts::default();

                let tasks = running.into_iter().map(|container| async {
                    match docker.containers().get(container.id).stop(&opts).await {
                        Ok(()) => {
                            let name = container.names.unwrap_or_default();
                            println!("Container {:?} stopped...", name);
                            Ok(())
                        }
                        Err(e) => {
                            eprintln!("Error: {e}");
                            Err(e)
                        }
                    }
                });
                futures::future::try_join_all(tasks)
                    .await
                    .context("Something went wrong stopping a container.. The errors should be logged to the console.")?;

                println!("All containers stopped successfully.");
            }

            println!("There shouldn't be any running containers now.");
        }
        Some(Commands::Pull { target, version }) => {
            let credentials =
                try_login(&ctx).context("No credentials found, run `msde_cli login` first.")?;

            let targets = target.map(|t| vec![t]).unwrap_or_else(|| {
                vec![
                    Target::Msde {
                        version: version.clone(),
                    },
                    Target::Compiler {
                        version: version.clone(),
                    },
                    Target::Bot {
                        version: version.clone(),
                    },
                    Target::Web3 {
                        version,
                        kind: Some(Web3Kind::All),
                    },
                ]
            });
            if !&cmd.no_cache {
                target_version_check(&targets)?;
            }
            let m = indicatif::MultiProgress::new();
            let mut tasks = vec![];
            for (image, tag) in get_images_and_tags(&targets) {
                let pb = m.add(progress_bar());

                tasks.push(pull(&docker, (image, tag), &credentials, pb));
            }
            let outcome = futures::future::try_join_all(tasks).await.map_err(|e| {
                m.clear().unwrap();
                e
            })?;
            m.clear().unwrap();
            if outcome.iter().all(|x| *x) {
                tracing::info!("All targets pulled!")
            } else {
                tracing::error!("Error pulling some of the images. Check errors above.");
                std::process::exit(-1);
            }
        }
        Some(Commands::Login {
            ghcr_key,
            pull_key,
            file,
        }) => {
            login(&ctx, ghcr_key, pull_key, file)?;
        }
        None => {
            tracing::trace!("No subcommand was passed, starting diagnostic..");
            let version_re = regex::Regex::new(r"\d+\.\d+\.\d+$").unwrap();

            let mut sys = System::new_all();

            sys.refresh_all();

            println!("System:");
            println!("total memory  : {} bytes", sys.total_memory());
            println!("used memory   : {} bytes", sys.used_memory());
            println!("total swap    : {} bytes", sys.total_swap());
            println!("used swap     : {} bytes", sys.used_swap());
            #[cfg(not(target_arch = "aarch64"))]
            {
                use raw_cpuid::CpuId;
                let cpuid = CpuId::new();
                if let Some(cpu_info) = cpuid.get_processor_brand_string() {
                    println!("CPU           : {}", cpu_info.as_str());
                }
            }
            println!("system name   : {}", sysinfo::System::name().unwrap());
            println!(
                "kernel version: {}",
                sysinfo::System::kernel_version().unwrap()
            );
            println!(
                "OS version    : {}",
                sysinfo::System::long_os_version().unwrap()
            );
            println!("host name     : {}", sysinfo::System::host_name().unwrap());
            println!("CPU arch      : {}", sysinfo::System::cpu_arch().unwrap());
            println!(
                "Docker version: {}",
                docker.version().await.unwrap().version.unwrap()
            );

            let opts = docker_api::opts::ImageListOpts::default();
            let docker_images = docker.images().list(&opts).await?;
            let mut local_image_stats: HashMap<String, Vec<String>> = HashMap::new();

            for docker_image in &docker_images {
                if docker_image
                    .repo_tags
                    .iter()
                    .any(|tag| REPOS_AND_IMAGES.iter().any(|im| tag.contains(im)))
                {
                    tracing::trace!(image = ?docker_image.repo_tags, "Looking at image..");
                    let image = docker
                        .images()
                        .get(&docker_image.id)
                        .inspect()
                        .await
                        .unwrap();
                    let image_tags = image.repo_tags.unwrap_or_default();

                    let version: Option<VersionedImage> =
                        image_tags.iter().fold(None, |img, tag| {
                            let result = if tag.ends_with(LATEST) {
                                let (name, original_tag) =
                                    tag.split_once(':').expect("a valid Docker image name");
                                Some((LATEST, name, original_tag))
                            } else if let Some(cap) = version_re.captures(tag) {
                                if let Some(version) = cap.get(0).map(|m| m.as_str()) {
                                    let (name, original_tag) =
                                        tag.split_once(':').expect("a valid Docker image name");
                                    Some((version, name, original_tag))
                                } else {
                                    None
                                }
                            } else {
                                None
                            };

                            let Some((version, name, original_tag)) = result else {
                                return img;
                            };
                            match img {
                                Some(mut a) => {
                                    a.aliases.push(name);
                                    Some(a)
                                }
                                None => Some(VersionedImage {
                                    version,
                                    name,
                                    aliases: vec![],
                                    id: docker_image.id.clone(),
                                    original_tag,
                                    resolved_version: semver::Version::parse(version).ok(),
                                }),
                            }
                        });

                    if let Some(version_info) = version {
                        tracing::trace!(
                            name = ?version_info.name,
                            version = ?version_info.version,
                            resolved_version = ?version_info.resolved_version,
                            original_tag = ?version_info.original_tag,
                            aliases = ?version_info.aliases,
                            "parsed local image"
                        );

                        local_image_stats
                            .entry(version_info.name.to_owned())
                            .or_default()
                            .push(version_info.version.to_owned());
                    }
                }
            }
            println!("Available local Merigo related images are:\n{local_image_stats:#?}");
        }
        Some(Commands::Clean { always_yes }) => {
            // TODO: Also remove the msde_dir if set
            println!("About to remove {:?}", ctx.config_dir);

            let proceed = if always_yes {
                true
            } else {
                dialoguer::Confirm::with_theme(&theme)
                    .with_prompt("This is an irreversible action. Are you sure to continue?")
                    .wait_for_newline(true)
                    .default(false)
                    .show_default(true)
                    .report(true)
                    .interact()?
            };
            if proceed {
                msde_cli::env::Context::clean(&ctx);
            }
        }
        Some(Commands::Up {}) => {
            msde_cli::compose::Compose::up_builtin(None)?;
        }
        Some(Commands::Init { path, force }) => {
            // TODO: integrate login, integrate BEAM file stuff.
            // Prompt whether example games should be included
            // Message to put their existing games inside a folder..
            let target = path.unwrap_or_else(|| {
                let res: String = Input::with_theme(&theme)
                    .with_prompt("Where should the project be initialized?\nInput a directory, or press enter to accept the default.")
                    .default(ctx.home.join("merigo").to_string_lossy().into_owned())
                    .interact_text()
                    .unwrap();
                PathBuf::from(res)
            });
            msde_cli::init::ensure_valid_project_path(&target, force)?;
            ctx.set_project_path(&target);
            let mut archive = tar::Archive::new(GzDecoder::new(msde_cli::PACKAGE));
            archive.unpack(&target).with_context(|| {
                format!(
                    "Failed to initialize MSDE at directory `{}`",
                    target.display()
                )
            })?;
            ctx.write_config(target.canonicalize().unwrap())?;
            ctx.write_package_local_config(self_version)?;
            tracing::info!(path = %target.display(), "Successfully initialized project at");
        }
        Some(Commands::UpgradeProject { path }) => {
            // Plan:
            // 1. Obtain the project path, and find metadata.json
            let project_path = path
                .or_else(|| ctx.explicit_project_path().cloned())
                .unwrap_or_else(|| {
                    let p = Input::with_theme(&theme)
                        .with_prompt("Where is the project located?")
                        .default(
                            msde_cli::env::home()
                                .unwrap()
                                .join("merigo")
                                .to_string_lossy()
                                .into_owned(),
                        )
                        .interact()
                        .unwrap();
                    PathBuf::from(p)
                });
            // TODO: These checks are already implemented elsewhere.
            tracing::debug!(path = %project_path.display(), "Upgrade project at");
            let config = project_path.join("metadata.json");
            let f = File::open(config)
                .context("metadata.json file is missing. Please rerun `msde_cli init`.")?;
            let reader = BufReader::new(f);
            let PackageLocalConfig {
                self_version: project_self_version,
                ..
            } = serde_json::from_reader(reader)
                .context("metadata.json file is invalid. Please rerun `msde_cli init`.")?;
            // 2. Compare the current self_version and the metadata's version.
            let project_self_version = semver::Version::parse(&project_self_version).unwrap();
            println!(
                "project self version {project_self_version:?} | self version {self_version:?}"
            );
            // 3. Lookup the migration matrix function (which is TBD.).
            // 4. Write the changes to disk, or display migration steps that need to be done manually.
            // 5. Update the metadata.json.
            // 6. Optionally display a warning message if the current project is not using the right self_version.
            tracing::info!("Automatic update done.");
            todo!();
        }
        Some(Commands::GenerateCompletions { shell }) => {
            generate(
                shell.unwrap_or(current_shell),
                &mut <Command as clap::CommandFactory>::command(),
                "msde-cli",
                &mut std::io::stdout(),
            );
        }
        Some(Commands::AddProfile { name, features }) => {
            ctx.write_profiles(name, features)
                .context("Failed to write profile.")?;
        }
        Some(Commands::SetProject { path }) => {
            let path = path.unwrap_or_else(|| {
                let p = Input::<'_, String>::with_theme(&theme)
                    .with_prompt("Where is the project located?")
                    .interact()
                    .unwrap();
                PathBuf::from(p)
            });
            ensure_valid_project_path(&path, true)
                .context("Project directory seems to be invalid")?;
            ctx.set_project_path(&path);
            ctx.run_project_checks(self_version)?;
            ctx.write_config(path)?;
        }
        Some(Commands::Status) => {
            // TODO: A lot of things here.
            println!("Merigo developer package version {self_version}");
        }
        Some(Commands::Docs) => {
            webbrowser::open("https://docs.merigo.co/getting-started/devpackage")
                .context("failed to open a browser")?;
        }
        _ => tracing::debug!("not now.."),
    }

    Ok(())
}

fn completions_path(shell: Shell) -> Option<&'static str> {
    match shell {
        Shell::Bash => Some("/usr/share/bash-completion/completions/msde-cli.bash"),
        Shell::Fish => Some("/usr/share/fish/vendor_completions.d/msde-cli.fish"),
        Shell::Zsh => Some("/usr/share/zsh/site-functions/_msde-cli"),
        // FIXME: not sure about others.
        _ => None,
    }
}

#[derive(Debug)]
struct VersionedImage<'v> {
    version: &'v str,
    resolved_version: Option<semver::Version>,
    name: &'v str,
    original_tag: &'v str,
    aliases: Vec<&'v str>,
    #[allow(dead_code)]
    id: String,
}

#[cfg(unix)]
pub fn new_docker() -> docker_api::Result<Docker> {
    Ok(Docker::unix("/var/run/docker.sock"))
}

#[cfg(not(unix))]
pub fn new_docker() -> docker_api::Result<Docker> {
    Docker::new("tcp://127.0.0.1:2375")
}

#[derive(Subcommand, Debug)]
enum Commands {
    Docs,
    Status,
    SetProject {
        #[arg(index = 1)]
        path: Option<PathBuf>,
    },
    AddProfile {
        #[arg(short, long)]
        name: String,
        #[arg(short, long, value_delimiter = ' ', num_args = 1..)]
        features: Vec<msde_cli::env::Feature>,
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
    UpgradeProject {
        #[arg(short, long)]
        path: Option<std::path::PathBuf>,
    },
    Up {},
    /// Wipe out all config files and folders.
    Clean {
        /// Continue without asking for further confirmation.
        #[arg(short = 'y', long, action = ArgAction::SetTrue)]
        always_yes: bool,
    },
    /// Runs the target service(s), then attaches to its logs
    Run {
        #[command(subcommand)]
        target: Option<Target>,
    },
    Stop,
    Start,
    Down,
    /// Open the logs of the target service.
    Log {
        #[command(subcommand)]
        target: Target,
    },
    /// Pull the latest docker image of the target service(s).
    Pull {
        #[command(subcommand)]
        target: Option<Target>,

        // the "version" argument in the other subcommand (kind of confusing)
        #[arg(short, long, required_unless_present = "version")]
        version: Option<String>,
    },
    Ssh,
    Shell,
    /// Initialize the MSDE developer package.
    ///
    /// This command will not delete any files, but will override anything in the target directory if the package content
    /// conflicts with an existing file.
    /// For that exact reason a non-empty directory will be rejected, unless the --force flag is present.
    Init {
        #[arg(short, long)]
        path: Option<std::path::PathBuf>,

        #[arg(long, action = ArgAction::SetTrue)]
        force: bool,
    },
    /// Run a command in the target service.
    Exec {
        cmd: String,
    },
    /// Verify the integrity of BEAM files.
    VerifyBeamFiles {
        #[arg(short, long)]
        version: Option<semver::Version>,

        #[arg(short, long)]
        path: Option<std::path::PathBuf>,
    },
    /// Update the BEAM files.
    UpdateBeamFiles {
        #[arg(short, long)]
        version: Option<semver::Version>,

        #[arg(short, long)]
        path: Option<std::path::PathBuf>,

        #[arg(long, action = ArgAction::SetTrue)]
        no_verify: bool,
    },
    /// Checks and stops all running containers.
    Containers {
        #[arg(short = 'y', long, action = ArgAction::SetTrue)]
        always_yes: bool,
    },
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
    Login {
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
enum Target {
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
enum Web3Kind {
    All,
    Consumer,
    Producer,
}

impl Target {
    fn get_version(&self) -> Option<&String> {
        match self {
            Target::Msde { version }
            | Target::Bot { version }
            | Target::Web3 { version, .. }
            | Target::Compiler { version } => version.as_ref(),
        }
    }

    fn images_and_tags(&self) -> Vec<(String, String)> {
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

fn handle_yes_no_prompt() -> bool {
    loop {
        println!("Are you sure to continue? [Y/n]");
        let mut answer = String::new();

        std::io::stdin()
            .read_line(&mut answer)
            .expect("Failed to read line");

        match answer.to_ascii_lowercase().trim() {
            "y" | "yes" => break true,
            "n" | "no" => break false,
            _ => {}
        };
    }
}

#[tracing::instrument(skip(docker, credentials, pb))]
async fn pull(
    docker: &Docker,
    (image, tag): (String, String),
    credentials: &SecretCredentials,
    pb: ProgressBar,
) -> anyhow::Result<bool> {
    let mut errored = false;
    let opts = docker_api::opts::PullOpts::builder()
        .image(&image)
        .tag(&tag)
        .auth(
            docker_api::opts::RegistryAuth::builder()
                .username(USER)
                .password(credentials.pull_key.expose_secret())
                .build(),
        )
        .build();

    let images = docker.images();
    let mut stream = images.pull(&opts);

    pb.set_message(format!("Pulling image {}:{}", &image, &tag));
    while let Some(pull_result) = stream.next().await {
        match pull_result {
            Ok(output) => match output {
                docker_api::models::ImageBuildChunk::Error {
                    error,
                    error_detail,
                } => {
                    pb.suspend(|| {
                        tracing::error!(err = ?error, detail = ?error_detail, "Error occurred");
                    });
                    errored = true;
                    pb.finish_with_message("Error pulling image. Errors should be logged above.");
                    break;
                }

                docker_api::models::ImageBuildChunk::PullStatus { .. } => {
                    pb.inc(1);
                }
                _ => {}
            },
            Err(e) => {
                pb.suspend(|| tracing::error!(err = ?e, "Error occurred"));
                errored = true;
                pb.finish_with_message("Error pulling image. Errors should be logged above.");
                break;
            }
        }
    }

    if !errored {
        pb.finish_with_message("Done.");
        return Ok(true);
    }

    Ok(false)
}

fn get_images_and_tags(targets: &[Target]) -> Vec<(String, String)> {
    targets.iter().fold(vec![], |mut acc, target| {
        acc.extend(target.images_and_tags());
        acc
    })
}

fn progress_bar() -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(80));
    pb.set_style(
        ProgressStyle::with_template("{spinner:.blue} {elapsed:3} {msg}")
            .unwrap()
            .tick_strings(&[
                "[    ]", "[=   ]", "[==  ]", "[=== ]", "[====]", "[ ===]", "[  ==]", "[   =]",
                "[    ]", "[   =]", "[  ==]", "[ ===]", "[====]", "[=== ]", "[==  ]", "[=   ]",
            ]),
    );
    pb
}

fn target_version_check(targets: &[Target]) -> anyhow::Result<()> {
    // TODO: Paths
    let file = File::open(".msde/index.json")?;
    let reader = BufReader::new(file);
    let index: Index = serde_json::from_reader(reader)?;
    for target in targets {
        let version = target.get_version();
        if let Some(version) = version {
            let entry = index
                .content
                .iter()
                .find(|metadata| metadata.for_target(&target))
                .unwrap();
            if !entry.contains_version(version) {
                tracing::warn!(%target, %version, available_versions = ?entry.parsed_versions.iter(), "Specified unknown version for target");
            }
        }
    }
    Ok(())
}
