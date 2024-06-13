use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{BufReader, BufWriter, Write},
    path::PathBuf,
    process::Stdio,
    time::Duration,
};

use anyhow::Context as _;
use clap::Parser;
use clap_complete::{generate, shells::Shell};
use dialoguer::{Confirm, Input};
use docker_api::{
    opts::{ContainerListOpts, ContainerStopOpts},
    Docker,
};
use flate2::bufread::GzDecoder;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
#[cfg(all(feature = "local_auth", debug_assertions))]
use msde_cli::{central_service::MerigoApiClient, local_auth, env::Authorization};
use msde_cli::{
    cli::{Command, Commands, Target, Web3Kind},
    compose::Pipeline,
    env::{Context, Feature},
    game::{
        import_games, PackageConfigEntry, PackageLocalConfig as GamePackageLocalConfig,
        PackageStagesConfig,
    },
    hooks::{execute_all, Hooks},
    init::ensure_valid_project_path,
    utils::{self, resolve_features},
    DEFAULT_DURATION, LATEST, MERIGO_UPSTREAM_VERSION, REPOS_AND_IMAGES, USER,
};

use secrecy::{ExposeSecret, Secret};
use sysinfo::System;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

#[cfg(debug_assertions)]
static LOGLEVEL: &str = "msde_cli=trace";

#[cfg(not(debug_assertions))]
static LOGLEVEL: &str = "msde_cli=info";

