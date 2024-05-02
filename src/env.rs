//! This module takes care of setting up the msde binary's environment.

use std::path::PathBuf;

pub fn msde_dir() -> PathBuf {
    std::env::var("MERIGO_DEV_PACKAGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = match home::home_dir() {
                Some(path) if !path.as_os_str().is_empty() => path,
                _ => panic!("failed to determine home directory"),
            };

            home.join("merigo")
        })
}

#[derive(Debug)]
pub struct Context {
    config_dir: PathBuf,
    msde_dir: PathBuf,
    version: Option<semver::Version>,
    authorization: Option<Authorization>,
}

// TODO: fields
#[derive(Debug)]
pub struct Authorization;

impl Context {
    pub fn init_from_env() -> Self {
        let home = match home::home_dir() {
            Some(path) if !path.as_os_str().is_empty() => path,
            _ => panic!("failed to determine home directory"),
        };
        let config_dir = home.join(".msde");
        std::fs::create_dir_all(&config_dir).unwrap();

        Self {
            config_dir,
            msde_dir: msde_dir(),
            version: None,
            authorization: None,
        }
    }
}
