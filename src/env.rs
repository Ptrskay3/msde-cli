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
