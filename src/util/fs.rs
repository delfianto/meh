//! File system utilities.

use std::path::Path;

/// Read a file if it exists, returning `None` for missing files.
pub async fn read_file_if_exists(path: &Path) -> anyhow::Result<Option<String>> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write a file, creating parent directories as needed.
pub async fn write_file_safe(path: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, content).await?;
    Ok(())
}

/// Delete a file if it exists. Returns `true` if the file was deleted.
pub async fn delete_file_if_exists(path: &Path) -> anyhow::Result<bool> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_read_file_if_exists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "content").unwrap();
        assert_eq!(
            read_file_if_exists(&path).await.unwrap(),
            Some("content".to_string())
        );
    }

    #[tokio::test]
    async fn test_read_file_not_exists() {
        let path = Path::new("/nonexistent/file.txt");
        assert_eq!(read_file_if_exists(path).await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_write_file_safe() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a/b/c.txt");
        write_file_safe(&path, "nested").await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "nested");
    }

    #[tokio::test]
    async fn test_delete_file_if_exists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("del.txt");
        std::fs::write(&path, "x").unwrap();
        assert!(delete_file_if_exists(&path).await.unwrap());
        assert!(!delete_file_if_exists(&path).await.unwrap());
    }
}
