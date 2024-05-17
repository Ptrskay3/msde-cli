//! This module takes care of setting up the msde binary's environment.
//!
//! The order of precedence is
//! - environment variables
//! - passed cli arguments (if exists)
//! - msde config file
//! - a sensible default (if exists)

use anyhow::Context as _;
use clap::ValueEnum;
use std::{
    fs,
    io::{BufReader, Seek, Write},
    path::{Path, PathBuf},
};
use strum::Display;

use crate::compose::{
    DOCKER_COMPOSE_BOT, DOCKER_COMPOSE_METRICS, DOCKER_COMPOSE_OTEL, DOCKER_COMPOSE_WEB3,
};

pub fn home() -> anyhow::Result<PathBuf> {
    match home::home_dir() {
        Some(path) if !path.as_os_str().is_empty() => Ok(path),
        _ => anyhow::bail!("failed to determine home directory"),
    }
}

pub fn msde_dir(config: Option<&Config>) -> anyhow::Result<PathBuf> {
    std::env::var("MERIGO_DEV_PACKAGE_DIR")
        .map(PathBuf::from)
        .or_else(|_| {
            config
                .and_then(|c| c.merigo_dev_package_dir.clone())
                .map(|path| path.canonicalize().map_err(Into::into))
                .ok_or_else(|| anyhow::Error::msg("unspecified project path"))?
        })
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Default, Clone)]
pub struct Config {
    #[serde(rename = "MERIGO_DEV_PACKAGE_DIR")]
    pub merigo_dev_package_dir: Option<PathBuf>,
    pub profiles: Profiles,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
#[serde(transparent)]
pub struct Profiles(Vec<ProfileSpec>);

impl Default for Profiles {
    fn default() -> Self {
        Self(vec![
            ProfileSpec {
                name: "minimal".into(),
                features: vec![],
            },
            ProfileSpec {
                name: "default".into(),
                features: vec![Feature::Metrics, Feature::Web3],
            },
            ProfileSpec {
                name: "full".into(),
                features: vec![
                    Feature::Metrics,
                    Feature::Web3,
                    Feature::OTEL,
                    Feature::Metrics,
                ],
            },
        ])
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct ProfileSpec {
    name: String,
    features: Vec<Feature>,
}

#[derive(
    serde::Deserialize,
    serde::Serialize,
    Debug,
    Clone,
    ValueEnum,
    Display,
    PartialEq,
    PartialOrd,
    Eq,
    Ord,
)]
#[serde(rename_all = "lowercase")]
pub enum Feature {
    Metrics = 1,
    OTEL = 2,
    Web3 = 3,
    Bot = 4,
}

#[derive(
    serde::Deserialize,
    serde::Serialize,
    Debug,
    Clone,
    ValueEnum,
    Display,
    PartialEq,
    PartialOrd,
    Eq,
    Ord,
)]
#[serde(rename_all = "lowercase")]
pub enum ExtendedFeature {
    Metrics = 1,
    OTEL = 2,
    Web3 = 3,
    Bot = 4,
    MSDE = 5,
    Base = 6,
}

impl ExtendedFeature {
    pub fn wait_target(&self) -> &str {
        match self {
            ExtendedFeature::Base => "/consul-vm-dev",
            ExtendedFeature::Metrics => "/grafana-vm-dev",
            ExtendedFeature::OTEL => "/kibana",
            ExtendedFeature::Web3 => "/web3-vm-dev",
            ExtendedFeature::Bot => "/msde-vm-dev", // Not a typo!
            ExtendedFeature::MSDE => "/msde-vm-dev",
        }
    }
}

impl From<Feature> for ExtendedFeature {
    fn from(value: Feature) -> Self {
        match value {
            Feature::Metrics => Self::Metrics,
            Feature::OTEL => Self::OTEL,
            Feature::Web3 => Self::Web3,
            Feature::Bot => Self::Bot,
        }
    }
}

