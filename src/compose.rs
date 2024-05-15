use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
};

use crate::env::Feature;
use indicatif::{ProgressBar, ProgressStyle};

use serde::{Deserialize, Serialize};
use tokio::{
    io::AsyncWriteExt,
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
    pub streamed_file_content: Option<&'a str>,
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
        if opts.streamed_file_content.is_some() {
            files.extend(&["-f", "-"])
        }

        // TODO: On Windows, current dir doesn't work, because it'll use Windows paths where Unix paths are expected.
        // We may use `wslpath-rs` (that requires WSL to be installed obviously), or maybe we can just pass `docker compose -f C:\path\to\compose.yml`..
        Command::new("docker")
            .current_dir(msde_dir)
            .stdout(stdout)
            .stderr(stderr)
            .stdin(Stdio::piped())
            .arg("compose")
            .args(files)
            .arg("up")
            .args(opts.into_args())
            .env("VSN", "3.10.0")
            .spawn()
            .map_err(Into::into)
    }
}

pub struct Pipeline;

// TODO: Consider this instead of the tokio::select for timeouts: https://stackoverflow.com/questions/43705010/how-to-query-a-child-process-status-regularly

impl Pipeline {
    // TODO: Don't repeat everything..
    pub async fn from_features<P: AsRef<Path>>(
        features: &mut [Feature],
        msde_dir: P,
        timeout: u64,
    ) {
        // TODO: don't unwrap everywhere
        features.sort();

        let volumes = generate_volumes(features, &msde_dir).unwrap();
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
        pb.set_message("Booting base services..");
        let mut child = Compose::up_custom(
            &[DOCKER_COMPOSE_BASE],
            Some(ComposeOpts {
                daemon: true,
                target: None,
                streamed_file_content: None,
            }),
            Stdio::null(),
            Stdio::null(),
            &msde_dir,
        )
        .unwrap();
        tokio::select! {
            exc = child.wait() => {
                 // TODO: Check exit status, if error, do the same as timeout (write to a log file)
                    match exc {
                        Ok(status) if status.success() => {
                            pb.finish_with_message("✅ Base services started.")
                        },
                        Ok(_) => todo!(),
                        Err(_) => todo!(),
                    }
            },
            _ = tokio::time::sleep(std::time::Duration::from_secs(timeout)) => {
                println!("timed out, killing process");
                child.kill().await.unwrap()
            },

        }
        let last_feature_idx = features.len().saturating_sub(1);
        let bot_enabled = features.iter().any(|f| matches!(f, Feature::Bot));

        // TODO: Sort features, inject base msde, if bot is not specified.
        for (i, feature) in features.iter().enumerate() {
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
            pb.set_message(format!("Booting {}..", feature));
            let f = feature.to_target();
            let mut child = Compose::up_custom(
                &[f],
                Some(ComposeOpts {
                    daemon: true,
                    target: if i == last_feature_idx && bot_enabled {
                        // TODO: If bot is enabled, it's not necessary?
                        Some("msde-vm-dev")
                    } else {
                        None
                    },
                    streamed_file_content: if i == last_feature_idx && bot_enabled {
                        Some(&volumes)
                    } else {
                        None
                    },
                }),
                Stdio::piped(),
                Stdio::piped(),
                &msde_dir,
            )
            .unwrap();
            // TODO: ensure that no deadlock is possible because of this
            if i == last_feature_idx && bot_enabled {
                let mut stdin = child.stdin.take().unwrap();
                stdin.write_all(volumes.as_bytes()).await.unwrap();
                stdin.flush().await.unwrap();
                drop(stdin);
            }
            tokio::select! {
                exc = child.wait() => {
                    // TODO: Check exit status, if error, do the same as timeout
                    match exc {
                        Ok(status) if status.success() => {
                            pb.finish_with_message(format!("✅ {feature} started."))
                        },
                        Ok(status) => {
                            pb.finish_with_message(format!("❌ Failed to start {feature}, stopping process.. (exit status {:?})", status.code()));
                            // TODO: implement this..
                            // let stdout = child.stdout.take().unwrap();
                            // let stderr = child.stderr.take().unwrap();
                            // let log_path = write_failed_start_log(&msde_dir, stdout, stderr).await.unwrap();
                            // println!("You may find the output of the failing command at:");
                            // println!("  {}  ", log_path.display());
                            break;
                        },
                        Err(_) => todo!(),
                    }
                },
                // TODO: --timeout flag to control the duration
                _ = tokio::time::sleep(std::time::Duration::from_secs(timeout)) => {
                    // These may be useful
                    // https://docs.rs/os_pipe/latest/os_pipe/
                    // https://docs.rs/duct/latest/duct/
                    // https://docs.rs/wait-timeout/latest/wait_timeout/
                    // Read https://stackoverflow.com/questions/49062707/capture-both-stdout-stderr-via-pipe
                    pb.finish_with_message(format!("❌ {feature} timed out, stopping process.."));
                    child.start_kill().unwrap();
                    let result  = child.wait_with_output().await.unwrap();
                    let log_path = write_failed_start_log(&msde_dir, &result.stdout, &result.stderr).await.unwrap();
                    println!("You may find the output of the failing command at:");
                    println!("  {}  ", log_path.display());
                    break;
                },

            }
        }

        if !bot_enabled {
            println!("should start MSDE too..");
        }
    }
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
