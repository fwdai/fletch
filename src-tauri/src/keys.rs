//! Manage the SSH key pair the host uses to authenticate to guest VMs.
//!
//! The keypair lives in the app data dir, not the user's `~/.ssh`. We
//! generate it on first run via `ssh-keygen` (always available on macOS).

use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::error::{Error, Result};
use crate::supervisor::KeyMaterial;

pub async fn ensure_key_pair(app_data_dir: &Path) -> Result<KeyMaterial> {
    let private = app_data_dir.join("id_ed25519_algiers");
    let public = app_data_dir.join("id_ed25519_algiers.pub");
    if private.exists() && public.exists() {
        return Ok(KeyMaterial {
            private_key: private,
            public_key: public,
        });
    }

    std::fs::create_dir_all(app_data_dir)?;

    let out = Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            "algiers-agent",
            "-f",
            private
                .to_str()
                .ok_or_else(|| Error::InvalidPath(private.display().to_string()))?,
        ])
        .output()
        .await?;

    if !out.status.success() {
        return Err(Error::Other(format!(
            "ssh-keygen failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }

    Ok(KeyMaterial {
        private_key: private,
        public_key: public,
    })
}

pub fn read_public_key(km: &KeyMaterial) -> Result<String> {
    Ok(std::fs::read_to_string(&km.public_key)?
        .trim()
        .to_string())
}

#[allow(dead_code)]
pub fn key_paths_for_testing(app_data: PathBuf) -> KeyMaterial {
    KeyMaterial {
        private_key: app_data.join("id_ed25519_algiers"),
        public_key: app_data.join("id_ed25519_algiers.pub"),
    }
}
