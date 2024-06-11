use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    time::Duration,
};

use anyhow::Context as _;
use backoff::backoff::Backoff;
use docker_api::{
    conn::TtyChunk,
    opts::{ConsoleSize, ExecCreateOpts},
    Docker, Exec,
};
use futures::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    compose::{progress_spinner, running_containers},
    env::Context,
    parsing::{parse_simple_tuple, ElixirTuple, OkVariant},
};

pub const RPC_START_SEQUENCE: &str = "\u{1}\0\0\0\0\0\0\u{8}";

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Stages {
    stages: Vec<StageConfig>,
    org: Option<Uuid>,
    name: String,
    guid: Uuid,
    #[serde(rename = "stageForPortalMetrics")]
    stage_for_portal_metrics: Option<serde_json::Value>,
}

// This is far from complete, but this is enough to get us started for creating or loading a game.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing)]
    disabled_in_stages: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct LocalElement {
    link: Option<String>,
}
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Google {}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Data {}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
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
        // JSON should only contain alphanumeric + punctuation, so this solution may be just fine.
        .skip_while(|c| !c.is_ascii_alphanumeric() && !c.is_ascii_punctuation())
        .collect::<String>()
}

pub async fn get_msde_config(docker: docker_api::Docker) -> anyhow::Result<Vec<Stages>> {
    let op = rpc(
        docker.clone(),
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

    if !op.ends_with("<> ...") {
        tracing::trace!(final_json = %op, "MSDE config concat");
        let stages: Vec<Stages> = serde_json::from_str(&op)?;
        return Ok(stages);
    }
    get_msde_config_chunked(docker).await
}

async fn get_msde_config_chunked(docker: docker_api::Docker) -> anyhow::Result<Vec<Stages>> {
    // The JSON is too big, we ask for it in 3500 character-long chunks (so hopefully it's less than 4096 bytes, since rpc command is limited to that)
    // Arguably I should be using byte size here, but it's too annoying to do behind rpc calls like this one.
    // If we want to be very safe, we should use 1024 as CHUNK_SIZE, since any unicode character is at most 4 bytes, so 4 * 1024 is exactly 4096 and we
    // still get full output - this is very unlikely, because all "frequent" alphanumeric characters and punctuation are usually in the 1 (maybe 2) byte range.
    let mut chunk = 0;
    const CHUNK_SIZE: usize = 3500;
    let mut final_json = String::new();
    loop {
        // A safety measure.. if there're more than 50 chunks, we're just empty-looping and something is inevitably broken.
        if chunk > 50 {
            anyhow::bail!("Failed to get MSDE config.");
        }
        let slice_start = chunk * CHUNK_SIZE;
        let slice_end = (chunk + 1) * CHUNK_SIZE;
        let cmd = format!("Game.configs |> Tuple.to_list |> Enum.at(1) |> Utils.Data.encodeJson! |> String.slice({slice_start}..{slice_end})");
        let next_chunk = rpc(docker.clone(), cmd).await?;
        let next_chunk = process_rpc_output(&next_chunk)
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");

        // The literal empty string means we've reached the end.
        if next_chunk.trim() == "\"\"" {
            tracing::trace!(%final_json, "MSDE config concat");
            let stages = serde_json::from_str(&final_json)?;
            return Ok(stages);
        }
        final_json.push_str(strip_once_chunked(&next_chunk, '"', chunk));
        chunk += 1
    }
}

fn strip_once_chunked(s: &str, chr: char, chunk: usize) -> &str {
    let mut lower = if s.starts_with(chr) { 1 } else { 0 };
    let upper = if s.ends_with(chr) { s.len() - 1 } else { 0 };
    if chunk == 0 {
        return &s[lower..upper];
    }
    // Any other non-first chunk will have an extra overlapping character, since Elixir ranges are *inclusive*.
    // Strip that off.
    lower = if s.starts_with(chr) { lower + 1 } else { lower };
    &s[lower..upper]
}

pub async fn sync_stage_with_ids<'a>(
    docker: docker_api::Docker,
    guid: &'a Uuid,
    suid: &'a Uuid,
) -> anyhow::Result<(String, &'a Uuid, &'a Uuid)> {
    let op = rpc(
        docker,
        format!("Game.sync(\"{guid}\", \"{suid}\", :all) ; "),
    )
    .await?;
    Ok((op, guid, suid))
}

pub async fn start_stage_with_ids<'a>(
    docker: docker_api::Docker,
    guid: &'a Uuid,
    suid: &'a Uuid,
) -> anyhow::Result<(String, &'a Uuid, &'a Uuid)> {
    let op = rpc(docker, format!("Game.start(\"{guid}\", \"{suid}\") ; ")).await?;
    Ok((op, guid, suid))
}

