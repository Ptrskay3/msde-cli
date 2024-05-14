use std::{path::Path, process::Stdio};

use crate::env::Feature;
use indicatif::{ProgressBar, ProgressStyle};
use tokio::io::AsyncReadExt;

use tokio::process::{Child, Command};
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

#[derive(Default)]
pub struct ComposeOpts<'a> {
    daemon: bool,
    target: Option<&'a str>,
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
        // TODO: Maybe we need the control over spawning
    ) -> anyhow::Result<Child>
    where
        S: Into<Stdio>,
        P: AsRef<Path>,
    {
        let files = files
            .iter()
            .flat_map(|file| ["-f", file])
            .collect::<Vec<_>>();
        let opts = opts.unwrap_or_default().into_args();
        Command::new("docker")
            .current_dir(msde_dir)
            .stdout(stdout)
            .stderr(stderr)
            .arg("compose")
            .args(files)
            .arg("up")
            .args(opts)
            .env("VSN", "3.10.0")
            .spawn()
            .map_err(Into::into)

        // let status = child.wait()?;
        // if !status.success() {
        //     eprintln!("docker compose failed with exit code: {}", status);
        // }

        // Ok(())
    }
}

pub struct Pipeline {}

impl Pipeline {
    pub async fn from_features<P: AsRef<Path>>(features: &[Feature], msde_dir: P) -> Self {
        let spinner_style = ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {wide_msg}")
            .unwrap()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");
        let pb = ProgressBar::new(1);
        pb.set_style(spinner_style);
        pb.enable_steady_tick(std::time::Duration::from_millis(80));
        pb.set_message("Booting base services..");
        // let bot = features.iter().any(|f| matches!(f, Feature::Bot));
        let mut child = Compose::up_custom(
            &[DOCKER_COMPOSE_BASE],
            Some(ComposeOpts {
                daemon: true,
                target: None,
            }),
            Stdio::null(),
            Stdio::null(),
            &msde_dir,
        )
        .unwrap();
        tokio::select! {
            _ = child.wait() => {
                pb.finish_with_message("✅ Base services started.")
            },
            _ = tokio::time::sleep(std::time::Duration::from_secs(100)) => {
                println!("timed out, killing process");
                child.kill().await.unwrap()
            },

        }
        'l: for feature in features {
            let spinner_style =
                ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {wide_msg}")
                    .unwrap()
                    .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");
            let pb = ProgressBar::new(1);
            pb.set_style(spinner_style);
            pb.enable_steady_tick(std::time::Duration::from_millis(80));
            pb.set_message(format!("Booting {}..", feature));
            let f = feature.to_target();
            let mut child = Compose::up_custom(
                &[f],
                Some(ComposeOpts {
                    daemon: true,
                    target: None,
                }),
                Stdio::piped(),
                Stdio::piped(),
                &msde_dir,
            )
            .unwrap();
            tokio::select! {
                _ = child.wait() => {
                    pb.finish_with_message(format!("✅ {feature} started."))
                },
                _ = tokio::time::sleep(std::time::Duration::from_secs(100)) => {
                    pb.finish_with_message("❌ {feature} timed out, killing process.. process stdout was");
                    // TODO: Do not print these to stdout, create a log file at the project dir.
                    child.kill().await.unwrap();
                    let mut op = child.stdout.take().unwrap();
                    let mut buf = String::new();
                    op.read_to_string(&mut buf).await.unwrap();
                    println!("{buf}");
                    break 'l;
                },

            }
        }
        Self {}
    }
}
