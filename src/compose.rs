use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
};

use crate::env::Feature;
use anyhow::Context as _;
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
}

impl<'a> ComposeOpts<'a> {
    fn into_args(self) -> Vec<&'a str> {
        let mut args = vec![];
        if self.daemon {
            args.push("-d");
        }
        if let Some(target) = self.target {
            args.push(target)
        }

        args
    }
}

impl Compose {
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

        // TODO: On Windows, current dir doesn't work, because it'll use Windows paths where Unix paths are expected.
        // We may use `wslpath-rs` (that requires WSL to be installed obviously), or maybe we can just pass `docker compose -f C:\path\to\compose.yml`..
        Command::new("docker")
            .current_dir(msde_dir)
            .stdout(stdout)
            .stderr(stderr)
            .stdin(stdin)
            .arg("compose")
            .args(files)
            .arg("up")
            .args(opts.into_args())
            .env("VSN", "3.10.0")
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
    pub async fn down_all<P: AsRef<Path>>(msde_dir: P, timeout: u64) -> anyhow::Result<()> {
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
                        println!("{e}");
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

    pub async fn from_features<P: AsRef<Path>>(
        features: &mut [Feature],
        msde_dir: P,
        timeout: u64,
        docker: &docker_api::Docker,
        quiet: bool,
    ) -> anyhow::Result<()> {
        features.sort();

        let volumes =
            generate_volumes(features, &msde_dir).context("Failed to generate volume bindings")?;
        let pb = progress_spinner(quiet);
        pb.set_message("Booting base services..");
        let child = Compose::up_custom(
            &[DOCKER_COMPOSE_BASE],
            Some(ComposeOpts {
                daemon: true,
                target: None,
                file_streamed_stdin: false,
            }),
            Stdio::null(),
            Stdio::null(),
            Stdio::piped(),
            &msde_dir,
        )?;
        wait_child_with_timeout(child, &pb, timeout, &msde_dir, "Base services").await?;

        let last_feature_idx = features.len().saturating_sub(1);
        let bot_enabled = features.iter().any(|f| matches!(f, Feature::Bot));

        for (i, feature) in features.iter().enumerate() {
            let pb = progress_spinner(quiet);
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
                }),
                Stdio::piped(),
                Stdio::piped(),
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
            let pb = progress_spinner(quiet);
            pb.set_message("Booting MSDE..");
            let mut child = Compose::up_custom(
                &[DOCKER_COMPOSE_MAIN],
                Some(ComposeOpts {
                    daemon: true,
                    target: Some("msde-vm-dev"),
                    file_streamed_stdin: true,
                }),
                Stdio::piped(),
                Stdio::piped(),
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
        wait_with_timeout(docker, timeout, quiet).await?;
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

#[cfg(unix)]
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

// Probably Windows needs special treatment, so let's just mark this as a todo
#[cfg(not(unix))]
fn generate_volumes(features: &[Feature], msde_dir: impl AsRef<Path>) -> anyhow::Result<String> {
    todo!()
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

pub async fn wait_with_timeout(
    docker: &docker_api::Docker,
    _timeout: u64,
    quiet: bool,
) -> anyhow::Result<()> {
    let containers = running_containers(docker).await?;
    let msde_id = containers
        .get("/msde-vm-dev")
        .context("MSDE is not running somehow?")?;
    let pb = progress_spinner(quiet);
    pb.set_message("Waiting for MSDE to be healthy..");
    tokio::select! {
        // TODO: Hardcoded the timeout for now, 60 seconds should be more than enough
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
