use std::{
    collections::HashMap,
    future::Future,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use crate::{env::Feature, game::rpc};
use anyhow::Context as _;
use docker_api::{
    opts::{ContainerRemoveOpts, ExecCreateOpts},
    Docker, Exec,
};

use futures::{StreamExt, TryFutureExt, TryStreamExt};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
};
pub struct Compose;

#[allow(dead_code)]
pub static DOCKER_COMPOSE_MAIN: &str = "docker/docker-compose.yml";
#[allow(dead_code)]
pub static DOCKER_COMPOSE_BASE: &str = "docker/docker-compose-base.yml";
#[allow(dead_code)]
pub static DOCKER_COMPOSE_METRICS: &str = "docker/docker-compose-metrics.yml";
#[allow(dead_code)]
pub static DOCKER_COMPOSE_WEB3: &str = "docker/docker-compose-web3.yml";
#[allow(dead_code)]
pub static DOCKER_COMPOSE_OTEL: &str = "docker/docker-compose-otel.yml";
#[allow(dead_code)]
pub static DOCKER_COMPOSE_BOT: &str = "docker/docker-compose-bot.yml";

const MERIGO_GAMES_DIR: &str = "/usr/local/bin/merigo/games";
const MERIGO_SAMPLE_DIR: &str = "/usr/local/bin/merigo/samples";

#[derive(Default)]
pub struct ComposeOpts<'a> {
    pub daemon: bool,
    pub target: Option<&'a str>,
    pub file_streamed_stdin: bool,
    pub build: bool,
}

impl<'a> ComposeOpts<'a> {
    fn into_args(self) -> Vec<&'a str> {
        let mut args = vec![];
        if self.daemon {
            args.push("-d");
        }
        if self.build {
            args.push("--build");
        }
        if let Some(target) = self.target {
            args.push(target)
        }

        args
    }
}

impl Compose {
    pub fn start_custom<S, P>(
        files: &[&str],
        opts: Option<ComposeOpts>,
        stdout: S,
        stderr: S,
        stdin: S,
        msde_dir: P,
    ) -> anyhow::Result<Child>
    where
        S: Into<Stdio>,
        P: AsRef<Path>,
    {
        let mut files = files
            .iter()
            .flat_map(|file| ["-f", file])
            .collect::<Vec<_>>();
        let opts = opts.unwrap_or_default();
        if opts.file_streamed_stdin {
            files.extend(&["-f", "-"])
        }

        Command::new("docker")
            .current_dir(msde_dir)
            .stdout(stdout)
            .stderr(stderr)
            .stdin(stdin)
            .arg("compose")
            .args(files)
            .arg("start")
            .args(opts.into_args())
            .env("VSN", "3.10.0") // TODO: Do not hardcode
            .spawn()
            .map_err(Into::into)
    }

    pub fn up_custom<S, P>(
        files: &[&str],
        opts: Option<ComposeOpts>,
        stdout: S,
        stderr: S,
        stdin: S,
        msde_dir: P,
    ) -> anyhow::Result<Child>
    where
        S: Into<Stdio>,
        P: AsRef<Path>,
    {
        let mut files = files
            .iter()
            .flat_map(|file| ["-f", file])
            .collect::<Vec<_>>();
        let opts = opts.unwrap_or_default();
        if opts.file_streamed_stdin {
            files.extend(&["-f", "-"])
        }

        Command::new("docker")
            .current_dir(msde_dir)
            .stdout(stdout)
            .stderr(stderr)
            .stdin(stdin)
            .arg("compose")
            .args(files)
            .arg("up")
            .args(opts.into_args())
            .env("VSN", "3.10.0") // TODO: Do not hardcode
            .spawn()
            .map_err(Into::into)
    }

