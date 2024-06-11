use crate::env::{Context, Feature};

#[cfg(target_os = "linux")]
pub fn wsl() -> bool {
    if let Ok(b) = std::fs::read("/proc/sys/kernel/osrelease") {
        if let Ok(s) = std::str::from_utf8(&b) {
            let a = s.to_ascii_lowercase();
            return a.contains("microsoft") || a.contains("wsl");
        }
    }
    false
}

#[cfg(not(target_os = "linux"))]
pub fn wsl() -> bool {
    false
}

/// Determine what features are enabled based on the --features and --profile arguments, taking into account that
/// the config file may or may not exist. Currently this falls back to the minimal profile on any error.
pub fn resolve_features(
    features: Vec<Feature>,
    profile: Option<String>,
    ctx: &Context,
) -> Vec<Feature> {
    match (features, profile) {
        (f, None) => f,
        (_, Some(profile)) => match &ctx.config {
            Some(cfg) => match cfg.profiles.0.get(&profile) {
                Some(f) => f.clone(),
                None => {
                    tracing::warn!(profile = %profile, "Profile does not exist, falling back to minimal profile");
                    vec![]
                }
            },
            None => {
                tracing::warn!(profile = %profile, "Config file does not exist, falling back to minimal profile");
                vec![]
            }
        },
    }
}