pub fn start_stages_mapping(
    stage_configs: Vec<Stages>,
) -> anyhow::Result<HashMap<Uuid, Vec<Uuid>>> {
    let mut mapping: HashMap<_, Vec<Uuid>> = HashMap::new();
    for stage_config in stage_configs {
        let suids: Vec<_> = stage_config
            .stages
            .iter()
            .filter_map(|stage| {
                if stage.launch && !stage.disabled_in_stages.unwrap_or(false) {
                    Some(stage.suid)
                } else {
                    None
                }
            })
            .collect();
        mapping
            .entry(stage_config.guid)
            .or_default()
            .extend(&suids[..])
    }
    Ok(mapping)
}

pub fn flatten_stage_mapping(
    mapping: &HashMap<Uuid, Vec<Uuid>>,
) -> anyhow::Result<Vec<(&Uuid, &Uuid)>> {
    let pairs = mapping.iter().fold(vec![], |mut acc, (guid, suids)| {
        for suid in suids {
            acc.push((guid, suid));
        }
        acc
    });
    Ok(pairs)
}

pub async fn import_stages(docker: Docker, stages: &[Stages]) -> anyhow::Result<()> {
    // Can't really do it concurrently, since it will overwhelm RPC calls like so:
    // "res was: 10:30:33.852 notice Protocol 'inet_tcp': the name msde_maint_@172.99.0.5 seems to be in use by another Erlang node"
    for stage in stages {
        import_stage(docker.clone(), stage).await?;
    }

    Ok(())
}

