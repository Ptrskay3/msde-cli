use md5::{Digest, Md5};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use zip_extensions::*;

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

#[tracing::instrument]
pub fn verify_beam_files<P: AsRef<Path> + std::fmt::Debug>(
    vsn: semver::Version,
    ext_priv_dir: P,
) -> anyhow::Result<()> {
    let beam_dir = ext_priv_dir.as_ref().join("beam_files");
    anyhow::ensure!(
        beam_dir.is_dir(),
        "The Merigo extension is missing. Run win the `--no-verify` flag to bypass."
    );
    let current_checksum = md5_dir(&beam_dir)?;
    let mut buf = String::new();
    let mut f = std::fs::File::open(ext_priv_dir.as_ref().join("checksum.txt"))?;
    f.read_to_string(&mut buf)?;
    let Some((version, checksum)) = buf.split_once(':') else {
        anyhow::bail!("invalid checksum file")
    };
    let version = semver::Version::parse(version)?;

    let success = match (version == vsn, checksum.trim() == current_checksum.trim()) {
        (true, true) => true,
        (false, _) => {
            tracing::warn!("BEAM files are built for version {version}, but you're running MSDE with version {vsn}.");
            false
        }
        (_, false) => {
            tracing::warn!( "BEAM files are not verifying against the original checksum, they might be incomplete");
            false
        }
    };
    if !success {
        let msg = "To bypass the validation part, pass the `--no-verify` flag.";
        tracing::warn!(msg);
        anyhow::bail!(msg)
    };
    Ok(())
}

// TODO: Paths..
#[tracing::instrument]
pub async fn update_beam_files(version: semver::Version, no_verify: bool) -> anyhow::Result<()> {
    let response = reqwest::get(format!(
        "https://merigo-beam-files.s3.amazonaws.com/{version}/merigo-extension.zip"
    ))
    .await?;

    if response.status() != 200 {
        tracing::trace!("response was {}", response.text().await.unwrap());
        anyhow::bail!("Failed to pull the Merigo extension, probably because it doesn't exist for version `{version}`");
    }

    let body = response.bytes().await?;

    let mut tmp_file = File::create("merigo-extension-tmp.zip")?;
    io::copy(&mut body.as_ref(), &mut tmp_file)?;
    let archive_file = PathBuf::from("merigo-extension-tmp.zip");
    let target_dir = PathBuf::from("./merigo-extension-tmp");
    tracing::trace!(path = ?target_dir, "extracting zip");
    zip_extract(&archive_file, &target_dir)?;
    if !no_verify {
        verify_beam_files(version, "./merigo-extension-tmp")?;
    }
    tracing::trace!("Copying BEAM files to their real destination..");
    std::fs::remove_dir_all("./merigo-extension-real")?;
    fs_extra::move_items(
        &["./merigo-extension-tmp"],
        "./merigo-extension-real",
        &fs_extra::dir::CopyOptions {
            copy_inside: true,
            ..Default::default()
        },
    )?;
    tracing::trace!("Removing temporal zip.");

    std::fs::remove_file("./merigo-extension-tmp.zip")?;
    tracing::trace!("Done.");
    Ok(())
}