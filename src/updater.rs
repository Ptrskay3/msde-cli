use md5::{Digest, Md5};
use std::borrow::Cow;
use std::cmp::Ordering;
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

#[derive(Debug)]
pub struct PackageUpgradePipeline {
    pub steps: Vec<PackageUpgradeStep>,
}

impl PackageUpgradePipeline {
    pub fn empty() -> Self {
        Self { steps: Vec::new() }
    }

    pub fn default_version_writer(self_version: semver::Version) -> Self {
        Self {
            steps: vec![PackageUpgradeStep::Auto(Auto {
                f: Box::new(move |ctx: &Context| -> anyhow::Result<()> {
                    ctx.upgrade_package_local_version(self_version)
                }),
            })],
        }
    }

    pub fn default_project_extractor() -> Self {
        Self {
            steps: vec![PackageUpgradeStep::Auto(Auto {
                f: Box::new(move |ctx: &Context| -> anyhow::Result<()> {
                    ctx.unpack_project_files()
                }),
            })],
        }
    }

    pub fn run(self, context: &Context, manual_only: bool) -> anyhow::Result<()> {
        for step in self.steps {
            step.perform(context, manual_only)?;
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

    pub fn push_manual<'a>(&mut self, display_msg: impl Into<Cow<'a, str>>) {
        self.steps.push(PackageUpgradeStep::Manual(Manual {
            display_msg: display_msg.into().into_owned(),
        }))
    }
}

#[derive(Debug)]
pub enum PackageUpgradeStep {
    // Steps that will be performed, because it's safe and easy to do.
    Auto(Auto),
    // Steps that will be displayed, because it can't be done automatically by this tool
    Manual(Manual),
}

impl PerformStep for PackageUpgradeStep {
    fn perform(self, context: &Context, manual_only: bool) -> anyhow::Result<()> {
        match self {
            PackageUpgradeStep::Auto(a) => a.perform(context, manual_only),
            PackageUpgradeStep::Manual(m) => m.perform(context, manual_only),
        }
    }
}

pub trait PerformStep {
    fn perform(self, context: &Context, manual_only: bool) -> anyhow::Result<()>;
}

pub struct Auto {
    f: Box<dyn FnOnce(&Context) -> anyhow::Result<()>>,
}

impl std::fmt::Debug for Auto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Auto")
            .field("f", &"boxed-upgrade-function")
            .finish()
    }
}

