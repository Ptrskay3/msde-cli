use std::path::PathBuf;

pub fn ensure_access() -> anyhow::Result<()> {
    todo!()
}

pub async fn ensure_docker(docker: &docker_api::Docker) -> anyhow::Result<()> {
    docker.ping().await.map(|_| ()).map_err(|e| {
        tracing::error!("Failed to connect Docker daemon");
        Into::into(e)
    })
}

pub fn ensure_project() -> anyhow::Result<()> {
    todo!()
}

pub fn ensure_valid_project_path(path: &PathBuf, force: bool) -> anyhow::Result<()> {
    if path.exists() {
        if path.is_dir() && path.read_dir()?.next().is_some() && !force {
            anyhow::bail!("The given directory is not empty.")
        } else if !path.is_dir() {
            anyhow::bail!("The given path is not a directory.")
        }
    }
    Ok(())
}
