use std::path::PathBuf;
use std::sync::OnceLock;

use crate::{Error, Result};

/// Resolved once per process. `map_row` used to call this for **every** row,
/// which meant one SHGetKnownFolderPath + one stat per row (N+1 syscalls on
/// every list/search).
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Root data directory: `%APPDATA%\clipstack\`.
pub fn data_dir() -> Result<PathBuf> {
    if let Some(dir) = DATA_DIR.get() {
        return Ok(dir.clone());
    }
    let dir = dirs::data_dir()
        .ok_or_else(|| Error::Other("%APPDATA% 不可用".into()))?
        .join("clipstack");
    ensure_dir(&dir)?;
    Ok(DATA_DIR.get_or_init(|| dir).clone())
}

pub fn images_dir() -> Result<PathBuf> {
    let dir = data_dir()?.join("images");
    ensure_dir(&dir)?;
    Ok(dir)
}

pub fn thumbs_dir() -> Result<PathBuf> {
    let dir = data_dir()?.join("thumbs");
    ensure_dir(&dir)?;
    Ok(dir)
}

pub fn db_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("clipstack.db"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("config.json"))
}

pub fn log_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("clipstack.log"))
}

pub fn ensure_dir(p: &PathBuf) -> Result<()> {
    if !p.exists() {
        std::fs::create_dir_all(p)?;
    }
    Ok(())
}
