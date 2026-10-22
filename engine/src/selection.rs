//! Files chosen with the operating system's own picker or drag and drop. They
//! are read in place, without a staging copy. Only the native app shell may
//! call this: it takes absolute paths, which the web interface never can.
use crate::{Shared, Upload, check, model::MAX_FILES, storage::validate};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAX_DEPTH: usize = 32;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Source {
    /// Path inside the transfer, with `/` separators.
    pub path: String,
    pub source: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
pub struct Selection {
    pub id: String,
    pub files: Vec<Upload>,
}

/// Expands chosen files and folders into regular files, keeping each folder's
/// name and structure. Symlinks are refused rather than followed.
pub fn collect(chosen: &[PathBuf]) -> Result<Vec<Source>> {
    ensure!(
        !chosen.is_empty() && chosen.len() <= MAX_FILES,
        "Choose between 1 and 10,000 files or folders"
    );
    let mut files = Vec::new();
    for path in chosen {
        ensure!(path.is_absolute(), "Invalid file selection");
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("Choose files with names this device can read")?;
        walk(path, name.to_owned(), 0, &mut files)?;
    }
    ensure!(
        !files.is_empty(),
        "This selection contains no files. Empty folders are not sent."
    );
    Ok(files)
}

fn walk(path: &Path, relative: String, depth: usize, files: &mut Vec<Source>) -> Result<()> {
    ensure!(
        depth <= MAX_DEPTH && files.len() < MAX_FILES,
        "Choose at most 10,000 files, in folders no more than 32 levels deep"
    );
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("Could not read {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!(
            "{} is a shortcut (symlink). Choose the file or folder it points to instead.",
            relative
        );
    }
    if metadata.is_file() {
        files.push(Source {
            path: relative,
            source: path.to_owned(),
        });
    } else if metadata.is_dir() {
        let mut children = std::fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        children.sort();
        for child in children {
            let name = child
                .file_name()
                .and_then(|name| name.to_str())
                .with_context(|| format!("Unsupported file name in {relative}"))?;
            walk(&child, format!("{relative}/{name}"), depth + 1, files)?;
        }
    }
    Ok(())
}

/// Registers sources as a batch that can be sent to a paired device.
pub async fn select(engine: &Shared, sources: Vec<Source>) -> Result<Selection> {
    check(
        !sources.is_empty() && sources.len() <= MAX_FILES,
        "Choose between 1 and 10,000 files",
    )?;
    let mut uploads = Vec::with_capacity(sources.len());
    let mut paths = Vec::with_capacity(sources.len());
    for file in sources {
        let metadata = tokio::fs::symlink_metadata(&file.source).await?;
        check(
            file.source.is_absolute() && metadata.is_file() && !metadata.file_type().is_symlink(),
            "Choose regular files, not symlinks",
        )?;
        uploads.push(Upload {
            path: file.path,
            size: metadata.len(),
        });
        paths.push(file.source);
    }
    let entries: Vec<_> = uploads
        .iter()
        .map(|upload| crate::model::Entry {
            path: upload.path.clone(),
            size: upload.size,
            blake3: "0".repeat(64),
        })
        .collect();
    validate(&entries)?;
    let mut batches = engine.uploads.lock().await;
    check(
        batches.len() < 100,
        "Too many selected batches; restart Drift to clear them",
    )?;
    let id = uuid::Uuid::new_v4().to_string();
    engine.native_sources.lock().await.insert(id.clone(), paths);
    batches.insert(id.clone(), uploads.clone());
    Ok(Selection { id, files: uploads })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_keep_their_name_and_structure() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("Trip");
        std::fs::create_dir_all(folder.join("day 2")).unwrap();
        std::fs::create_dir_all(folder.join("empty")).unwrap();
        std::fs::write(folder.join("a.jpg"), b"a").unwrap();
        std::fs::write(folder.join("day 2/b.jpg"), b"b").unwrap();
        let single = root.path().join("notes.txt");
        std::fs::write(&single, b"n").unwrap();
        let files = collect(&[folder.clone(), single.clone()]).unwrap();
        let paths: Vec<_> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["Trip/a.jpg", "Trip/day 2/b.jpg", "notes.txt"]);
        assert_eq!(files[1].source, folder.join("day 2/b.jpg"));
        assert!(collect(&[folder.join("empty")]).is_err());
        assert!(collect(&[PathBuf::from("relative.txt")]).is_err());
        assert!(collect(&[]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_refused_not_followed() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("secret"), b"s").unwrap();
        let folder = root.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::os::unix::fs::symlink(root.path().join("secret"), folder.join("link")).unwrap();
        let error = collect(&[folder]).unwrap_err().to_string();
        assert!(error.contains("symlink"), "{error}");
    }
}
