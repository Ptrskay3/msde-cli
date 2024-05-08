use std::process::{Command, Stdio};

pub struct Compose;

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
    pub fn up_custom(files: &[&str], opts: Option<ComposeOpts>) -> anyhow::Result<()> {
        let files = files
            .iter()
            .flat_map(|file| ["-f", file])
            .collect::<Vec<_>>();
        let opts = opts.unwrap_or_default().into_args();
        let mut child = Command::new("docker")
            .stdout(Stdio::null())
            .arg("compose")
            .args(files)
            .arg("up")
            .args(opts)
            .spawn()?;

        let status = child.wait()?;
        if !status.success() {
            eprintln!("docker compose failed with exit code: {}", status);
        }

        Ok(())
    }

    pub fn up_builtin(_opts: Option<ComposeOpts>) -> anyhow::Result<()> {
        todo!();
        // let opts = opts.unwrap_or_default().into_args();
        // let mut child = Command::new("docker")
        //     .stdin(Stdio::piped())
        //     .stdout(Stdio::null()) // TODO
        //     .arg("compose")
        //     .arg("-f")
        //     .arg("-")
        //     .arg("up")
        //     .args(opts)
        //     .spawn()?;

        // let Some(mut stdin) = child.stdin.take() else {
        //     anyhow::bail!("Failed to get stdin for docker-compose")
        // };
        // stdin.write_all(DOCKER_COMPOSE_BASE.as_bytes())?;
        // drop(stdin);
        // let status = child.wait().expect("Failed to wait for docker-compose");
        // if !status.success() {
        //     eprintln!("docker compose failed with exit code: {}", status);
        // }

        // Ok(())
    }
}