    pub fn stop_all<P>(msde_dir: P) -> anyhow::Result<Child>
    where
        P: AsRef<Path>,
    {
        let files = &[
            DOCKER_COMPOSE_BOT,
            DOCKER_COMPOSE_MAIN,
            DOCKER_COMPOSE_METRICS,
            DOCKER_COMPOSE_OTEL,
            DOCKER_COMPOSE_WEB3,
        ]
        .iter()
        .flat_map(|file| ["-f", file])
        .collect::<Vec<_>>();

        Command::new("docker")
            .current_dir(msde_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .arg("compose")
            .args(files)
            .arg("stop")
            .spawn()
            .map_err(Into::into)
    }

    pub fn down_all<P>(msde_dir: P) -> anyhow::Result<Child>
    where
        P: AsRef<Path>,
    {
        let files = &[
            DOCKER_COMPOSE_BOT,
            DOCKER_COMPOSE_MAIN,
            DOCKER_COMPOSE_METRICS,
            DOCKER_COMPOSE_OTEL,
            DOCKER_COMPOSE_WEB3,
        ]
        .iter()
        .flat_map(|file| ["-f", file])
        .collect::<Vec<_>>();

        Command::new("docker")
            .current_dir(msde_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .arg("compose")
            .args(files)
            .arg("down")
            .spawn()
            .map_err(Into::into)
    }
}

pub struct Pipeline;

impl Pipeline {
    pub async fn down_all<P: AsRef<Path>>(
        docker: &Docker,
        msde_dir: P,
        timeout: u64,
    ) -> anyhow::Result<()> {
        let spinner_style = ProgressStyle::with_template("{spinner:.blue} {msg}")
            .unwrap()
            .tick_strings(&[
                "⠁", "⠂", "⠄", "⡀", "⡈", "⡐", "⡠", "⣀", "⣁", "⣂", "⣄", "⣌", "⣔", "⣤", "⣥", "⣦",
                "⣮", "⣶", "⣷", "⣿", "⡿", "⠿", "⢟", "⠟", "⡛", "⠛", "⠫", "⢋", "⠋", "⠍", "⡉", "⠉",
                "⠑", "⠡", "⢁",
            ]);
        let pb = ProgressBar::new(1);
        pb.set_style(spinner_style);
        pb.enable_steady_tick(std::time::Duration::from_millis(80));
        pb.set_message("Stopping all services..");
        let mut child = Compose::down_all(&msde_dir)?;

        tokio::select! {
            exc = child.wait() => {
                match exc {
                    Ok(status) if status.success() => {
                        clean_otel_volumes(docker).await?;
                        web3_stop_consumers(docker).await?;
                        pb.finish_with_message("✅ All services stopped.")
                    },
                    Ok(status) => {
                        pb.finish_with_message(format!("❌ Failed to stop services, stopping process.. (exit status {:?})", status.code().unwrap_or(1)));
                        let mut stdout = child.stdout.take().context("Failed to take child stdout")?;
                        let mut stderr = child.stderr.take().context("Failed to take child stderr")?;
                        let mut stdout_buf = vec![];
                        let mut stderr_buf = vec![];
                        stdout.read_to_end(&mut stdout_buf).await?;
                        stderr.read_to_end(&mut stderr_buf).await?;
                        drop(stdout);
                        drop(stderr);

                        let log_path = write_failed_start_log(&msde_dir, stdout_buf.as_slice(), stderr_buf.as_slice()).await?;
                        println!("You may find the output of the failing command at:");
                        println!("  {}  ", log_path.display());
                        return Err(anyhow::Error::msg("Failed"));

                    },
                    Err(e) => {
                        // FIXME: Unclear from the documentation what happens here. Probably things go really wrong here, so we should just exit immediately.
                        eprintln!("{e}");
                        return Err(anyhow::Error::msg("Failed"));

                    },
                }
            },
            _ = tokio::time::sleep(std::time::Duration::from_secs(timeout)) => {
                pb.finish_with_message("❌ Stopping services timed out, stopping process..");
                child.start_kill()?;
                let result  = child.wait_with_output().await?;
                let log_path = write_failed_start_log(&msde_dir, &result.stdout, &result.stderr).await?;
                println!("You may find the output of the failing command at:");
                println!("  {}  ", log_path.display());
                return Err(anyhow::Error::msg("Failed"));
            },
        }
        Ok(())
    }

    pub async fn stop_all<P: AsRef<Path>>(
        docker: &Docker,
        msde_dir: P,
        timeout: u64,
    ) -> anyhow::Result<()> {
        let spinner_style = ProgressStyle::with_template("{spinner:.blue} {msg}")
            .unwrap()
            .tick_strings(&[
                "⠁", "⠂", "⠄", "⡀", "⡈", "⡐", "⡠", "⣀", "⣁", "⣂", "⣄", "⣌", "⣔", "⣤", "⣥", "⣦",
                "⣮", "⣶", "⣷", "⣿", "⡿", "⠿", "⢟", "⠟", "⡛", "⠛", "⠫", "⢋", "⠋", "⠍", "⡉", "⠉",
                "⠑", "⠡", "⢁",
            ]);
        let pb = ProgressBar::new(1);
        pb.set_style(spinner_style);
        pb.enable_steady_tick(std::time::Duration::from_millis(80));
        pb.set_message("Stopping all services..");
        let mut child = Compose::stop_all(&msde_dir)?;

        tokio::select! {
            exc = child.wait() => {
                match exc {
                    Ok(status) if status.success() => {
                        web3_stop_consumers(docker).await?;
                        pb.finish_with_message("✅ All services stopped.")
                    },
                    Ok(status) => {
                        pb.finish_with_message(format!("❌ Failed to stop services, stopping process.. (exit status {:?})", status.code().unwrap_or(1)));
                        let mut stdout = child.stdout.take().context("Failed to take child stdout")?;
                        let mut stderr = child.stderr.take().context("Failed to take child stderr")?;
                        let mut stdout_buf = vec![];
                        let mut stderr_buf = vec![];
                        stdout.read_to_end(&mut stdout_buf).await?;
                        stderr.read_to_end(&mut stderr_buf).await?;
                        drop(stdout);
                        drop(stderr);

                        let log_path = write_failed_start_log(&msde_dir, stdout_buf.as_slice(), stderr_buf.as_slice()).await?;
                        println!("You may find the output of the failing command at:");
                        println!("  {}  ", log_path.display());
                        return Err(anyhow::Error::msg("Failed"));

                    },
                    Err(e) => {
                        // FIXME: Unclear from the documentation what happens here. Probably things go really wrong here, so we should just exit immediately.
                        eprintln!("{e}");
                        return Err(anyhow::Error::msg("Failed"));

                    },
                }
            },
            _ = tokio::time::sleep(std::time::Duration::from_secs(timeout)) => {
                pb.finish_with_message("❌ Stopping services timed out, stopping process..");
                child.start_kill()?;
                let result  = child.wait_with_output().await?;
                let log_path = write_failed_start_log(&msde_dir, &result.stdout, &result.stderr).await?;
                println!("You may find the output of the failing command at:");
                println!("  {}  ", log_path.display());
                return Err(anyhow::Error::msg("Failed"));
            },
        }
        Ok(())
    }

    // FIXME: Too many arguments
    pub async fn up_from_features<
        P: AsRef<Path>,
        F: Future<Output = anyhow::Result<()>>,
        G: Future<Output = anyhow::Result<()>>,
    >(
        features: &mut [Feature],
        msde_dir: P,
        vsn: &str,
        timeout: u64,
        docker: &docker_api::Docker,
        quiet: bool,
        build: bool,
        attach_future: Option<F>,
        import_hook: Option<G>,
        raw: bool,
    ) -> anyhow::Result<()> {
        features.sort();

        let volumes =
            generate_volumes(features, &msde_dir).context("Failed to generate volume bindings")?;
        let pb = progress_spinner(quiet || raw);
        pb.set_message("Booting base services..");
        let child = Compose::up_custom(
            &[DOCKER_COMPOSE_BASE],
            Some(ComposeOpts {
                daemon: true,
                target: None,
                file_streamed_stdin: false,
                build,
            }),
            if raw {
                Stdio::inherit()
            } else {
                Stdio::piped()
            },
            if raw {
                Stdio::inherit()
            } else {
                Stdio::piped()
            },
            Stdio::piped(),
            &msde_dir,
        )?;
        wait_child_with_timeout(child, &pb, timeout, &msde_dir, "Base services").await?;

        let last_feature_idx = features.len().saturating_sub(1);
        let bot_enabled = features.iter().any(|f| matches!(f, Feature::Bot));

        for (i, feature) in features.iter().enumerate() {
            let pb = progress_spinner(quiet || raw);
            pb.set_message(format!("Booting {}..", feature));
            let f = feature.to_target();
            let mut child = Compose::up_custom(
                &[f],
                Some(ComposeOpts {
                    daemon: true,
                    // FIXME: bot_enabled should be negated?
                    target: if i == last_feature_idx && bot_enabled {
                        Some("msde-vm-dev")
                    } else {
                        None
                    },
                    file_streamed_stdin: i == last_feature_idx && bot_enabled,
                    build,
                }),
                if raw {
                    Stdio::inherit()
                } else {
                    Stdio::piped()
                },
                if raw {
                    Stdio::inherit()
                } else {
                    Stdio::piped()
                },
                Stdio::piped(),
                &msde_dir,
            )?;
            // Attach volumes to the bot command, if it's enabled.
            if i == last_feature_idx && bot_enabled {
                let mut stdin = child.stdin.take().context("Failed to take child stdin")?;
                stdin.write_all(volumes.as_bytes()).await?;
                stdin.flush().await?;
                drop(stdin);
            }
            wait_child_with_timeout(child, &pb, timeout, &msde_dir, &feature.to_string()).await?;
        }

        if !bot_enabled {
            let pb = progress_spinner(quiet || raw);
            pb.set_message("Booting MSDE..");
            let mut child = Compose::up_custom(
                &[DOCKER_COMPOSE_MAIN],
                Some(ComposeOpts {
                    daemon: true,
                    target: Some("msde-vm-dev"),
                    file_streamed_stdin: true,
                    build,
                }),
                if raw {
                    Stdio::inherit()
                } else {
                    Stdio::piped()
                },
                if raw {
                    Stdio::inherit()
                } else {
                    Stdio::piped()
                },
                Stdio::piped(),
                &msde_dir,
            )?;
            // Attach volumes to the MSDE up command, since it's the last one running.
            let mut stdin = child.stdin.take().context("Failed to take child stdin")?;
            stdin.write_all(volumes.as_bytes()).await?;
            stdin.flush().await?;
            drop(stdin);
            wait_child_with_timeout(child, &pb, timeout, msde_dir, "MSDE").await?;
        }
        pb.set_message("🪝 Registering post-init hooks..");
        if features.contains(&Feature::Metrics) {
            init_grafana(docker.clone())
                .await
                .context("Failed to run grafana init script")?;
        }
        if features.contains(&Feature::Web3) {
            web3_patch(docker.clone())
                .await
                .context("Failed to patch Web3")?;
        }

        rewrite_sysconfig(docker.clone(), features, vsn)
            .await
            .context("Failed to rewrite sys.config")?;
        let mut handle = None;
        if !features.contains(&Feature::OTEL) {
            // Have to delay this, since the node may be down at this point of time.
            let docker = docker.clone();
            handle = Some(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(8)).await;
                if let Err(e) = disable_otel(docker).await {
                    eprintln!("Failed to disable OTEL in MSDE: {e}");
                }
            }));
        }
        pb.finish_with_message("✅ Registered post-init hooks.");
        match (attach_future, import_hook) {
            (None, None) => {
                wait_with_timeout(docker, quiet).await?;
            }
            (None, Some(import_hook)) => {
                wait_with_timeout(docker, quiet).await?;
                import_hook.await?;
            }
            (Some(attach_future), None) => {
                pb.set_draw_target(ProgressDrawTarget::hidden());
                tracing::info!("Attaching to MSDE logs..");
                // Attaching overrides quiet, since we don't want to intercept logs from the container with the progress spinner.
                if let Err(e) = tokio::try_join!(attach_future, wait_with_timeout(docker, true)) {
                    tracing::error!(error = %e, "Failed to start MSDE");
                    anyhow::bail!("Failed.");
                }
            }
            (Some(attach_future), Some(import_hook)) => {
                // This is a bit tricky: We'd like to attach immediately, so users can see logs, but we have to run the health check in the
                // background as well. However, we can't start importing games until the health check is ok. To do this, we chain the health
                // check and the import hook as one single future.
                pb.set_draw_target(ProgressDrawTarget::hidden());
                tracing::info!("Attaching to MSDE logs..");
                let chained_import_future =
                    wait_with_timeout(docker, true).and_then(|_| import_hook);
                if let Err(e) = tokio::try_join!(attach_future, chained_import_future) {
                    tracing::error!(error = %e, "Failed to start MSDE");
                    anyhow::bail!("Failed.");
                }
            }
        }

