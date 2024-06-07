//! This module is meant to implement the hooks section in the metadata.json.
//!
//! Hooks are custom scripts that can be automatically integrated into the developer package's lifecycle.

use std::{collections::HashMap, path::PathBuf, process::Stdio};

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct Hooks {
    pub pre_run: Vec<ScriptHook>,
    pub post_run: Vec<ScriptHook>,
}

pub fn execute_all(hooks: Vec<ScriptHook>) -> anyhow::Result<()> {
    for script in hooks {
        script.execute()?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ScriptHook {
    pub cmd: String,
    pub args: Option<Vec<String>>,
    pub working_directory: Option<PathBuf>,
    pub env_overrides: Option<HashMap<String, String>>,
    #[serde(default)]
    pub hide_output: bool,
    #[serde(default)]
    pub continue_on_failure: bool,
}

impl ScriptHook {
    pub fn execute(self) -> anyhow::Result<()> {
        let mut cmd = std::process::Command::new(self.cmd.clone());
        let mut cmd = cmd
            .args(self.args.unwrap_or_default())
            .envs(self.env_overrides.unwrap_or_default())
            .env("MSDE_CLI_RUNNER", "true")
            .stdin(Stdio::null())
            .stdout(if self.hide_output {
                Stdio::null()
            } else {
                Stdio::inherit()
            })
            .stderr(if self.hide_output {
                Stdio::null()
            } else {
                Stdio::inherit()
            });
        if let Some(wd) = self.working_directory {
            cmd = cmd.current_dir(wd);
        }

        let mut child = cmd.spawn().with_context(|| {
            format!("failed to spawn custom script (command was `{}`)", self.cmd)
        })?;

        let success = child.wait()?.success();
        if success || self.continue_on_failure {
            Ok(())
        } else {
            Err(anyhow::Error::msg(
                "Custom hook script failed. Check the output above for details.",
            ))
        }
    }
}
