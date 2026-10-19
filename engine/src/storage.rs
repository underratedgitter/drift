use crate::model::{CHUNK, Entry, MAX_FILES, Transfer};
use anyhow::{Context, Result, ensure};
use redb::{Database, ReadableTable, TableDefinition};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const RECORDS: TableDefinition<&str, &str> = TableDefinition::new("transfers");

#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
    db: Arc<Database>,
}

pub fn safe_path(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && path.len() <= 1024,
        "Invalid path length"
    );
    ensure!(
        !path.contains(['\\', ':', '\0']) && !path.starts_with('/'),
        "Unsafe filename"
    );
    for part in path.split('/') {
        ensure!(
            !part.is_empty() && part != "." && part != ".." && part.len() <= 200,
            "Unsafe path component"
        );
        ensure!(
            !part
                .chars()
                .any(|c| c.is_control() || "<>\"|?*".contains(c)),
            "Unsupported filename"
        );
        ensure!(!part.ends_with(['.', ' ']), "Unsafe trailing characters");
        let stem = part.split('.').next().unwrap().to_uppercase();
        ensure!(
            ![
                "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
                "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
                "LPT9"
            ]
            .contains(&stem.as_str()),
            "Reserved filename"
        );
    }
    Ok(())
}

pub fn validate(entries: &[Entry]) -> Result<()> {
    ensure!(
        !entries.is_empty() && entries.len() <= MAX_FILES,
        "Choose between 1 and 10,000 files"
    );
    let mut paths = HashSet::new();
    let mut total = 0u64;
    for entry in entries {
        safe_path(&entry.path)?;
        ensure!(
            entry.size <= 9_007_199_254_740_991,
            "File exceeds safe offset range"
        );
        total = total.checked_add(entry.size).context("Size overflow")?;
        ensure!(
            total <= 9_007_199_254_740_991,
            "Transfer exceeds safe offset range"
        );
        ensure!(
            entry.sha256.len() == 64 && hex::decode(&entry.sha256).is_ok(),
            "Invalid SHA-256"
        );
        ensure!(
            paths.insert(entry.path.to_lowercase()),
            "Duplicate or case-colliding filename"
        );
    }
    for path in &paths {
        let mut prefix = String::new();
        let parts: Vec<_> = path.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            ensure!(!paths.contains(&prefix), "File conflicts with directory");
        }
    }
    Ok(())
}