        pb.set_message("Waiting for post-init hooks to finish..");
        // If we don't attach, we'll need to wait for this delayed call to finish before exiting.
        if let Some(handle) = handle {
            handle.await?;
        }
        pb.finish_with_message("✅ MSDE is ready.");
        Ok(())
    }
}

async fn wait_child_with_timeout<P: AsRef<Path>>(
    mut child: Child,
    pb: &ProgressBar,
    timeout: u64,
    msde_dir: P,
    target: &str,
) -> anyhow::Result<()> {
    tokio::select! {
        exc = child.wait() => {
            match exc {
                Ok(status) if status.success() => {
                    pb.finish_with_message(format!("✅ {target} started."))
                },
                Ok(status) => {
                    pb.finish_with_message(format!("❌ Failed to start {target}, stopping process.. (exit status {:?})", status.code().unwrap_or(1)));
                    let mut stdout = child.stdout.take().context("Failed to take child stdout")?;
                    let mut stderr = child.stderr.take().context("Failed to take child stderr")?;
                    let mut stdout_buf = vec![];
                    let mut stderr_buf = vec![];
                    stdout.read_to_end(&mut stdout_buf).await?;
                    stderr.read_to_end(&mut stderr_buf).await?;
                    drop(stdout);
                    drop(stderr);

                    let log_path = write_failed_start_log(&msde_dir, stdout_buf.as_slice(), stderr_buf.as_slice()).await?;
                    println!("You may find the output of the failing command at:");
                    println!("  {}  ", log_path.display());
                    return Err(anyhow::Error::msg("Failed"));
                },
                Err(e) => {
                    // FIXME: Unclear from the documentation what happens here. Probably things go really wrong here, so we should just exit immediately.
                    println!("{e}");
                    return Err(anyhow::Error::msg("Failed"));
                }
            }
        },
        _ = tokio::time::sleep(std::time::Duration::from_secs(timeout)) => {
            pb.finish_with_message(format!("❌ {target} timed out, stopping process.."));
            child.start_kill()?;
            let result  = child.wait_with_output().await?;
            let log_path = write_failed_start_log(&msde_dir, &result.stdout, &result.stderr).await?;
            println!("You may find the output of the failing command at:");
            println!("  {}  ", log_path.display());
            return Err(anyhow::Error::msg("Failed"));
        },
    }
    Ok(())
}