impl PerformStep for Auto {
    fn perform(self, context: &Context, manual_only: bool) -> anyhow::Result<()> {
        if !manual_only {
            (self.f)(context)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct Manual {
    display_msg: String,
}

impl PerformStep for Manual {
    fn perform(self, _context: &Context, _manual_only: bool) -> anyhow::Result<()> {
        println!("{}", self.display_msg);
        Ok(())
    }
}

pub fn consecutive_upgrade(
    current: semver::Version,
    project: semver::Version,
    _ctx: &Context,
) -> anyhow::Result<Option<PackageUpgradePipeline>> {
    // IMPORTANT: Only define major and minor version upgrades here, and only consecutively.
    // Always ignore the patch version (and never do breaking changes in a patch version).
    let (c_major, c_minor) = (&current.major, &current.minor);
    let (p_major, p_minor) = (&project.major, &project.minor);
    match ((c_major, c_minor), (p_major, p_minor)) {
        // If you don't need to do any specific migration, just return `Ok(None)`.
        // Otherwise, you may add arbitrary code to an upgrade. This step is kept to showcase the logic.
        // There're two built-in steps that you don't need to care about:
        //   - The unpacking of the `package` folder - this will be upgraded on the user's machine.
        //   - The upgrade of the `metadata.json` file in their project folder.
        ((0, 13), (0, 14)) => {
            let mut pipeline = PackageUpgradePipeline::empty();
            pipeline.push_auto(|_ctx: &Context| -> anyhow::Result<()> {
                println!("This is an automatic upgrade step that may run arbitrary code.");
                Ok(())
            });
            pipeline.push_manual("Hello from a manual step! This will be printed to the terminal as an instruction to the user.");
            Ok(Some(pipeline))
        }
        // Versions under 0.13 don't need any special treatment.
        ((0, _), (0, &a)) if a < 13 => Ok(None),
        (_, _) => {
            tracing::error!(%current, %project,
                "Internal error: unexpected version pair"
            );
            anyhow::bail!("Failed");
        }
    }
}

/// This pipeline executes a series of consecutive upgrades, so we don't need to exponentially grow the upgrade matrix for
/// every possible version we release.
#[derive(Debug)]
pub struct TransitiveUpgradePipeline {
    pub pipelines: Vec<PackageUpgradePipeline>,
}

impl TransitiveUpgradePipeline {
    pub fn new() -> Self {
        Self {
            pipelines: Vec::new(),
        }
    }

    pub fn with_default_writers(self_version: semver::Version) -> Self {
        Self {
            pipelines: vec![
                PackageUpgradePipeline::default_version_writer(self_version),
                PackageUpgradePipeline::default_project_extractor(),
            ],
        }
    }

    pub fn push_pipeline(&mut self, pipeline: PackageUpgradePipeline) {
        self.pipelines.push(pipeline);
    }

    pub fn run(self, context: &Context, manual_only: bool) -> anyhow::Result<()> {
        for pipeline in self.pipelines {
            pipeline.run(context, manual_only)?;
        }
        Ok(())
    }
}

impl FromIterator<anyhow::Result<Option<PackageUpgradePipeline>>> for TransitiveUpgradePipeline {
    fn from_iter<T: IntoIterator<Item = anyhow::Result<Option<PackageUpgradePipeline>>>>(
        iter: T,
    ) -> Self {
        let mut transitive_upgrade_pipeline = TransitiveUpgradePipeline::new();
        for i in iter {
            if let Ok(Some(pipeline)) = i {
                transitive_upgrade_pipeline.push_pipeline(pipeline);
            }
        }

        transitive_upgrade_pipeline
    }
}

impl Extend<anyhow::Result<Option<PackageUpgradePipeline>>> for TransitiveUpgradePipeline {
    fn extend<T: IntoIterator<Item = anyhow::Result<Option<PackageUpgradePipeline>>>>(
        &mut self,
        iter: T,
    ) {
        for i in iter {
            if let Ok(Some(pipeline)) = i {
                self.push_pipeline(pipeline);
            }
        }
    }
}

pub fn get_upgrade_path(
    from: &semver::Version,
    to: &semver::Version,
) -> Vec<(semver::Version, semver::Version)> {
    let mut path = Vec::new();
    let mut current_version = from.clone();

    while &current_version < to {
        let next_version = if current_version.minor == to.minor && current_version.major == to.major
        {
            to.clone()
        } else {
            semver::Version::new(current_version.major, current_version.minor + 1, 0)
        };

        path.push((current_version.clone(), next_version.clone()));
        current_version = next_version;
    }

    path
}

// TODO: Prompt and display what files will be overwritten.
pub fn upgrade_project(
    current: semver::Version,
    project: semver::Version,
    ctx: &Context,
    manual_only: bool,
) -> anyhow::Result<()> {
    match current.cmp(&project) {
        Ordering::Less => {
            tracing::info!("You're trying to downgrade the project. Consider installing an older version of `msde-cli`.");
            return Ok(());
        }
        Ordering::Equal => {
            tracing::info!("Up to date.");
            return Ok(());
        }
        _ => {}
    }
    tracing::info!("Upgrading project {project} -> {current}");

    let mut pipeline = TransitiveUpgradePipeline::with_default_writers(current.clone());
    pipeline.extend(
        get_upgrade_path(&project, &current)
            .into_iter()
            .map(|(lower, upper)| consecutive_upgrade(lower, upper, &ctx)),
    );
    pipeline.run(&ctx, manual_only)?;
    Ok(())
}
