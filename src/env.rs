//! This module takes care of setting up the msde binary's environment.
//!
//! The order of precedence is
//! - environment variables
//! - passed cli arguments (if exists)
//! - msde config file
//! - a sensible default (if exists)

use anyhow::Context as _;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use strum::Display;

use crate::{
    compose::{
        DOCKER_COMPOSE_BOT, DOCKER_COMPOSE_METRICS, DOCKER_COMPOSE_OTEL, DOCKER_COMPOSE_WEB3,
    },
    hooks::Hooks,
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

// This is a helper that preserves *important* config values that are essential to deserialize, even if other things fail..
#[derive(serde::Deserialize, serde::Serialize, Debug, Default, Clone)]
pub struct ConfigStatic {
    #[serde(rename = "MERIGO_DEV_PACKAGE_DIR")]
    pub merigo_dev_package_dir: Option<PathBuf>,
}

impl From<ConfigStatic> for Config {
    fn from(value: ConfigStatic) -> Self {
        Config {
            merigo_dev_package_dir: value.merigo_dev_package_dir,
            ..Default::default()
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
#[serde(transparent)]
pub struct Profiles(pub HashMap<String, Vec<Feature>>);

impl Default for Profiles {
    fn default() -> Self {
        let mut hm = HashMap::new();
        hm.insert("minimal".into(), vec![]);
        hm.insert("default".into(), vec![Feature::Metrics, Feature::Web3]);
        hm.insert(
            "full".into(),
            vec![
                Feature::Metrics,
                Feature::Web3,
                Feature::OTEL,
                Feature::Metrics,
            ],
        );

        Self(hm)
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

impl Feature {
    pub fn from_primitive(primitive: usize) -> anyhow::Result<Self> {
        match primitive {
            0 => Ok(Self::Metrics),
            1 => Ok(Self::OTEL),
            2 => Ok(Self::Web3),
            3 => Ok(Self::Bot),
            _ => anyhow::bail!("Invalid primitive, failed to convert integer to Feature."),
        }
    }

    pub fn required_images_and_tags(&self) -> Vec<(String, String)> {
        match self {
            Feature::Metrics => {
                vec![(String::from("prom/prometheus"), String::from("v2.45.0"))]
            }
            Feature::OTEL => {
                let stack_version =
                    std::env::var("STACK_VERSION").unwrap_or_else(|_| String::from("8.7.1"));
                vec![
                    (
                        String::from("otel/opentelemetry-collector-contrib"),
                        String::from("0.97.0"),
                    ),
                    (
                        String::from("docker.elastic.co/apm/apm-server"),
                        stack_version.clone(),
                    ),
                    (
                        String::from("docker.elastic.co/elasticsearch/elasticsearch"),
                        stack_version.clone(),
                    ),
                    (
                        String::from("docker.elastic.co/kibana/kibana"),
                        stack_version.clone(),
                    ),
                    (
                        String::from("docker.elastic.co/beats/filebeat"),
                        stack_version.clone(),
                    ),
                    (
                        String::from("docker.elastic.co/logstash/logstash"),
                        stack_version,
                    ),
                ]
            }
            // TODO: These have internal image components that require auth.
            Feature::Web3 => vec![(
                String::from("softwaremill/elasticmq"),
                String::from("latest"),
            )],
            Feature::Bot => vec![],
        }
    }
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
    pub hooks: Option<Hooks>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Authorization {
    pub token: String,
}

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

    // If the file is broken (maybe it uses the older scheme) this function handles that migration part too.
    pub fn write_profiles(&self, name: String, features: Vec<Feature>) -> anyhow::Result<()> {
        let config_file = self.config_dir.join("config.json");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .open(&config_file)?;

        let mut buf = String::new();
        let _bytes_read = f.read_to_string(&mut buf)?;

        let cfg = match serde_json::from_str::<Config>(&buf) {
            Ok(mut cfg) => {
                cfg.profiles
                    .0
                    .entry(name)
                    .and_modify(|f| f.clone_from(&features))
                    .or_insert(features);
                cfg
            }
            Err(_) => match serde_json::from_str::<ConfigStatic>(&buf) {
                Ok(cfg_static) => {
                    let mut cfg = Config::from(cfg_static);
                    cfg.profiles.0.insert(name, features);
                    cfg
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Invalid config file format, failed to preserve project path.");
                    let mut cfg = Config::default();
                    cfg.profiles.0.insert(name, features);
                    cfg
                }
            },
        };

        let f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&config_file)?;
        let mut writer = std::io::BufWriter::new(f);

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
                hooks: None,
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
    ) -> Result<Option<PackageLocalConfig>, ProjectCheckErrors> {
        let Some(msde_dir) = self.msde_dir.as_ref() else {
            return Ok(None);
        };
        let metadata_file = msde_dir.join("./metadata.json");

        let f = fs::read_to_string(metadata_file)?;

        let metadata = serde_json::from_str::<PackageLocalConfig>(&f)?;

        let project_version = semver::Version::parse(&metadata.self_version)?;
        if project_version != self_version {
            return Err(ProjectCheckErrors::VersionMismatch(
                project_version,
                self_version,
            ));
        }
        Ok(Some(metadata))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectCheckErrors {
    #[error("metadata.json file is missing")]
    MissingMetadata(#[from] std::io::Error),
    #[error("metadata.json file is invalid: {0}")]
    InvalidMetadata(#[from] serde_json::Error),
    #[error("Project is outdated: project version is {0}, but CLI is version {1}")]
    VersionMismatch(semver::Version, semver::Version),
    #[error("Invalid project version in metadata.json")]
    InvalidVersion(#[from] semver::Error),
}
