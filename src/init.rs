pub fn ensure_access() -> anyhow::Result<()> {
    todo!()
}

pub async fn ensure_docker(docker: &docker_api::Docker) -> anyhow::Result<()> {
    docker.version().await.map(|_| ()).map_err(|e| {
        tracing::error!("Failed to connect Docker daemon");
        Into::into(e)
    })
}

pub fn ensure_project() -> anyhow::Result<()> {
    todo!()
}
