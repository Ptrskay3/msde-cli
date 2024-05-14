use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use crate::env::Feature;
use indicatif::{ProgressBar, ProgressStyle};

use tokio::process::{Child, ChildStderr, ChildStdout, Command};
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

pub struct Pipeline;

impl Pipeline {
    // TODO: Generate volumes (probably pass them via stdin and --, and don't write out file to disk)
    // also TODO: Don't repeat everything..
    pub async fn from_features<P: AsRef<Path>>(features: &[Feature], msde_dir: P) {
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
            _ = tokio::time::sleep(std::time::Duration::from_secs(100)) => {
                println!("timed out, killing process");
                child.kill().await.unwrap()
            },

        }
        for feature in features {
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
                    target: None,
                }),
                Stdio::piped(),
                Stdio::piped(),
                &msde_dir,
            )
            .unwrap();
            tokio::select! {
                exc = child.wait() => {
                    // TODO: Check exit status, if error, do the same as timeout
                    match exc {
                        Ok(status) if status.success() => {
                            pb.finish_with_message(format!("✅ {feature} started."))
                        },
                        Ok(status) => {
                            pb.finish_with_message(format!("❌ Failed to start {feature}, stopping process.. (exit status {:?})", status.code()));
                            let stdout = child.stdout.take().unwrap();
                            let stderr = child.stderr.take().unwrap();
                            let log_path = write_failed_start_log(&msde_dir, stdout, stderr).await.unwrap();
                            println!("You may find the output of the failing command at:");
                            println!("  {}  ", log_path.display());
                            break;
                        },
                        Err(_) => todo!(),
                    }
                },
                _ = tokio::time::sleep(std::time::Duration::from_secs(100)) => {
                    pb.finish_with_message(format!("❌ {feature} timed out, stopping process.."));
                    // FIXME: kill or start_kill? kill may block forever.. This way it becomes zombie proc.
                    child.start_kill().unwrap();
                    let stdout = child.stdout.take().unwrap();
                    let stderr = child.stderr.take().unwrap();
                    let log_path = write_failed_start_log(&msde_dir, stdout, stderr).await.unwrap();
                    println!("You may find the output of the failing command at:");
                    println!("  {}  ", log_path.display());
                    // TODO: this may block forever.. try to avoid it.
                    child.kill().await.unwrap();
                    break;
                },

            }
        }
    }
}

// TODO: Add timestamp
#[allow(unused)]
async fn write_failed_start_log<P: AsRef<Path>>(
    msde_dir: P,
    mut stdout: ChildStdout,
    mut stderr: ChildStderr,
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
    tokio::io::copy(&mut stdout, &mut writer).await?;
    tokio::io::copy(&mut "\nFailing process stderr:\n".as_bytes(), &mut writer).await?;
    tokio::io::copy(&mut stderr, &mut writer).await?;

    Ok(log_file)
}