impl Feature {
    pub fn to_target(&self) -> &'static str {
        match self {
            Feature::OTEL => DOCKER_COMPOSE_OTEL,
            Feature::Metrics => DOCKER_COMPOSE_METRICS,
            Feature::Web3 => DOCKER_COMPOSE_WEB3,
            Feature::Bot => DOCKER_COMPOSE_BOT,
        }
    }
}

#[derive(Debug)]
pub struct Context {
    pub home: PathBuf,
    pub config_dir: PathBuf,
    pub msde_dir: Option<PathBuf>,
    pub version: Option<semver::Version>,
    pub authorization: Option<Authorization>,
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
        std::fs::create_dir_all(&config_dir).with_context(|| {
            format!(
                "Failed to create config directory at {}",
                config_dir.display()
            )
        })?;
        let config = {
            let config_file = config_dir.join("config.json");
            if let Ok(f) = fs::read_to_string(config_file) {
                match serde_json::from_str(&f) {
                    Ok(config) => Some(config),
                    Err(e) => {
                        tracing::debug!(error = %e, "config file seems to be broken.");
                        None
                    }
                }
            } else {
                None
            }
        };
        let msde_dir = msde_dir(config.as_ref()).ok();

        Ok(Self {
            home,
            config_dir,
            msde_dir,
            version: None,
            authorization: None,
            config,
        })
    }

    pub fn explicit_project_path(&self) -> Option<&PathBuf> {
        self.msde_dir.as_ref()
    }

    pub fn clean(&self) {
        std::fs::remove_dir_all(&self.config_dir).unwrap();
    }

    pub fn write_profiles(&self, name: String, features: Vec<Feature>) -> anyhow::Result<()> {
        let config_file = self.config_dir.join("config.json");
        let f = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .open(config_file)?;

        let current = BufReader::new(&f);
        let mut cfg: Config = serde_json::from_reader(current)?;
        cfg.profiles.0.push(ProfileSpec { name, features });
        let mut writer = std::io::BufWriter::new(f);
        writer.rewind()?;

        serde_json::to_writer(&mut writer, &cfg)?;
        writer.flush()?;
        Ok(())
    }

    pub fn write_config(&self, project_path: PathBuf) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.config_dir)?;
        let config_file = self.config_dir.join("config.json");
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(config_file)?;

        let mut writer = std::io::BufWriter::new(f);

        serde_json::to_writer(
            &mut writer,
            &Config {
                merigo_dev_package_dir: Some(project_path),
                ..self.config.clone().unwrap_or_default()
            },
        )?;
        writer.flush()?;
        Ok(())
    }

    pub fn write_package_local_config(&self, self_version: semver::Version) -> anyhow::Result<()> {
        let msde_dir = self
            .msde_dir
            .as_ref()
            .context("Package location is unknown")?;
        std::fs::create_dir_all(msde_dir)?;
        let config_file = msde_dir.join("metadata.json");
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

    pub fn set_project_path(&mut self, project_path: impl AsRef<Path>) {
        self.msde_dir = Some(project_path.as_ref().to_path_buf())
    }

    pub fn run_project_checks(
        &self,
        self_version: semver::Version,
    ) -> anyhow::Result<Option<PackageLocalConfig>> {
        let Some(msde_dir) = self.msde_dir.as_ref() else {
            return Ok(None);
        };
        let metadata_file = msde_dir.join("./metadata.json");
        anyhow::ensure!(metadata_file.exists(), "metadata file is missing");
        let f = fs::read_to_string(metadata_file)?;

        let metadata =
            serde_json::from_str::<PackageLocalConfig>(&f).context("metadata file is invalid")?;

        let project_version = semver::Version::parse(&metadata.self_version)?;
        if project_version != self_version {
            anyhow::bail!(
                "Project is version {project_version}, but CLI is version {self_version}."
            )
        }
        Ok(Some(metadata))
    }
}
