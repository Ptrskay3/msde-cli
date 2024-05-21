use std::{borrow::Cow, collections::HashMap};

use anyhow::Context as _;
use docker_api::{
    conn::TtyChunk,
    opts::{ConsoleSize, ExecCreateOpts},
    Exec,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::compose::running_containers;

pub const RPC_START_SEQUENCE: &str = "\u{1}\0\0\0\0\0\0\u{8}";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Stages {
    stages: Vec<StageConfig>,
    org: Option<Uuid>,
    name: String,
    guid: Uuid,
    #[serde(rename = "stageForPortalMetrics")]
    stage_for_portal_metrics: Option<serde_json::Value>,
}

// This is far from complete, but this is enough to get us started for creating a game.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StageConfig {
    guid: Option<Uuid>,
    suid: Uuid,
    name: Option<String>,
    #[serde(default)]
    launch: bool,
    script: LocalElement,
    tuning: LocalElement,
    #[serde(rename = "macrosEnabled")]
    macros_enabled: Option<bool>,
    evmlistener: Option<bool>,
    #[serde(rename = "portalWarning")]
    portal_warning: Option<bool>,
    #[serde(default)]
    maintenance: Option<bool>,
    google: Option<Google>,
    data: Option<Data>,
    #[serde(default)]
    #[serde(rename = "cmsUseCDN")]
    cms_use_cdn: Option<bool>,
    cms: Option<String>,
    #[serde(rename = "buildKeyHash")]
    build_key_hash: Option<String>,
    analytics: Option<Analytics>,
    tags: Option<Vec<String>>,
    #[serde(rename = "statusUpdates")]
    status_updates: Option<serde_json::Value>,
    #[serde(rename = "statusUpdateInterval")]
    status_update_interval: Option<serde_json::Value>,
    read_block_delay: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LocalElement {
    link: Option<String>,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Google {}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Data {}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Analytics {}

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
        .tty(false)
        .console_size(ConsoleSize {
            height: 1080,
            width: 1920,
        })
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
                anyhow::bail!("expected stdout chunk, got something else")
            }
        }
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}

pub fn process_rpc_output(output: &str) -> String {
    output
        .trim_start_matches(RPC_START_SEQUENCE)
        .trim()
        .chars()
        // (Note: This probably happened because we allocated a tty)
        // We have a leading "    �" in the output (and I didn't take the time to understand what it is yet)
        // However, JSON should only contain alphanumeric + punctuation, so this solution may be just fine.
        .skip_while(|c| !c.is_ascii_alphanumeric() && !c.is_ascii_punctuation())
        .collect::<String>()
}

// TODO: implement the chunked mechanism...
pub async fn get_msde_config(docker: docker_api::Docker) -> anyhow::Result<Vec<Stages>> {
    let op = rpc(
        docker,
        "Game.configs |> Tuple.to_list |> Enum.at(1) |> Utils.Data.encodeJson!",
    )
    .await?;
    // These transforms are not very pretty and inefficient too, but it works.. sigh
    let op = process_rpc_output(&op);
    let op = op
        .trim_start_matches('"')
        .trim_end_matches('"')
        .replace("\\\"", "\"")
        .replace("\\\\", "\\");

    let stages: Vec<Stages> = serde_json::from_str(&op)?;
    Ok(stages)
}

pub async fn start_stage(docker: docker_api::Docker, guid: Uuid, suid: Uuid) -> anyhow::Result<()> {
    let op = rpc(
        docker,
        format!("Game.sync(\"{guid}\", \"{suid}\", :all) ; :timer.sleep(1000) ; Game.start(\"{guid}\", \"{suid}\")"),
    )
    .await?;
    // TODO: don't print here, return probably
    println!("{}", process_rpc_output(&op));
    Ok(())
}

pub fn start_stages_batch_mapping(
    stage_configs: Vec<Stages>,
) -> anyhow::Result<HashMap<Uuid, Vec<Uuid>>> {
    let mut mapping: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for stage_config in stage_configs {
        let suids: Vec<_> = stage_config
            .stages
            .iter()
            .filter_map(|stage| if stage.launch { Some(stage.suid) } else { None })
            .collect();
        mapping
            .entry(stage_config.guid)
            .or_default()
            .extend(&suids[..])
    }
    Ok(mapping)
}

pub async fn start_stages_batch_commands(stage_configs: Vec<Stages>) -> anyhow::Result<(String, String)> {
    let mapping = start_stages_batch_mapping(stage_configs)?;
    let (sync, start) = mapping.iter().fold(
        (String::new(), String::new()),
        |(mut sync_acc, mut start_acc), (guid, suids)| {
            for suid in suids {
                sync_acc += &format!("Game.sync(\"{guid}\", \"{suid}\", :all) ; ");
                start_acc += &format!("Game.start(\"{guid}\", \"{suid}\") ; ");
            }
            (sync_acc, start_acc)
        },
    );
    Ok((sync, start))
}
