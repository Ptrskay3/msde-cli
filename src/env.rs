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
    fs::File,
    io::{BufReader, Seek, Write},
    path::{Path, PathBuf},
};

pub fn home() -> anyhow::Result<PathBuf> {
    match home::home_dir() {
        Some(path) if !path.as_os_str().is_empty() => Ok(path),
        _ => anyhow::bail!("failed to determine home directory"),
    }
}

// TODO: Accept the config file as argument, don't open it here.
pub fn msde_dir(home: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
    std::env::var("MERIGO_DEV_PACKAGE_DIR")
        .map(PathBuf::from)
        .or_else(|_| {
            // TODO: Don't open and deserialize this file here..
            let config = home.as_ref().join(".msde/config.json");
            let f = File::open(config)?;
            let reader = BufReader::new(f);
            let config: Config = serde_json::from_reader(reader)?;

            // TODO: This implicitly checks whether the path exists.
            // Not sure this is good or bad..
            config
                .merigo_dev_package_dir
                .map(|p| p.canonicalize())
                .ok_or(anyhow::Error::msg("invalid config"))
                .map_err(|_| anyhow::Error::msg("invalid path"))?
                .map_err(|_| anyhow::Error::msg("invalid config"))
        })
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct Config {
    #[serde(rename = "MERIGO_DEV_PACKAGE_DIR")]
    pub merigo_dev_package_dir: Option<PathBuf>,
    pub profiles: Profiles,
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
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
                features: vec![Feature::Metrics],
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

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct ProfileSpec {
    name: String,
    features: Vec<Feature>,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Feature {
    OTEL,
    Metrics,
    Web3,
    Bot,
}

#[derive(Debug)]
pub struct Context {
    pub home: PathBuf,
    pub config_dir: PathBuf,
    pub msde_dir: Option<PathBuf>,
    pub version: Option<semver::Version>,
    pub authorization: Option<Authorization>,
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
        std::fs::create_dir_all(&config_dir).with_context(|| {
            format!(
                "Failed to create config directory at {}",
                config_dir.display()
            )
        })?;
        let msde_dir = msde_dir(&home).ok();
        let config = {
            let config_file = config_dir.join("config.json");
            if let Ok(f) = File::open(config_file) {
                let reader = std::io::BufReader::new(f);
                match serde_json::from_reader(reader) {
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
    // TODO: Read if exists, and modify (maybe not even here, we should load Config into memory in init)
    // if we load it into memory on init, we may just pass the current value down from main to here, so we don't need to read again.
    pub fn write_config(&self, project_path: PathBuf) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.config_dir)?;
        let config_file = self.config_dir.join("config.json");
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true) // TODO: Truncating until we properly read and modify this file.
            .open(config_file)?;

        let mut writer = std::io::BufWriter::new(f);

        serde_json::to_writer(
            &mut writer,
            &Config {
                merigo_dev_package_dir: Some(project_path),
                // TODO: don't always write the default
                profiles: Default::default(),
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
        let f = std::fs::OpenOptions::new().read(true).open(metadata_file)?;

        let reader = std::io::BufReader::new(f);
        let metadata = serde_json::from_reader::<_, PackageLocalConfig>(reader)
            .context("metadata file is invalid")?;

        let project_version = semver::Version::parse(&metadata.self_version)?;
        if project_version != self_version {
            anyhow::bail!(
                "Project is version {project_version}, but CLI is version {self_version}."
            )
        }
        Ok(Some(metadata))
    }
}
