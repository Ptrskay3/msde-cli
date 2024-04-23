use md5::{Digest, Md5};
use std::fs;
use std::io::{self, Read};
use std::path::Path;

pub fn md5_update_from_dir(directory: &Path, mut hash: Md5) -> io::Result<Md5> {
    assert!(directory.is_dir());

    let mut paths: Vec<_> = fs::read_dir(directory)?
        .map(|res| res.expect("insufficient permissions").path())
        .collect();
    paths.sort_by(|a, b| {
        a.to_string_lossy()
            .to_lowercase()
            .cmp(&b.to_string_lossy().to_lowercase())
    });

    for path in paths {
        hash.update(path.file_name().unwrap().to_string_lossy().as_bytes());

        if path.is_file() {
            let mut file = fs::File::open(&path)?;
            let mut buffer = [0; 4096];
            loop {
                let bytes_read = file.read(&mut buffer)?;
                if bytes_read == 0 {
                    break;
                }
                hash.update(&buffer[0..bytes_read]);
            }
        } else if path.is_dir() {
            hash = md5_update_from_dir(&path, hash)?;
        }
    }
    Ok(hash)
}

pub fn md5_dir(directory: &Path) -> io::Result<String> {
    let hasher = Md5::new();
    let hasher = md5_update_from_dir(directory, hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}


pub fn update_self() {}
pub fn update_server() {}
pub fn update_beam_files() {}

static DEFAULT_UPDATE_ROOT: &str = "TODO: an url to download this CLI tool";

fn update_root() -> String {
    std::env::var("MSDE_UPDATE_ROOT")
        .inspect(|path| tracing::debug!("`MSDE_UPDATE_ROOT` has been set to `{path}`"))
        .unwrap_or_else(|_| String::from(DEFAULT_UPDATE_ROOT))
}
