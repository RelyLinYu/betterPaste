use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::{Error, Result};

/// Resolved once per process. `map_row` used to call this for **every** row,
/// which meant one SHGetKnownFolderPath + one stat per row (N+1 syscalls on
/// every list/search).
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// `%APPDATA%\clipstack\` —— 永远是这个位置，用来放 `location.txt`（自定义数据目录的
/// 指针）与默认数据。即使数据目录被改到别处，这个目录也继续存在。
pub fn bootstrap_dir() -> Result<PathBuf> {
    let dir = dirs::data_dir()
        .ok_or_else(|| Error::Other("%APPDATA% 不可用".into()))?
        .join("clipstack");
    ensure_dir(&dir)?;
    Ok(dir)
}

/// 自定义数据目录的指针文件。
pub fn location_file() -> Result<PathBuf> {
    Ok(bootstrap_dir()?.join("location.txt"))
}

/// 实际数据目录，优先级：
///   1. 环境变量 `CLIPSTACK_DATA_DIR`
///   2. `%APPDATA%\clipstack\location.txt` 里的自定义路径
///   3. `%APPDATA%\clipstack`
pub fn data_dir() -> Result<PathBuf> {
    if let Some(dir) = DATA_DIR.get() {
        return Ok(dir.clone());
    }
    let dir = resolve_data_dir()?;
    ensure_dir(&dir)?;
    Ok(DATA_DIR.get_or_init(|| dir).clone())
}

fn resolve_data_dir() -> Result<PathBuf> {
    if let Ok(raw) = std::env::var("CLIPSTACK_DATA_DIR") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    if let Ok(raw) = std::fs::read_to_string(location_file()?) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    bootstrap_dir()
}

/// 记住自定义数据目录（`path = None` 表示恢复默认）。
pub fn set_custom_dir(path: Option<&Path>) -> Result<()> {
    let file = location_file()?;
    match path {
        Some(p) => std::fs::write(&file, p.to_string_lossy().as_bytes())?,
        None => {
            if file.exists() {
                std::fs::remove_file(&file)?;
            }
        }
    }
    Ok(())
}

/// 判断两个路径是否指向同一个文件/目录。
///
/// 这是"自拷贝保护"的核心：Windows 上 `CopyFileEx` 的源和目标如果是同一个文件，
/// 会先把目标截断再读，结果**整份文件变成 0**（数据目录迁移就差点这么毁掉用户数据）。
/// 不存在的一方退化为"父目录 canonicalize + 文件名"再比，避免文件还没创建时漏判。
pub fn same_path(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> String {
        let abs = std::fs::canonicalize(p).unwrap_or_else(|_| match (p.parent(), p.file_name()) {
            (Some(parent), Some(name)) => std::fs::canonicalize(parent)
                .map(|c| c.join(name))
                .unwrap_or_else(|_| p.to_path_buf()),
            _ => p.to_path_buf(),
        });
        abs.to_string_lossy()
            .replace('/', "\\")
            .trim_start_matches(r"\\?\")
            .trim_end_matches('\\')
            .to_lowercase()
    }
    norm(a) == norm(b)
}

/// 递归复制数据目录里的**普通文件**（跳过 SQLite 的 `-wal/-shm`、临时文件、
/// 指针文件与数据库本体——数据库由 `Store::vacuum_into` 单独导快照）。
///
/// 单个文件复制失败（被占用/权限）不再让整个迁移失败：记下来继续，
/// 返回 `(复制的字节数, 失败的文件名列表)`。
pub fn copy_tree(from: &Path, to: &Path) -> Result<(u64, Vec<String>)> {
    ensure_dir(&to.to_path_buf())?;
    let mut bytes = 0u64;
    let mut failed: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with("-wal")
            || name.ends_with("-shm")
            || name.ends_with(".tmp")
            || name == "location.txt"
            || name == "clipstack.db"
        {
            continue;
        }
        let target = to.join(&name);
        // 源和目标同一个东西时一定要跳过，否则会把文件清成 0 字节
        if same_path(&entry.path(), &target) {
            failed.push(format!("{name}（源与目标相同，已跳过）"));
            continue;
        }
        if entry.file_type()?.is_dir() {
            let (b, mut f) = copy_tree(&entry.path(), &target)?;
            bytes += b;
            failed.append(&mut f);
        } else {
            match std::fs::copy(entry.path(), &target) {
                Ok(n) => bytes += n,
                Err(e) => failed.push(format!("{name}（{e}）")),
            }
        }
    }
    Ok((bytes, failed))
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
    // `exists()` 为真但其实是文件时，后面写库会以莫名其妙的方式失败
    if !p.is_dir() {
        std::fs::create_dir_all(p)?;
    }
    Ok(())
}