// TODO: Add timestamp
#[allow(unused)]
async fn write_failed_start_log<P: AsRef<Path>>(
    msde_dir: P,
    stdout: &[u8],
    stderr: &[u8],
) -> anyhow::Result<PathBuf> {
    let log_dir = msde_dir.as_ref().join("log");
    std::fs::create_dir_all(&log_dir)?;
    let log_file = log_dir.join("output.log");
    let f = tokio::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(&log_file)
        .await?;
    let mut writer = tokio::io::BufWriter::new(f);
    tokio::io::copy(&mut "Failing process stdout:\n".as_bytes(), &mut writer).await?;
    writer.write_all(stdout).await?;
    tokio::io::copy(&mut "\nFailing process stderr:\n".as_bytes(), &mut writer).await?;
    writer.write_all(stderr).await?;
    writer.flush().await?;

    Ok(log_file)
}

pub fn progress_spinner(quiet: bool) -> ProgressBar {
    let spinner_style = ProgressStyle::with_template("{spinner:.blue} {msg}")
        .unwrap()
        .tick_strings(&[
            "⠁", "⠂", "⠄", "⡀", "⡈", "⡐", "⡠", "⣀", "⣁", "⣂", "⣄", "⣌", "⣔", "⣤", "⣥", "⣦", "⣮",
            "⣶", "⣷", "⣿", "⡿", "⠿", "⢟", "⠟", "⡛", "⠛", "⠫", "⢋", "⠋", "⠍", "⡉", "⠉", "⠑", "⠡",
            "⢁",
        ]);
    let pb = ProgressBar::new(1);
    if quiet {
        pb.set_draw_target(ProgressDrawTarget::hidden());
    }
    pb.set_style(spinner_style);
    pb.enable_steady_tick(std::time::Duration::from_millis(80));
    pb
}

