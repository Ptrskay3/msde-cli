use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Write;

use anyhow::Context;
use clap::ValueEnum;
use clap::{ArgAction, Parser, Subcommand};
use docker_api::opts::ContainerListOpts;
use docker_api::opts::ContainerStopOpts;
use docker_api::Docker;
use futures::StreamExt;
use secrecy::ExposeSecret;
use secrecy::Secret;
use sysinfo::{System, SystemExt};
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
struct Command {
    #[arg(short, long)]
    debug: bool,

    #[arg(short, long)]
    no_build_cache: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

impl Command {
    fn should_ignore_credentials(&self) -> bool {
        matches!(
            self.command,
            None | Some(Commands::BuildCache { .. })
                | Some(Commands::Login { .. })
                | Some(Commands::Containers { .. })
        )
    }
}

const LATEST: &str = "latest";
const CLEAR: &str = "\x1B[2J\x1B[1;1H";
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

#[derive(serde::Deserialize)]
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
    ghcr_key: Option<String>,
    pull_key: Option<String>,
    file: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(".msde").context("Failed to create cache directory")?;
    if let Some(path_buf) = file {
        // TODO: Maybe open it, and check whether this file makes sense?
        std::fs::copy(path_buf, ".msde/credentials.json")?;
    } else {
        let ghcr_key = ghcr_key.context("ghrc-key is required")?;
        let pull_key = pull_key.context("pull-key is required")?;
        let credentials = UnsafeCredentials { ghcr_key, pull_key };
        let file = File::create(".msde/credentials.json")?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, &credentials)?;
        writer.flush()?;
    }
    tracing::warn!("stored *unencrypted* credentials to `.msde/credentials.json`");
    Ok(())
}

fn try_login() -> anyhow::Result<SecretCredentials> {
    let file = std::fs::File::open(".msde/credentials.json")?;
    let reader = BufReader::new(file);
    let credentials: SecretCredentials = serde_json::from_reader(reader)?;
    Ok(credentials)
}

async fn create_index(
    client: &reqwest::Client,
    duration: i64,
    credentials: SecretCredentials,
) -> anyhow::Result<()> {
    let version_re = regex::Regex::new(r"\d+\.\d+\.\d+$").unwrap();

    std::fs::create_dir_all(".msde").context("Failed to create cache directory")?;

    let registry_requests = REPOS_AND_IMAGES.iter().map(|repo_and_image| {
        let client = &client;
        let key = credentials.ghcr_key.expose_secret();
        async move {
            let url = format!("https://ghcr.io/v2/merigo-co/{repo_and_image}/tags/list?n=1000");
            client
                .get(&url)
                .bearer_auth(key)
                .send()
                .await?
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
    tracing::trace!("local cache built");
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "msde_cli=trace".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::trace!("starting CLI..");
    let cmd = Command::parse();
    tracing::trace!(?cmd, "arguments parsed");
    tracing::trace!("attempting to connect to Docker daemon..");
    let docker = new_docker()?;
    tracing::trace!("connected");
    let client = reqwest::Client::new();

    if !&cmd.no_build_cache {
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
                    let credentials =
                        try_login().context("No credentials found, run `msde-cli login` first.")?;
                    create_index(&client, DEFAULT_DURATION, credentials).await?;
                }
            }
            (_, Err(_)) => {
                tracing::debug!("image registry cache is not built, building now.");
                let credentials =
                    try_login().context("No credentials found, run `msde-cli login` first.")?;
                create_index(&client, DEFAULT_DURATION, credentials).await?;
            }
        }
    }

    match cmd.command {
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
                try_login().context("No credentials found, run `msde-cli login` first.")?;
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
                        Ok(_) => {
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

            println!("There shouldn't be any running containers now, let's roll");
        }
        Some(Commands::Pull { target }) => {
            let credentials =
                try_login().context("No credentials found, run `msde-cli login` first.")?;
            pull(&docker, &target, cmd.no_build_cache, credentials).await?;
        }
        Some(Commands::Login {
            ghcr_key,
            pull_key,
            file,
        }) => {
            login(ghcr_key, pull_key, file)?;
        }
        None => {
            tracing::trace!("No subcommand was passed, starting diagnostic..");
            let version_re = regex::Regex::new(r"\d+\.\d+\.\d+$").unwrap();

            use raw_cpuid::CpuId;
            let cpuid = CpuId::new();
            let mut sys = System::new_all();

            sys.refresh_all();

            println!("System:");
            println!("total memory  : {} bytes", sys.total_memory());
            println!("used memory   : {} bytes", sys.used_memory());
            println!("total swap    : {} bytes", sys.total_swap());
            println!("used swap     : {} bytes", sys.used_swap());
            if let Some(cpu_info) = cpuid.get_processor_brand_string() {
                println!("CPU           : {}", cpu_info.as_str());
            }

            println!("system name   : {}", sys.name().unwrap());
            println!("kernel version: {}", sys.kernel_version().unwrap());
            println!("OS version    : {}", sys.os_version().unwrap());
            println!("host name     : {}", sys.host_name().unwrap());
            println!(
                "Docker version: {}",
                docker.version().await.unwrap().version.unwrap()
            );

            let opts = docker_api::opts::ImageListOpts::default();
            let docker_images = docker.images().list(&opts).await?;
            let mut local_image_stats: HashMap<String, Vec<String>> = HashMap::new();

            for docker_image in docker_images.iter() {
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
        _ => tracing::debug!("not now.."),
    }

    Ok(())
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
pub fn new_docker() -> Result<Docker> {
    Docker::new("tcp://127.0.0.1:8080")
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Runs the target service(s), then attaches to its logs
    Run {
        #[command(subcommand)]
        target: Option<Target>,
    },
    Stop,
    Start,
    Down,
    Log {
        #[command(subcommand)]
        target: Target,
    },
    Pull {
        #[command(subcommand)]
        target: Target,
    },
    Ssh,
    Shell,
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
                tracing::debug!(%tag, "assembled tag is");

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
                tracing::debug!(%tag, "assembled tag is");

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

async fn pull(
    docker: &Docker,
    target: &Target,
    no_cache: bool,
    credentials: SecretCredentials,
) -> anyhow::Result<()> {
    tracing::trace!(%target, "attempting to pull an image.. ");
    let version = target.get_version();

    if !no_cache {
        if let Some(version) = version {
            let file = File::open(".msde/index.json")?;
            let reader = BufReader::new(file);
            let index: Index = serde_json::from_reader(reader)?;

            let entry = index
                .content
                .iter()
                .find(|metadata| metadata.for_target(target))
                .unwrap();
            if !entry.contains_version(version) {
                tracing::warn!(%target, %version, available_versions = ?entry.parsed_versions, "Specified unknown version for target");
            }
        }
    }

    let images_and_tags = target.images_and_tags();
    for (image, tag) in images_and_tags {
        let opts = docker_api::opts::PullOpts::builder()
            .image(image)
            .tag(tag)
            .auth(
                docker_api::opts::RegistryAuth::builder()
                    .username(USER)
                    .password(credentials.pull_key.expose_secret())
                    .build(),
            )
            .build();

        let images = docker.images();
        let mut stream = images.pull(&opts);
        while let Some(pull_result) = stream.next().await {
            match pull_result {
                Ok(output) => {
                    if let docker_api::models::ImageBuildChunk::PullStatus {
                        progress,
                        status,
                        ..
                    } = output
                    {
                        if let Some(progress) = progress {
                            println!("{CLEAR}{progress}");
                        } else {
                            println!("{status}");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(err = ?e, "Error occurred");
                    break;
                }
            }
        }
    }

    Ok(())
}
