//! 跨平台数据目录与数据库文件位置。
//!
//! 不引入 `dirs` 依赖：三个桌面平台各自的环境变量已足够，且便于测试与排查。

use std::path::PathBuf;

/// 应用数据目录（不存在时由打开 DB 的代码创建）。
pub fn data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join("Library/Application Support/OneLink");
        }
    }
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("OneLink");
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("onelink");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/share/onelink");
    }
    PathBuf::from(".onelink")
}

/// SQLite 数据库文件路径。
pub fn db_path() -> PathBuf {
    data_dir().join("onelink.db")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_path_lives_in_the_app_data_dir() {
        let db = db_path();
        assert_eq!(db.file_name().unwrap(), "onelink.db");
        assert_eq!(db.parent(), Some(data_dir().as_path()));
    }
}
