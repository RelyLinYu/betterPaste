use std::path::PathBuf;

use crate::{Error, Result};

/// Root data directory: `%APPDATA%\clipstack\`.
pub fn data_dir() -> Result<PathBuf> {
    let dir = dirs::data_dir()
        .ok_or_else(|| Error::Other("%APPDATA% 不可用".into()))?
        .join("clipstack");
    ensure_dir(&dir)?;
    Ok(dir)
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
