use std::borrow::Cow;

use anyhow::Context as _;
use docker_api::{conn::TtyChunk, opts::ExecCreateOpts, Exec};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::compose::running_containers;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Stages {
    stages: Vec<StageConfig>,
}

// This is far from complete, but this is enough for creating a game.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StageConfig {
    guid: Uuid,
    suid: Uuid,
    name: String,
    launch: bool,
    script: LocalElement,
    tuning: LocalElement,
    #[serde(rename = "macrosEnabled")]
    macros_enabled: bool,
    evmlistener: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LocalElement {
    link: String,
}

pub fn create_game() -> anyhow::Result<()> {
    // Plan:
    // 1. Compile the template game into the CLI tool.
    // 2. Check whether the path is free (or --force) and copy over stuff.. probably generate new UUIDs.
    // 3. Check whether the stages.yml exists, and update or create it with the new game.
    // 4. Client code? Probably just skip now. Longer term we may need to initialize that too and swap IDs.
    // 5. Trigger a fresh load if MSDE is running. (load_games function..)
    // 6. Create the game config as string, and trigger an import in MSDE.
    Ok(())
}

pub async fn rpc(
    docker: docker_api::Docker,
    cmd: impl Into<Cow<'_, str>>,
) -> anyhow::Result<String> {
    let containers = running_containers(&docker).await?;
    let msde_id = containers
        .get("/msde-vm-dev")
        .context("MSDE is not running")?;
    let opts = ExecCreateOpts::builder()
        .command(vec![
            "/usr/local/bin/merigo/msde/bin/msde",
            "rpc",
            cmd.into().as_ref(),
        ])
        .attach_stdout(true)
        .tty(true)
        .privileged(true)
        .build();

    let exec = Exec::create(docker, msde_id, &opts).await?;

    let mut stream = exec.start(&Default::default()).await?;
    let mut output: Vec<u8> = vec![];
    while let Some(Ok(chunk)) = stream.next().await {
        match chunk {
            TtyChunk::StdOut(buf) => {
                output.extend(&buf[..]);
            }
            _ => {
                todo!()
            }
        }
    }

    Ok(String::from_utf8_lossy(&output).into_owned())
}