fn generate_volumes(features: &[Feature], msde_dir: impl AsRef<Path>) -> anyhow::Result<String> {
    let games_dir = msde_dir.as_ref().join("games");
    let samples_dir = msde_dir.as_ref().join("samples");
    let volumes = vec![
        format!("{}:{MERIGO_GAMES_DIR}", games_dir.display()),
        format!("{}:{MERIGO_SAMPLE_DIR}", samples_dir.display()),
    ];
    let service = Service { volumes };

    let mut mapping = Services {
        services: HashMap::new(),
    };
    mapping.services.insert("compiler-vm-dev", service.clone());
    mapping.services.insert("msde-vm-dev", service.clone());
    if features.iter().any(|f| matches!(f, Feature::Bot)) {
        mapping.services.insert("bot-vm-dev", service);
    }
    serde_yaml::to_string(&mapping).map_err(Into::into)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Services<'a> {
    #[serde(borrow)]
    services: HashMap<&'a str, Service>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Service {
    volumes: Vec<String>,
}

pub async fn running_containers(
    docker: &docker_api::Docker,
) -> anyhow::Result<HashMap<String, String>> {
    Ok(docker
        .containers()
        .list(&Default::default())
        .await?
        .into_iter()
        .map(|c| {
            (
                c.names.unwrap_or_default(),
                c.id.unwrap_or_else(|| String::from("unknown")),
            )
        })
        .map(|(mut c, id)| (c.pop().unwrap_or_else(|| String::from("unknown")), id))
        .collect())
}