type BoxedFuture = std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>>>>;

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

    let upstream_version = semver::Version::parse(MERIGO_UPSTREAM_VERSION).unwrap();

    let current_shell = Shell::from_env().unwrap_or(Shell::Bash);
    let mut ctx = msde_cli::env::Context::from_env()?;
    tracing::trace!(?ctx, "context");

    if let Some(msde_dir) = ctx.msde_dir.as_ref() {
        let docker_compose_env = msde_dir.join("./docker/.env");
        dotenvy::from_path(docker_compose_env).ok();
    }

    let cmd = Command::parse();
    let self_version = <Command as clap::CommandFactory>::command()
        .get_version()
        .map(|s| semver::Version::parse(s).unwrap())
        .unwrap();

    if !matches!(
        &cmd.command,
        // TODO: don't run this on some other commands. Probably refactor this whole block..
        Some(
            Commands::Init { .. }
                | Commands::UpgradeProject { .. }
                | Commands::GenerateCompletions { .. }
        )
    ) {
        match (ctx.msde_dir.as_ref(), std::env::var("MERIGO_NOWARN_INIT")) {
            (Some(msde_dir), _) => {
                tracing::info!(path = %msde_dir.display(), "Active project is at");
                if let Err(e) = &ctx.run_project_checks(self_version.clone()) {
                    tracing::warn!(error = %e, "project is invalid");
                }
            }
            (None, Ok(_)) => {}
            (None, _) => {
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
            std::fs::File::open(&ctx.config_dir.join("index.json")),
        ) {
            (true, _) => {}
            (_, Ok(content)) => {
                let reader = BufReader::new(content);
                let index: Index = serde_json::from_reader(reader)?;

                if time::OffsetDateTime::now_utc().unix_timestamp() > index.valid_until {
                    tracing::debug!("image registry cache is too old, rebuilding now.");
                    let credentials = try_login(&ctx)
                        .context("No credentials found, run `msde_cli login` first.")?;
                    create_index(&ctx, &client, DEFAULT_DURATION, credentials).await?;
                }
            }
            (_, Err(_)) => {
                tracing::debug!("image registry cache is not built, building now.");
                let credentials =
                    try_login(&ctx).context("No credentials found, run `msde_cli login` first.")?;
                create_index(&ctx, &client, DEFAULT_DURATION, credentials).await?;
            }
        }
    }

    match cmd.command {
        Some(Commands::UpdateBeamFiles {
            version, no_verify, ..
        }) => {
            let version = version.unwrap_or(upstream_version);

            msde_cli::updater::update_beam_files(&ctx, version.clone(), no_verify).await?;
            tracing::info!("BEAM files updated to version `{version}`.");
        }
        Some(Commands::VerifyBeamFiles { version, path }) => {
            let version = version.unwrap_or(upstream_version);

            let Some(path) = path.or_else(|| {
                ctx.msde_dir
                    .map(|msde_dir| msde_dir.join("merigo-extension"))
            }) else {
                anyhow::bail!(
                    "No path found to merigo extension. Please specify the --path argument."
                )
            };
            msde_cli::updater::verify_beam_files(version, path)?;
            tracing::info!("BEAM files verified.");
        }
        Some(Commands::Versions { target }) => {
            let file = File::open(&ctx.config_dir.join("index.json"))
                .context("local cache not found, please omit the `--no-cache` flag")?;
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
            create_index(
                &ctx,
                &client,
                duration.unwrap_or(DEFAULT_DURATION),
                credentials,
            )
            .await?
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
                target_version_check(&targets, &ctx)?;
            }
            let m = indicatif::MultiProgress::new();
            let mut tasks = vec![];
            for (image, tag) in get_images_and_tags(&targets) {
                let pb = m.add(progress_bar());

                tasks.push(pull(&docker, (image, tag), Some(&credentials), pb));
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
        Some(Commands::Clean { always_yes }) => {
            // TODO: Also remove the msde_dir if set?
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
        Some(Commands::CreateGame {
            game,
            stage,
            guid,
            suid,
        }) => {
            let Some(msde_dir) = &ctx.msde_dir.as_ref() else {
                anyhow::bail!("project must be set")
            };
            let target = msde_dir.join("games").join(&game).join(&stage);
            if target.exists() {
                anyhow::bail!(format!(
                    "A game with name combination '{game}/{stage}' already exists."
                ))
            }

            let mut archive = tar::Archive::new(GzDecoder::new(msde_cli::TEMPLATE));
            archive.unpack(&target).with_context(|| {
                format!(
                    "Failed to initialize a new game at directory `{}`",
                    target.display()
                )
            })?;

            let stages_path = msde_dir.join("games/stages.yml");
            let stages = std::fs::read_to_string(&stages_path)
                .context("games/stages.yml file doesn't exist, but it should..")?;
            let mut local_cfg = serde_yaml::from_str::<PackageStagesConfig>(&stages)
                .context("Failed to deserialize stages.yml")?;
            let guid = guid.unwrap_or_else(|| {
                if let Some(existing_local_cfg) = local_cfg.try_find_guid_in(&game) {
                    if let Ok(local_config) =
                        std::fs::read_to_string(msde_dir.join("games").join(existing_local_cfg))
                    {
                        let local_cfg =
                            serde_yaml::from_str::<GamePackageLocalConfig>(&local_config)
                                .expect("local_config.yml is invalid");
                        local_cfg.guid
                    } else {
                        Uuid::new_v4()
                    }
                } else {
                    Uuid::new_v4()
                }
            });
            local_cfg.0.push(PackageConfigEntry {
                config: PathBuf::from(format!("{game}/{stage}/local_config.yml")),
                scripts: PathBuf::from(format!("{game}/{stage}/scripts")),
                tuning: PathBuf::from(format!("{game}/{stage}/tuning")),
                disabled: Some(false),
            });
            let cfg = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(stages_path)?;
            let mut writer = BufWriter::new(cfg);
            serde_yaml::to_writer(&mut writer, &local_cfg)?;
            writer.flush()?;

            let local_config_path = target.join("local_config.yml");
            let local_config = std::fs::read_to_string(&local_config_path)?;
            let mut local_cfg = serde_yaml::from_str::<GamePackageLocalConfig>(&local_config)?;
            local_cfg.game.clone_from(&game);
            local_cfg.stage.clone_from(&stage);
            local_cfg.guid = guid;
            local_cfg.suid = suid.unwrap_or_else(Uuid::new_v4);
            let cfg = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(local_config_path)?;
            let mut writer = BufWriter::new(cfg);
            serde_yaml::to_writer(&mut writer, &local_cfg)?;
            writer.flush()?;
        }
        Some(Commands::Up {
            features,
            timeout,
            quiet,
            attach,
            build,
            raw,
            profile,
        }) => {
            let Some(msde_dir) = &ctx.msde_dir.as_ref() else {
                anyhow::bail!("project must be set")
            };
            let Some(metadata) = ctx.run_project_checks(self_version)? else {
                anyhow::bail!("No valid active project found");
            };
            let attach_future = if attach {
                Some(Target::Msde { version: None }.attach(&docker))
            } else {
                None
            };

            let mut features = resolve_features(features, profile, &ctx);

            Pipeline::up_from_features(
                features.as_mut_slice(),
                msde_dir,
                // FIXME: Why `target_msde_version` is an Option? Probably it shouldn't be.
                metadata.target_msde_version.unwrap().to_string().as_str(),
                timeout,
                &docker,
                quiet,
                build,
                attach_future,
                Option::<BoxedFuture>::None,
                raw,
            )
            .await?;
        }
        Some(Commands::Down { timeout }) => {
            let Some(msde_dir) = &ctx.msde_dir.as_ref() else {
                anyhow::bail!("project must be set")
            };
            Pipeline::down_all(&docker, msde_dir, timeout).await?;
        }
        Some(Commands::Stop { timeout }) => {
            let Some(msde_dir) = &ctx.msde_dir.as_ref() else {
                anyhow::bail!("project must be set")
            };
            Pipeline::stop_all(&docker, msde_dir, timeout).await?;
        }
        Some(Commands::RunHooks { pre, post }) => {
            anyhow::ensure!(ctx.msde_dir.is_some(), "project must be set");
            let Some(metadata) = ctx.run_project_checks(self_version)? else {
                anyhow::bail!("No valid active project found");
            };
            if let Some(hooks) = metadata.hooks {
                if pre {
                    execute_all(hooks.pre_run).context("failed to execute pre-run hook")?;
                }
                if post {
                    execute_all(hooks.post_run).context("failed to execute pre-run hook")?;
                }
            }
        }
        Some(Commands::Run {
            features,
            timeout,
            quiet,
            attach,
            build,
            raw,
            no_hooks,
            profile,
        }) => {
            let Some(msde_dir) = &ctx.msde_dir.as_ref() else {
                anyhow::bail!("project must be set")
            };
            let Some(mut metadata) = ctx.run_project_checks(self_version)? else {
                anyhow::bail!("No valid active project found");
            };

            let mut features = resolve_features(features, profile, &ctx);

            let d = docker.clone();
            let attach_future = if attach {
                Some(Target::Msde { version: None }.attach(&d))
            } else {
                None
            };

            if !no_hooks {
                if let Some(hooks) = std::mem::take(&mut metadata.hooks) {
                    execute_all(hooks.pre_run).context("failed to execute pre-run hook")?;

                    metadata.hooks = Some(Hooks {
                        pre_run: Vec::new(),
                        post_run: hooks.post_run,
                    });
                }
            }

            Pipeline::up_from_features(
                features.as_mut_slice(),
                msde_dir,
                metadata.target_msde_version.unwrap().to_string().as_str(),
                timeout,
                &docker,
                quiet,
                build,
                attach_future,
                Some(import_games(&ctx, docker.clone(), quiet || raw || attach)),
                raw,
            )
            .await?;
            if !no_hooks {
                if let Some(hooks) = metadata.hooks {
                    execute_all(hooks.post_run).context("failed to execute post-run hook")?;
                }
            }
        }
        Some(Commands::Init {
            path,
            force,
            pull_images,
            no_pull_images,
            features,
        }) => {
            // TODO: integrate login, integrate BEAM file stuff.
            // Prompt whether example games should be included
            // Message to put their existing games inside a folder..
            let mut target = path.unwrap_or_else(|| {
                let res: String = Input::with_theme(&theme)
                    .with_prompt("Where should the project be initialized?\nInput a directory, or press enter to accept the default.")
                    .default(ctx.home.join("merigo").to_string_lossy().into_owned())
                    .interact_text()
                    .unwrap();
                PathBuf::from(res)
            });

            if utils::wsl()
                && (target.starts_with("/mnt/")
                    || target
                        .canonicalize()
                        .map(|p| p.starts_with("/mnt/"))
                        .unwrap_or(false))
            {
                tracing::warn!("You seem to be using the Windows filesystem.\nIt's highly recommended to use the WSL filesystem, otherwise the package will not work correctly.");
                let res: String = Input::with_theme(&theme)
                    .with_prompt("Input a directory, or press enter to accept the default.")
                    .default(ctx.home.join("merigo").to_string_lossy().into_owned())
                    .interact_text()
                    .unwrap();
                target = PathBuf::from(res);
            }

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
            let should_pull = if pull_images {
                true
            } else if !no_pull_images {
                Confirm::with_theme(&theme)
                    .with_prompt("It's recommended to pull all Docker images to avoid slow cold starts. Do you wish to do it now?")
                    .interact()
                    .unwrap()
            } else {
                false
            };
            tracing::info!(path = %target.display(), "Successfully initialized project at");
            if should_pull {
                let mut images_and_tags = vec![
                    (String::from("postgres"), String::from("13")),
                    (String::from("dpage/pgadmin4"), String::from("latest")),
                    (String::from("hashicorp/consul"), String::from("latest")),
                    (String::from("redis"), String::from("6.2")),
                ];
                let features = features.unwrap_or_else(|| {
                    let selection = dialoguer::MultiSelect::new()
                        .with_prompt("Which features do you wish to use? Use the arrow keys to move, Space to select and Enter to confirm.")
                        // Note: Do not change the order of these, as the ordering corresponds to the `Feature` enum.
                        .items(&["Metrics", "OTEL", "Web3", "Bot"])
                        .defaults(&[true, false, true, false])
                        .interact()
                        .unwrap();
                    selection
                        .into_iter()
                        .flat_map(Feature::from_primitive)
                        .collect::<Vec<Feature>>()
            });

                images_and_tags.extend(
                    features
                        .iter()
                        .flat_map(|feature| feature.required_images_and_tags()),
                );

                let m = indicatif::MultiProgress::new();
                let mut tasks = vec![];
                for (image, tag) in images_and_tags {
                    let pb = m.add(progress_bar());

                    tasks.push(pull(&docker, (image, tag), None, pb));
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
            } else if features.is_some() {
                tracing::warn!("Passing --features without --pull-images has no effect.")
            }
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
            let msde_cli::env::PackageLocalConfig {
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
        Some(Commands::Rpc { cmd }) => {
            let op = msde_cli::game::rpc(docker, cmd).await?;
            println!("{}", msde_cli::game::process_rpc_output(&op));
        }
        Some(Commands::ImportGames { quiet }) => {
            import_games(&ctx, docker, quiet).await?;
        }
        Some(Commands::Log { target }) => {
            target.attach(&docker).await?;
        }
        Some(Commands::Ssh { target }) => {
            let Some(name) = target.container_name() else {
                anyhow::bail!("Invalid target for command")
            };
            let pty = pty_process::blocking::Pty::new()?;
            pty.resize(pty_process::Size::new(1920, 1080))?;
            let mut cmd = pty_process::blocking::Command::new("docker");
            cmd.args(&["exec", "-it", name, "/bin/bash"]);
            cmd.stdin(Stdio::inherit());
            cmd.stdout(Stdio::inherit());
            cmd.stderr(Stdio::inherit());
            let mut child = cmd.spawn(&pty.pts()?)?;
            child.wait()?;
        }
        Some(Commands::Shell { target }) => {
            let (name, remote_console_path) = match (
                target.container_name(),
                target.container_remote_console_path(),
            ) {
                (Some(container_name), Some(remote_console_path)) => {
                    (container_name, remote_console_path)
                }
                _ => anyhow::bail!("Invalid target for command"),
            };
            let pty = pty_process::blocking::Pty::new()?;
            pty.resize(pty_process::Size::new(1920, 1080))?;
            let mut cmd = pty_process::blocking::Command::new("docker");
            cmd.args(&["exec", "-it", name, remote_console_path, "remote_console"]);
            cmd.stdin(Stdio::inherit());
            cmd.stdout(Stdio::inherit());
            cmd.stderr(Stdio::inherit());
            let mut child = cmd.spawn(&pty.pts()?)?;
            child.wait()?;
        }
        #[cfg(all(feature = "local_auth", debug_assertions))]
        Some(Commands::RunAuthServer) => {
            local_auth::run_local_auth_server().await?;
        }
        #[cfg(all(feature = "local_auth", debug_assertions))]
        Some(Commands::Register { name }) => {
            let client = MerigoApiClient::new(
                String::from("http://localhost:8765"),
                None,
                self_version.to_string(),
            );
            let token = client.register(&name).await?;
            println!("Token is {token}");
        }
        #[cfg(all(feature = "local_auth", debug_assertions))]
        Some(Commands::LoginDev { token }) => {
            let client = MerigoApiClient::new(
                String::from("http://localhost:8765"),
                None,
                self_version.to_string(),
            );
            let name = client.login(&token).await?;
            let auth = ctx.config_dir.join("auth.json");
            let f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(auth)?;
            let writer = BufWriter::new(f);
            serde_json::to_writer(writer, &Authorization { token })?;

            tracing::info!("Authenticated as `{name}`.");
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
        _ => {
            tracing::debug!("not now..");
            unimplemented!();
        }
    }

    Ok(())
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct MetadataResponse {
    name: String,
    tags: Vec<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ErrorResponse {
    errors: Vec<ApiError>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ApiError {
    code: String,
    message: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum ApiResponse {
    Ok(MetadataResponse),
    Error(ErrorResponse),
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
    ctx: &Context,
    client: &reqwest::Client,
    duration: i64,
    credentials: SecretCredentials,
) -> anyhow::Result<()> {
    let version_re = regex::Regex::new(r"\d+\.\d+\.\d+$").unwrap();

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
                .json::<ApiResponse>()
                .await
        }
    });

    let responses = futures::future::try_join_all(registry_requests).await?;

    let content = responses
        .into_iter()
        .filter_map(|response| match response {
            ApiResponse::Ok(metadata) => Some(metadata),
            ApiResponse::Error(e) => { 
                tracing::error!(error = ?e, "Error getting a response");
                None 
            }
        })
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
    let file = File::create(ctx.config_dir.join("index.json"))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, &index)?;
    writer.flush()?;
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
    credentials: Option<&SecretCredentials>,
    pb: ProgressBar,
) -> anyhow::Result<bool> {
    let mut errored = false;
    let opts = docker_api::opts::PullOpts::builder()
        .image(&image)
        .tag(&tag)
        .auth(if let Some(creds) = credentials {
            docker_api::opts::RegistryAuth::builder()
                .username(USER)
                .password(creds.pull_key.expose_secret())
                .build()
        } else {
            docker_api::opts::RegistryAuth::builder().build()
        })
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

fn target_version_check(targets: &[Target], ctx: &Context) -> anyhow::Result<()> {
    let file = File::open(ctx.config_dir.join("index.json"))?;
    let reader = BufReader::new(file);
    let index: Index = serde_json::from_reader(reader)?;
    for target in targets {
        let version = target.get_version();
        if let Some(version) = version {
            let entry = index
                .content
                .iter()
                .find(|metadata| metadata.for_target(target))
                .unwrap();
            if !entry.contains_version(version) {
                tracing::warn!(%target, %version, available_versions = ?entry.parsed_versions.iter(), "Specified unknown version for target");
            }
        }
    }
    Ok(())
}
