use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("数据库错误：{0}")]
    Db(#[from] rusqlite::Error),

    #[error("文件读写错误：{0}")]
    Io(#[from] std::io::Error),

    #[error("JSON 错误：{0}")]
    Json(#[from] serde_json::Error),

    #[error("图片处理错误：{0}")]
    Image(#[from] image::ImageError),

    #[error("剪切板错误：{0}")]
    Clipboard(String),

    #[error("热键错误：{0}")]
    Hotkey(String),

    #[error("窗口错误：{0}")]
    Window(String),

    #[error("{0}")]
    Other(String),
}

impl From<tauri::Error> for Error {
    fn from(e: tauri::Error) -> Self {
        Error::Other(e.to_string())
    }
}

/// Tauri commands can only return `Serialize` errors; keep the message readable.
impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