pub async fn wait_until_heathy(docker: &docker_api::Docker, target_id: &str) -> anyhow::Result<()> {
    loop {
        let health = docker
            .containers()
            .get(target_id)
            .inspect()
            .await?
            .state
            .context("Failed to get container state")?
            .health
            .context("Failed to get container health")?
            .status
            .context("Failed to get container health status")?;

        if health.as_str() == "healthy" {
            break Ok(());
        } else if health.as_str() == "unhealthy" {
            break Err(anyhow::Error::msg("container failed to start"));
        } else if health.as_str() == "none" {
            break Err(anyhow::Error::msg("health check not defined for container"));
        }

        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

pub async fn wait_with_timeout(docker: &docker_api::Docker, quiet: bool) -> anyhow::Result<()> {
    let containers = running_containers(docker).await?;
    let msde_id = containers
        .get("/msde-vm-dev")
        .context("MSDE is not running somehow?")?;
    let pb = progress_spinner(quiet);
    pb.set_message("Waiting for MSDE to be healthy..");
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {
            pb.finish_with_message("❌ MSDE health check timed out.");
        }
        r = wait_until_heathy(docker, msde_id) => {
            match r {
                Ok(_) => pb.finish_with_message("✅ MSDE is healthy."),
                Err(e) => { pb.finish_with_message("❌ MSDE health check failed."); tracing::error!(%e); }
            }
        }
    }
    Ok(())
}

pub async fn web3_stop_consumers(docker: &Docker) -> anyhow::Result<()> {
    let consumer_events = String::from("consumer_events");
    let containers = docker
        .containers()
        .list(&Default::default())
        .await?
        .into_iter()
        .filter(|container| {
            container
                .names
                .iter()
                .any(|name| name.contains(&consumer_events))
        })
        .collect::<Vec<_>>();

    for container in containers {
        if let Some(id) = container.id {
            docker
                .containers()
                .get(&id)
                .remove(
                    &ContainerRemoveOpts::builder()
                        .force(true)
                        .volumes(true)
                        .build(),
                )
                .await?;
        }
    }

    Ok(())
}

pub async fn clean_otel_volumes(docker: &Docker) -> anyhow::Result<()> {
    const VOLUMES_TO_CLEAR: [&str; 4] = [
        "docker_esdata01-vm-dev",
        "docker_kibanadata-vm-dev",
        "docker_msde-log-dev",
        "docker_logstash-vm-dev",
    ];

    for volume_name in &VOLUMES_TO_CLEAR {
        if let Err(e) = docker.volumes().get(volume_name.to_owned()).delete().await {
            tracing::debug!("Failed to remove volume {}: {}", volume_name, e);
        }
    }

    Ok(())
}

async fn run_command_in_container(
    docker: Docker,
    container: &str,
    cmd: &[&str],
) -> anyhow::Result<()> {
    let containers = running_containers(&docker).await?;
    let id = containers
        .get(container)
        .with_context(|| format!("{:?} is not running", container))?;

    let opts = ExecCreateOpts::builder()
        .command(cmd)
        .attach_stdout(false)
        .tty(true)
        .build();

    let exec = Exec::create(docker, id, &opts).await?;

    let mut stream = exec.start(&Default::default()).await?;

    while let Some(Ok(_)) = stream.next().await {}

    Ok(())
}

pub async fn web3_patch(docker: Docker) -> anyhow::Result<()> {
    let reg_web3 = [
        "curl",
        "-X",
        "PUT",
        "-d",
        r#"{"ID": "web3_services", "Name": "web3_services", "Address": "172.99.0.7"}"#,
        "http://172.99.0.2:8500/v1/agent/service/register",
    ];
    let unreg = [
        "curl",
        "-X",
        "PUT",
        "http://172.99.0.2:8500/v1/agent/service/deregister/msde_game:msde@172.99.0.5",
    ];
    let reg_msde = [
        "curl",
        "-X",
        "PUT",
        "-d",
        r#"{"ID": "msde_game", "Name": "msde_game", "Address": "172.99.0.5"}"#,
        "http://172.99.0.2:8500/v1/agent/service/register",
    ];

    let container_name = "/web3-vm-dev";

    run_command_in_container(docker.clone(), container_name, &reg_web3).await?;
    // FIXME: original did unreg 3 times
    run_command_in_container(docker.clone(), container_name, &unreg).await?;
    run_command_in_container(docker, container_name, &reg_msde).await?;

    Ok(())
}

pub async fn init_grafana(docker: Docker) -> anyhow::Result<()> {
    run_command_in_container(
        docker,
        "/grafana-vm-dev",
        &["bash", "/usr/local/grafana/init.sh"],
    )
    .await?;
    Ok(())
}

pub async fn rewrite_sysconfig(
    docker: Docker,
    features: &[Feature],
    vsn: &str,
) -> anyhow::Result<()> {
    let container_name = "/msde-vm-dev";
    let container_file_path = format!("/usr/local/bin/merigo/msde/releases/{}/sys.config", vsn);

    // TODO: This is doing more work than it needs to for getting the container id..
    let containers = running_containers(&docker).await?;
    let id = containers
        .get(container_name)
        .with_context(|| format!("{} is not running", container_name))?;

    let bytes = docker
        .containers()
        .get(id)
        .copy_from(Path::new(&container_file_path))
        .try_concat()
        .await?;

    let mut archive = tar::Archive::new(&bytes[..]);
    let mut sys_config = archive
        .entries()
        .context("Failed to iterate archive")?
        .next()
        .context("Failed to get sys.config file")??;
    let mut buffer = String::new();
    let _bytes_read = sys_config.read_to_string(&mut buffer)?;

    if !features.contains(&Feature::OTEL) {
        buffer = buffer.replace("{traces_exporter,otlp}", "{traces_exporter,none}");
    } else {
        buffer = buffer.replace("{traces_exporter,none}", "{traces_exporter,otlp}");
    }

    if !features.contains(&Feature::Metrics) && !features.contains(&Feature::OTEL) {
        buffer = buffer.replace("{stats,[{enable,true}]}", "{stats,[{enable,false}]}");
    } else {
        buffer = buffer.replace("{stats,[{enable,false}]}", "{stats,[{enable,true}]}");
    }

    if !features.contains(&Feature::Web3) {
        buffer = buffer.replace(
            "{evmlistener,[{enable,true}]}",
            "{evmlistener,[{enable,false}]}",
        );
    } else {
        buffer = buffer.replace(
            "{evmlistener,[{enable,false}]}",
            "{evmlistener,[{enable,true}]}",
        );
    }

    if let Err(e) = docker
        .containers()
        .get(id)
        .copy_file_into(container_file_path, buffer.as_bytes())
        .await
    {
        eprintln!("Error copying back sys.config file: {e}")
    }

    let reload_config_cmd = [
        "/bin/bash",
        "-c",
        "/usr/local/bin/merigo/msde/bin/msde reload_config",
    ];
    run_command_in_container(docker.clone(), container_name, &reload_config_cmd).await?;

    Ok(())
}

async fn disable_otel(docker: Docker) -> anyhow::Result<()> {
    rpc(
        docker,
        r#"require Logger;
             Logger.warn("[OTEL] OpenTelemetry is disabled, killing related applications.") ;
             defmodule KO, do: def kill_otel(), do: (Application.stop(:opentelemetry_exporter) ; Application.stop(:opentelemetry_cowboy) ; Application.stop(:opentelemetry)) ;
             :rpc.multicall(Sys.Cluster.msdeNodes(), KO, :kill_otel, []) ;
             Logger.warn("[OTEL] Done. If you need OpenTelemetry, rerun with the otel feature enabled.")
          "#,
    ).await?;
    Ok(())
}
