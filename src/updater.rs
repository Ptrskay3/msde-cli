use md5::{Digest, Md5};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use zip_extensions::*;

use crate::env::Context;
use crate::MERIGO_EXTENSION;

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
        anyhow::bail!("invalid checksum file, file did not contain a ':'")
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

#[tracing::instrument]
pub async fn update_beam_files(
    ctx: &Context,
    version: semver::Version,
    no_verify: bool,
) -> anyhow::Result<()> {
    const MERIGO_EXTENSION_TMP_ZIP: &str = "merigo-extension-tmp.zip";
    let Some(msde_dir) = ctx.msde_dir.as_ref() else {
        anyhow::bail!("No active project found.");
    };
    let response = reqwest::get(format!(
        "https://merigo-beam-files.s3.amazonaws.com/{version}/merigo-extension.zip"
    ))
    .await?;

    if response.status() != 200 {
        tracing::trace!("response was {}", response.text().await.unwrap());
        anyhow::bail!("Failed to pull the Merigo extension, probably because it doesn't exist for version `{version}`");
    }

    let body = response.bytes().await?;

    let mut tmp_file = File::create(msde_dir.join(MERIGO_EXTENSION_TMP_ZIP))?;
    io::copy(&mut body.as_ref(), &mut tmp_file)?;
    tracing::trace!(path = ?msde_dir, "extracting zip");
    zip_extract(
        &msde_dir.join(MERIGO_EXTENSION_TMP_ZIP),
        &msde_dir.join("merigo-extension-tmp"),
    )?;
    if !no_verify {
        verify_beam_files(version, msde_dir.join("merigo-extension-tmp"))?;
    }
    tracing::trace!("Copying BEAM files to their real destination..");
    // Ignoring the error, because it may not exist.
    let _ = std::fs::remove_dir_all(msde_dir.join(MERIGO_EXTENSION));
    fs_extra::move_items(
        &[msde_dir.join("merigo-extension-tmp")],
        msde_dir.join(MERIGO_EXTENSION),
        &fs_extra::dir::CopyOptions {
            copy_inside: true,
            ..Default::default()
        },
    )?;
    tracing::trace!("Removing temporal zip.");

    std::fs::remove_file(msde_dir.join(MERIGO_EXTENSION_TMP_ZIP))?;
    tracing::trace!("Done.");
    Ok(())
}

pub struct PackageUpgradePipeline {
    pub steps: Vec<PackageUpgradeStep>,
}

impl PackageUpgradePipeline {
    pub fn empty() -> Self {
        Self { steps: Vec::new() }
    }

    pub fn with_default_version_writer(self_version: semver::Version) -> Self {
        Self {
            steps: vec![PackageUpgradeStep::Auto(Auto {
                f: Box::new(move |ctx: &Context| -> anyhow::Result<()> {
                    ctx.upgrade_package_local_version(self_version)?;
                    Ok(())
                }),
            })],
        }
    }

    pub fn run(self, context: &Context) -> anyhow::Result<()> {
        for step in self.steps {
            step.perform(context)?;
        }
        Ok(())
    }

    pub fn push_auto<F>(&mut self, f: F)
    where
        F: FnOnce(&Context) -> anyhow::Result<()> + 'static,
    {
        self.steps
            .push(PackageUpgradeStep::Auto(Auto { f: Box::new(f) }));
    }

    pub fn push_manual(&mut self, display_msg: String) {
        self.steps
            .push(PackageUpgradeStep::Manual(Manual { display_msg }))
    }
}

pub enum PackageUpgradeStep {
    Auto(Auto),
    Manual(Manual),
}

impl PerformStep for PackageUpgradeStep {
    fn perform(self, context: &Context) -> anyhow::Result<()> {
        match self {
            PackageUpgradeStep::Auto(a) => a.perform(context),
            PackageUpgradeStep::Manual(m) => m.perform(context),
        }
    }
}

pub trait PerformStep {
    fn perform(self, context: &Context) -> anyhow::Result<()>;
}

pub struct Auto {
    f: Box<dyn FnOnce(&Context) -> anyhow::Result<()>>,
}

impl PerformStep for Auto {
    fn perform(self, context: &Context) -> anyhow::Result<()> {
        (self.f)(context)?;
        Ok(())
    }
}

pub struct Manual {
    display_msg: String,
}

impl PerformStep for Manual {
    fn perform(self, _context: &Context) -> anyhow::Result<()> {
        println!("{}", self.display_msg);
        Ok(())
    }
}

pub fn matrix(
    current: semver::Version,
    project: semver::Version,
    ctx: &Context,
) -> anyhow::Result<()> {
    match current.cmp(&project) {
        std::cmp::Ordering::Less => {
            tracing::info!("You're trying to downgrade the project. Consider installing an older version of `msde-cli`.");
            Ok(())
        }
        std::cmp::Ordering::Equal => {
            tracing::info!("Up to date.");
            Ok(())
        }
        std::cmp::Ordering::Greater => {
            // Actually perform the upgrade steps.
            // TODO: This is just an example.
            let pipeline = PackageUpgradePipeline::with_default_version_writer(current.clone());
            pipeline.run(ctx)?;
            match (current, project) {
                (c, p) => {
                    tracing::info!("No upgrade steps defined (current is {c}, project is {p})")
                }
            }
            Ok(())
        }
    }
}
