//! This module takes care of setting up the msde binary's environment.
//!
//! The order of precedence is
//! - environment variables
//! - passed cli arguments (if exists)
//! - msde config file
//! - a sensible default (if exists)

use anyhow::Context as _;
use std::{
    fs::File,
    io::{BufReader, Write},
    path::PathBuf,
};

pub fn home() -> anyhow::Result<PathBuf> {
    match home::home_dir() {
        Some(path) if !path.as_os_str().is_empty() => Ok(path),
        _ => anyhow::bail!("failed to determine home directory"),
    }
}

pub fn msde_dir(home: PathBuf) -> anyhow::Result<(PathBuf, bool)> {
    let mut dir_set = true;
    let path = std::env::var("MERIGO_DEV_PACKAGE_DIR")
        .map(PathBuf::from)
        .or_else(|_| {
            // TODO: Don't open and deserialize this file here..
            let config = home.join(".msde/config.json");
            let f = File::open(config)?;
            let reader = BufReader::new(f);
            let config: Config = serde_json::from_reader(reader)?;

            config
                .merigo_dev_package_dir
                .map(|p| p.canonicalize())
                .ok_or(anyhow::Error::msg("invalid config"))
                .map_err(|_| anyhow::Error::msg("invalid path"))?
                .map_err(|_| anyhow::Error::msg("invalid config"))
        })
        .or_else(|_: anyhow::Error| {
            dir_set = false;
            Ok(home.join("merigo"))
        });
    path.map(|p| (p, dir_set))
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct Config {
    #[serde(rename = "MERIGO_DEV_PACKAGE_DIR")]
    pub merigo_dev_package_dir: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Context {
    pub config_dir: PathBuf,
    pub msde_dir: PathBuf,
    pub version: Option<semver::Version>,
    pub authorization: Option<Authorization>,
    /// Whether the working directory was explicitly set by the user by any means.
    pub dir_set: bool,
    // TODO: read this in init, if exists.
    pub config: Option<Config>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PackageLocalConfig {
    pub target_msde_version: Option<String>,
    pub self_version: String,
    pub timestamp: i64,
}

// TODO: fields
#[derive(Debug)]
pub struct Authorization;

impl Context {
    pub fn from_env() -> anyhow::Result<Self> {
        let home = home()?;
        let config_dir = home.join(".msde");
        std::fs::create_dir_all(&config_dir).context("Failed to create config directory")?;
        let (msde_dir, dir_set) = msde_dir(home).expect("to be valid");
        Ok(Self {
            config_dir,
            msde_dir,
            version: None,
            authorization: None,
            dir_set,
            config: None,
        })
    }

    pub fn clean(&self) {
        std::fs::remove_dir_all(&self.config_dir).unwrap();
    }

    // TODO: Read if exists, and modify (maybe not even here, we should load Config into memory in init)
    pub fn write_config(&self, project_path: PathBuf) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.config_dir)?;
        let config_file = self.config_dir.join("config.json");
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(config_file)?;

        let mut writer = std::io::BufWriter::new(f);

        serde_json::to_writer(
            &mut writer,
            &Config {
                merigo_dev_package_dir: Some(project_path),
            },
        )?;
        writer.flush()?;
        Ok(())
    }

    pub fn write_package_local_config(&self, self_version: semver::Version) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.msde_dir)?;
        let config_file = self.msde_dir.join("metadata.json");
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(config_file)?;

        let mut writer = std::io::BufWriter::new(f);

        serde_json::to_writer(
            &mut writer,
            &PackageLocalConfig {
                target_msde_version: Some("3.10.0".into()), // TODO: Do not hardcode
                self_version: self_version.to_string(),
                timestamp: time::OffsetDateTime::now_utc().unix_timestamp(),
            },
        )?;
        writer.flush()?;
        Ok(())
    }

    pub fn set_project_path(&mut self, project_path: &PathBuf) {
        self.msde_dir = project_path.clone();
    }
}