pub async fn hash_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hash = Sha256::new();
    let mut chunk = vec![0; CHUNK];
    loop {
        let n = file.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        hash.update(&chunk[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}

impl Store {
    pub fn open(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root)?;
        ensure!(
            !std::fs::symlink_metadata(&root)?.file_type().is_symlink(),
            "Data directory cannot be a symlink"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        }
        let db = Database::create(root.join("history.redb"))?;
        let tx = db.begin_write()?;
        {
            tx.open_table(RECORDS)?;
        }
        tx.commit()?;
        Ok(Self {
            root,
            db: Arc::new(db),
        })
    }

    pub fn save(&self, transfer: &Transfer) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            tx.open_table(RECORDS)?.insert(
                transfer.id.as_str(),
                serde_json::to_string(transfer)?.as_str(),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Transfer>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(RECORDS)?;
        let mut result = Vec::new();
        for row in table.iter()? {
            let (_, value) = row?;
            result.push(serde_json::from_str(value.value())?);
        }
        result.sort_by_key(|t: &Transfer| std::cmp::Reverse(t.updated));
        Ok(result)
    }

    pub fn get(&self, id: &str) -> Result<Transfer> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(RECORDS)?;
        let value = table.get(id)?.context("Transfer not found")?;
        Ok(serde_json::from_str(value.value())?)
    }

    pub fn clear_history(&self) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            tx.open_table(RECORDS)?.retain(|_, value| {
                serde_json::from_str::<Transfer>(value)
                    .map(|t| t.status != "complete" && t.status != "cancelled")
                    .unwrap_or(true)
            })?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn partial(&self, id: &str, index: usize) -> PathBuf {
        self.root.join("partial").join(id).join(index.to_string())
    }

    pub async fn write_chunk(
        &self,
        transfer: &mut Transfer,
        index: usize,
        offset: u64,
        bytes: &[u8],
    ) -> Result<()> {
        self.write_range(transfer, index, offset, bytes, CHUNK)
            .await
    }

    pub(crate) async fn write_window(
        &self,
        transfer: &mut Transfer,
        index: usize,
        offset: u64,
        bytes: &[u8],
    ) -> Result<()> {
        self.write_range(transfer, index, offset, bytes, crate::model::WINDOW)
            .await
    }

    async fn write_range(
        &self,
        transfer: &mut Transfer,
        index: usize,
        offset: u64,
        bytes: &[u8],
        limit: usize,
    ) -> Result<()> {
        ensure!(
            !self.root.join("received").join(&transfer.id).exists(),
            "Files are already published; they cannot be overwritten"
        );
        let entry = transfer.entries.get(index).context("Invalid file index")?;
        ensure!(
            bytes.len() <= limit && !bytes.is_empty(),
            "Invalid chunk size"
        );
        ensure!(
            transfer.status == "receiving",
            "Transfer is not approved or is paused"
        );
        ensure!(
            transfer.offsets[index] == offset,
            "Offset mismatch; query status before retrying"
        );
        ensure!(
            offset
                .checked_add(bytes.len() as u64)
                .is_some_and(|n| n <= entry.size),
            "Chunk exceeds declared file size"
        );
        let path = self.partial(&transfer.id, index);
        tokio::fs::create_dir_all(path.parent().unwrap()).await?;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .await?;
        file.set_len(offset).await?;
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        file.write_all(bytes).await?;
        file.sync_data().await?;
        transfer.offsets[index] += bytes.len() as u64;
        transfer.updated = crate::model::now();
        self.save(transfer)?;
        Ok(())
    }

    pub async fn finish(&self, transfer: &mut Transfer) -> Result<()> {
        ensure!(transfer.status == "receiving", "Transfer is not receiving");
        let destination = self.root.join("received").join(&transfer.id);
        // History can be cleared independently of received files, or the process
        // can stop after publication but before the completion metadata commits.
        // Verify immutable published files rather than touching their hard links.
        if destination.exists() {
            for entry in &transfer.entries {
                let path = destination.join(&entry.path);
                ensure!(
                    tokio::fs::metadata(&path).await?.len() == entry.size
                        && hash_file(&path).await? == entry.sha256,
                    "Destination conflict: {}",
                    entry.path
                );
            }
            transfer.offsets = transfer.entries.iter().map(|entry| entry.size).collect();
            transfer.status = "complete".into();
            transfer.updated = crate::model::now();
            return self.save(transfer);
        }
        for (index, entry) in transfer.entries.iter().enumerate() {
            ensure!(transfer.offsets[index] == entry.size, "Incomplete file");
            let path = self.partial(&transfer.id, index);
            tokio::fs::create_dir_all(path.parent().unwrap()).await?;
            if entry.size == 0 && !path.exists() {
                tokio::fs::File::create(&path).await?.sync_all().await?;
            }
            ensure!(
                hash_file(&path).await? == entry.sha256,
                "SHA-256 mismatch: {}",
                entry.path
            );
        }
        // Build an unpublished tree, then publish it with one rename. Originals
        // remain for crash recovery. Desktop hard links avoid large copies.
        // Android app SELinux policy denies hard links even in app-private data;
        // copy into the unpublished tree there, verify again, then rename.
        let pending = self.root.join("publishing").join(&transfer.id);
        for (index, entry) in transfer.entries.iter().enumerate() {
            let target = pending.join(&entry.path);
            tokio::fs::create_dir_all(target.parent().unwrap()).await?;
            if target.exists() {
                ensure!(
                    hash_file(&target).await? == entry.sha256,
                    "Publication conflict"
                );
            } else {
                #[cfg(not(target_os = "android"))]
                tokio::fs::hard_link(self.partial(&transfer.id, index), &target)
                    .await
                    .context("Could not hard-link verified file for publication")?;
                #[cfg(target_os = "android")]
                {
                    let copied = tokio::fs::copy(self.partial(&transfer.id, index), &target).await;
                    if let Err(error) = copied {
                        // Remove only this unpublished copy so an interrupted
                        // copy can be retried from the intact partial source.
                        let _ = tokio::fs::remove_file(&target).await;
                        return Err(error).context("Could not copy verified file for publication");
                    }
                    tokio::fs::File::open(&target).await?.sync_all().await?;
                    ensure!(
                        hash_file(&target).await? == entry.sha256,
                        "Publication copy failed SHA-256 verification"
                    );
                }
            }
        }
        tokio::fs::create_dir_all(destination.parent().unwrap()).await?;
        if destination.exists() {
            for entry in &transfer.entries {
                ensure!(
                    hash_file(&destination.join(&entry.path)).await? == entry.sha256,
                    "Destination conflict"
                );
            }
        } else {
            tokio::fs::rename(&pending, &destination).await?;
        }
        transfer.status = "complete".into();
        transfer.updated = crate::model::now();
        self.save(transfer)?;
        Ok(())
    }
}
