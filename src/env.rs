//! This module takes care of setting up the msde binary's environment.
//!
//! The order of precedence is
//! - environment variables
//! - passed cli arguments (if exists)
//! - msde config file
//! - a sensible default (if exists)

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
}

// TODO: fields
#[derive(Debug)]
pub struct Authorization;

impl Context {
    pub fn from_env() -> Self {
        let home = match home::home_dir() {
            Some(path) if !path.as_os_str().is_empty() => path,
            _ => panic!("failed to determine home directory"),
        };
        let config_dir = home.join(".msde");
        std::fs::create_dir_all(&config_dir).unwrap();
        let (msde_dir, dir_set) = msde_dir(home).expect("to be valid");
        Self {
            config_dir,
            msde_dir,
            version: None,
            authorization: None,
            dir_set,
        }
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
}