async fn import_stage(docker: Docker, stage: &Stages) -> anyhow::Result<()> {
    let json = serde_json::to_string(&stage)?
        .replace("\\", "\\\\")
        .replace("\"", "\\\"");
    let res = rpc(docker.clone(), format!("\"{json}\" |> Game.import()")).await?;
    if process_rpc_output(&res) != ":ok" {
        let suids = stage.stages.iter().map(|s| s.suid).collect::<Vec<_>>();
        tracing::warn!(guid = %stage.guid, suid = ?suids, msg = ?process_rpc_output(&res), "Stage import failed")
    }
    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PackageConfigEntry {
    pub config: PathBuf,
    pub scripts: PathBuf,
    pub tuning: PathBuf,
    pub disabled: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PackageStagesConfig(pub Vec<PackageConfigEntry>);

impl PackageStagesConfig {
    /// If the game name is in Self, return the path of the local_config.yml we can fetch the guid from.
    pub fn try_find_guid_in(&self, game_name: &str) -> Option<&PathBuf> {
        self.0.iter().find_map(|cfg_entry| {
            if cfg_entry.config.starts_with(game_name) {
                Some(&cfg_entry.config)
            } else {
                None
            }
        })
    }
}

// TODO: This name is duplicated
#[derive(Debug, Deserialize, Serialize)]
pub struct PackageLocalConfig {
    pub game: String,
    pub stage: String,
    pub guid: Uuid,
    pub suid: Uuid,
    pub launch: bool,
}

// Probably handle these errors gracefully, except the when the project dir is missing (as warnings maybe?)
pub fn parse_package_local_stages_file(ctx: &Context) -> anyhow::Result<Vec<Stages>> {
    let Some(msde_dir) = ctx.msde_dir.as_ref() else {
        anyhow::bail!("Project dir must be set");
    };
    let stages_file = msde_dir.join("games/stages.yml");
    // The volume is mounted to /usr/local/bin/merigo/games, so we the way the compiler node works we need to step back to the games folder.
    let base_segment = PathBuf::from("../games");
    let stages = fs::read_to_string(&stages_file)
        .with_context(|| format!("stage file missing, should be at {}", stages_file.display()))?;

    let stages: PackageStagesConfig = serde_yaml::from_str(&stages)?;
    let mut stage_configs: Vec<Stages> = vec![];
    for stage in stages.0 {
        let local_cfg = msde_dir.join("games").join(stage.config);
        match fs::read_to_string(&local_cfg) {
            Ok(local) => match serde_yaml::from_str::<PackageLocalConfig>(&local) {
                Ok(package_local_config) => {
                    if let Some(idx) = stage_configs
                        .iter()
                        .position(|sc| sc.guid == package_local_config.guid)
                    {
                        stage_configs
                            .get_mut(idx)
                            .unwrap()
                            .stages
                            .push(StageConfig {
                                suid: package_local_config.suid,
                                guid: Some(package_local_config.guid),
                                launch: package_local_config.launch,
                                name: Some(package_local_config.stage),
                                tuning: LocalElement {
                                    link: Some(
                                        base_segment
                                            .join(stage.tuning)
                                            .to_string_lossy()
                                            .into_owned(),
                                    ),
                                },
                                script: LocalElement {
                                    link: Some(
                                        base_segment
                                            .join(stage.scripts)
                                            .to_string_lossy()
                                            .into_owned(),
                                    ),
                                },
                                disabled_in_stages: stage.disabled,
                                ..Default::default()
                            })
                    } else {
                        stage_configs.push(Stages {
                            stages: vec![StageConfig {
                                suid: package_local_config.suid,
                                guid: Some(package_local_config.guid),
                                launch: package_local_config.launch,
                                name: Some(package_local_config.stage),
                                tuning: LocalElement {
                                    link: Some(
                                        base_segment
                                            .join(stage.tuning)
                                            .to_string_lossy()
                                            .into_owned(),
                                    ),
                                },
                                script: LocalElement {
                                    link: Some(
                                        base_segment
                                            .join(stage.scripts)
                                            .to_string_lossy()
                                            .into_owned(),
                                    ),
                                },
                                disabled_in_stages: stage.disabled,
                                ..Default::default()
                            }],
                            org: None,
                            name: package_local_config.game,
                            guid: package_local_config.guid,
                            ..Default::default()
                        });
                    }
                }
                Err(error) => {
                    tracing::warn!(search_path = %local_cfg.display(), %error, "local_config.yml is invalid")
                }
            },
            Err(error) => {
                tracing::warn!(search_path = %local_cfg.display(), %error, "local_config.yml not found")
            }
        }
    }

    Ok(stage_configs)
}

// The idea there is to first merge based on guid, then deduplicate based on the suid part.
// Kind of ugly, we may clean this up later.
pub fn merge_stages(this: Vec<Stages>, other: Vec<Stages>) -> Vec<Stages> {
    let mut map: HashMap<Uuid, Stages> = HashMap::new();

    let insert_stages = |vec: Vec<Stages>, map: &mut HashMap<Uuid, Stages>| {
        for stages in vec {
            map.entry(stages.guid)
                .and_modify(|existing_stages| existing_stages.stages.extend(stages.stages.clone()))
                .or_insert_with(|| stages.clone());
        }
    };

    insert_stages(this, &mut map);
    insert_stages(other, &mut map);

    map.into_values()
        .map(|mut stages| {
            let mut seen = HashSet::new();
            stages.stages.retain(|stage| seen.insert(stage.suid));
            stages
        })
        .collect()
}

// This function is using streams rather than try_join_all, since it may overwhelm erlang rpc
// calls and we'd get errors about the node being used elsewhere.
// TODO: refactor to use well-defined functions
pub async fn import_games(ctx: &Context, docker: Docker, quiet: bool) -> anyhow::Result<()> {
    let pb = progress_spinner(quiet);
    pb.set_message("🔍 Discovering stages..");
    let local = parse_package_local_stages_file(ctx)?;
    let remote = get_msde_config(docker.clone()).await?;
    let merged_config = merge_stages(local, remote);
    pb.set_message("📥 Importing stages..");
    import_stages(docker.clone(), &merged_config).await?;
    let mapping = start_stages_mapping(merged_config)?;
    let id_pairs = flatten_stage_mapping(&mapping)?;
    if id_pairs.is_empty() {
        pb.finish_with_message("No importable games found. Done.");
        return Ok(());
    }
    pb.set_message("🔁 Starting sync..");
    let mut progress_count = 0;
    let num_of_jobs = id_pairs.len();
    let mut sync_tasks = stream::iter(id_pairs.clone())
        .map(|(guid, suid)| sync_stage_with_ids(docker.clone(), guid, suid));
    let mut sync_job_ids = vec![];
    while let Some(sync_task) = sync_tasks.next().await {
        let (op, guid, suid) = sync_task.await?;
        let op = process_rpc_output(&op);
        pb.set_message(format!(
            "🔁 Starting sync.. {progress_count}/{}",
            num_of_jobs
        ));
        progress_count += 1;
        match parse_simple_tuple(&mut op.as_str()) {
            Ok(ElixirTuple::OkEx(OkVariant::Uuid(uuid))) => sync_job_ids.push((uuid, guid, suid)),
            e => {
                pb.suspend(|| {
                    tracing::warn!(e = ?e, output = ?op, "rpc output was unexpected");
                });
            }
        }
    }

    let mut sync_status = futures::stream::iter(sync_job_ids.clone()).map(|(id, guid, suid)| {
        (
            rpc(docker.clone(), format!("Codify.getSyncJobStatus(\"{id}\")")),
            async move { guid },
            async move { suid },
        )
    });
    let mut results = vec![];
    while let Some((status, guid, suid)) = sync_status.next().await {
        if let Ok(r) = status.await {
            results.push((process_rpc_output(&r), guid.await, suid.await));
        }
    }

    let mut remaining_sync_ids: Vec<_> = results
                .iter()
                .zip(sync_job_ids.iter())
                .filter_map(
                    |((r, guid, suid), job_id)| match parse_simple_tuple(&mut r.as_str()) {
                        Ok(ElixirTuple::OkEx(OkVariant::String(status))) => match status {
                            "Finished" => None,
                            "Verify Error" | "Tuning Error" | "Scripts Error" => {
                                pb.suspend(|| {
                                    tracing::error!(status = ?status, guid = %guid, suid = %suid, "sync failed");
                                });
                                None
                            }
                            // These are not completed yet.
                            _ => Some(job_id),
                        },
                        e => {
                            pb.suspend(|| {
                                tracing::warn!(e = ?e, output = ?r, "rpc output was unexpected");
                            });

                            None
                        }
                    },
                )
                .collect();

    let mut backoff = backoff::ExponentialBackoffBuilder::new()
        .with_max_elapsed_time(Some(Duration::from_secs(30)))
        .build();

    while !remaining_sync_ids.is_empty() {
        let Some(backoff_duration) = backoff.next_backoff() else {
            tracing::error!(ids = ?remaining_sync_ids, "No backoff left, some sync jobs failed to complete in time.");
            break;
        };

        tokio::time::sleep(backoff_duration).await;

        let mut sync_status =
            futures::stream::iter(remaining_sync_ids.clone()).map(|(id, guid, suid)| {
                (
                    rpc(docker.clone(), format!("Codify.getSyncJobStatus(\"{id}\")")),
                    async move { guid },
                    async move { suid },
                )
            });
        let mut new_sync_results = vec![];
        while let Some((status, guid, suid)) = sync_status.next().await {
            if let Ok(r) = status.await {
                new_sync_results.push((process_rpc_output(&r), guid.await, suid.await));
            }
        }

        remaining_sync_ids = new_sync_results
            .iter()
            .zip(remaining_sync_ids.into_iter())
            .filter_map(|((r, guid, suid), job_id)| {
                match parse_simple_tuple(&mut r.as_str()) {
                    Ok(ElixirTuple::OkEx(OkVariant::String(status))) => match status {
                        "Finished" => None,
                        // In a backoff situation, if "Setting Up script File System" is still in progress, that means it's stuck cause
                        // the folder doesn't exist or something.
                        // Arguably we should handle this better in MSDE, but let's handle this here for now..
                        "Verify Error"
                        | "Tuning Error"
                        | "Scripts Error"
                        | "Setting Up script File System" => {
                            pb.suspend(|| {
                                tracing::error!(status = ?status, %guid, %suid, "sync failed");
                            });
                            None
                        }
                        // These are not completed yet.
                        _ => Some(job_id),
                    },
                    e => {
                        pb.suspend(|| {
                            tracing::warn!(e = ?e, output = ?r, "rpc output was unexpected");
                        });
                        None
                    }
                }
            })
            .collect();
    }

    pb.set_message("🚀 Launching stages..");
    let mut progress_count = 0;
    let mut start_tasks =
        stream::iter(id_pairs).map(|(guid, suid)| start_stage_with_ids(docker.clone(), guid, suid));
    let mut success = true;
    while let Some(sync_task) = start_tasks.next().await {
        pb.set_message(format!(
            "🚀 Launching stages.. {progress_count}/{}",
            num_of_jobs
        ));
        progress_count += 1;
        let (op, guid, suid) = sync_task.await?;
        let op = process_rpc_output(&op);
        // FIXME: Parsing this properly is a pain, because we may get output from the Job script like this:
        // "[36m09:12:13.597 debug [Job.Script] Crashed reading types(), or no types defined %ArgumentError{message: \"argument error\"}\n\u{1b}[0m:ok"
        if !matches!(
            parse_simple_tuple(&mut op.as_str()),
            Ok(ElixirTuple::ErrorEx("game_running"))
        ) && !op.ends_with(":ok")
        {
            success = false;
            pb.suspend(|| {
                tracing::warn!(output = ?op, %guid, %suid, "starting stage failed");
            });
        }
    }
    pb.finish_with_message("Done.");
    if !success {
        tracing::warn!("Failed to start some stages. Consider running `msde-cli log compiler` in a different terminal and try again.");
    }
    Ok(())
}
