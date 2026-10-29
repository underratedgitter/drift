use crate::model::{CHUNK, Entry, MAX_FILES, Transfer, TrustedDevice};
use anyhow::{Context, Result, ensure};
use redb::{Database, ReadableTable, TableDefinition};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const RECORDS: TableDefinition<&str, &str> = TableDefinition::new("transfers");
const DEVICES: TableDefinition<&str, &str> = TableDefinition::new("devices");
// Where each received transfer's top-level items were saved, keyed by transfer
// ID. Kept apart from Transfer records because those are also sent to peers.
const DELIVERIES: TableDefinition<&str, &str> = TableDefinition::new("deliveries");

/// How often receive progress is committed to the history database. File bytes
/// are synced before their offset advances, so after a crash the recorded offset
/// lags by at most this interval and those bytes are simply sent again; it never
/// claims data that is not on disk. Committing on every chunk made many-file
/// transfers quadratic: each small file rewrote and synced the whole record.
const PROGRESS_COMMIT: std::time::Duration = std::time::Duration::from_millis(250);

type Live = std::collections::HashMap<String, (Transfer, std::time::Instant)>;

#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
    /// Visible folder that verified received files are moved into, such as
    /// Downloads/Drift. Without one they stay under `root/received/<id>`.
    pub inbox: Option<PathBuf>,
    db: Arc<Database>,
    // Receive progress newer than its last database commit (see PROGRESS_COMMIT).
    live: Arc<std::sync::Mutex<Live>>,
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
            entry.blake3.len() == 64 && hex::decode(&entry.blake3).is_ok(),
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
    let mut hash = blake3::Hasher::new();
    let mut chunk = vec![0; CHUNK];
    loop {
        let n = file.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        hash.update(&chunk[..n]);
    }
    Ok(hash.finalize().to_hex().to_string())
}

// Per-chunk sync that does not empty the drive's write cache. On Apple systems
// Rust's sync_data issues F_FULLFSYNC, which costs tens of milliseconds, so every
// small file paid it. Plain fsync still pushes the bytes to the drive; the full
// barrier (sync_data) runs before every progress commit and at completion, so a
// committed offset never covers data that is only in the drive's cache.
async fn sync_chunk(file: &mut tokio::fs::File, barrier: bool) -> Result<()> {
    if barrier || !cfg!(target_vendor = "apple") {
        file.sync_data().await?;
        return Ok(());
    }
    file.flush().await?;
    #[cfg(target_vendor = "apple")]
    {
        use std::os::fd::AsRawFd;
        let fd = file.as_raw_fd();
        tokio::task::spawn_blocking(move || {
            if unsafe { libc::fsync(fd) } == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        })
        .await??;
    }
    Ok(())
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
            tx.open_table(DEVICES)?;
            tx.open_table(DELIVERIES)?;
        }
        tx.commit()?;
        let store = Self {
            root,
            inbox: None,
            db: Arc::new(db),
            live: Arc::default(),
        };
        store.settle_after_restart()?;
        Ok(store)
    }

    // Sessions never survive a restart, so a transfer that was moving when Drift
    // closed cannot still be "sending". Show it as resumable instead of frozen.
    fn settle_after_restart(&self) -> Result<()> {
        // Prepared batches live only in memory, so after a restart nothing can use
        // the copies staged for them; delete them instead of keeping hidden gigabytes.
        if let Err(error) = std::fs::remove_dir_all(self.root.join("outbox"))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("Could not clear old prepared copies: {error}");
        }
        for mut transfer in self.list()? {
            // Receivers keep their state: the next offer of the same files resumes it.
            let interrupted = transfer.direction == "send"
                && ["sending", "awaiting approval", "verifying"]
                    .contains(&transfer.status.as_str());
            if interrupted {
                transfer.status = "interrupted".into();
                transfer.error = Some(
                    "Drift closed during this transfer. Choose the same files again to resume."
                        .into(),
                );
                self.save(&transfer)?;
            }
        }
        Ok(())
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
        // The committed record is now the newest state.
        self.live.lock().unwrap().remove(&transfer.id);
        Ok(())
    }

    // Whether receive progress for this transfer is due for a durable commit.
    fn progress_due(&self, id: &str) -> bool {
        self.live
            .lock()
            .unwrap()
            .get(id)
            .is_none_or(|(_, at)| at.elapsed() >= PROGRESS_COMMIT)
    }

    // Records receive progress; `commit` (from progress_due, decided before the
    // chunk's sync) makes it durable, otherwise it stays in memory.
    fn save_progress(&self, transfer: &Transfer, commit: bool) -> Result<()> {
        let committed = self
            .live
            .lock()
            .unwrap()
            .get(&transfer.id)
            .map(|(_, at)| *at);
        let at = match committed {
            Some(at) if !commit => at,
            _ => {
                self.save(transfer)?;
                std::time::Instant::now()
            }
        };
        self.live
            .lock()
            .unwrap()
            .insert(transfer.id.clone(), (transfer.clone(), at));
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Transfer>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(RECORDS)?;
        let live = self.live.lock().unwrap();
        let mut result = Vec::new();
        for row in table.iter()? {
            let (_, value) = row?;
            let transfer: Transfer = serde_json::from_str(value.value())?;
            result.push(match live.get(&transfer.id) {
                Some((newer, _)) => newer.clone(),
                None => transfer,
            });
        }
        drop(live);
        result.sort_by_key(|t: &Transfer| std::cmp::Reverse(t.updated));
        Ok(result)
    }

    pub fn get(&self, id: &str) -> Result<Transfer> {
        if let Some((transfer, _)) = self.live.lock().unwrap().get(id) {
            return Ok(transfer.clone());
        }
        let tx = self.db.begin_read()?;
        let table = tx.open_table(RECORDS)?;
        let value = table.get(id)?.context("Transfer not found")?;
        Ok(serde_json::from_str(value.value())?)
    }

    pub fn devices(&self) -> Result<Vec<TrustedDevice>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(DEVICES)?;
        let mut devices = Vec::new();
        for row in table.iter()? {
            devices.push(serde_json::from_str(row?.1.value())?);
        }
        Ok(devices)
    }

    pub fn save_device(&self, device: &TrustedDevice) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            tx.open_table(DEVICES)?
                .insert(device.id.as_str(), serde_json::to_string(device)?.as_str())?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove_device(&self, id: &str) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            tx.open_table(DEVICES)?.remove(id)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Absolute paths of the saved top-level files and folders of a delivered
    /// transfer, in manifest order.
    pub fn saved(&self, id: &str) -> Result<Vec<String>> {
        Ok(self
            .delivery_plan(id)?
            .into_iter()
            .map(|(_, path)| path)
            .collect())
    }

    fn delivery_plan(&self, id: &str) -> Result<Vec<(String, String)>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(DELIVERIES)?;
        Ok(match table.get(id)? {
            Some(value) => serde_json::from_str(value.value())?,
            None => Vec::new(),
        })
    }

    fn save_delivery_plan(&self, id: &str, plan: &[(String, String)]) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            tx.open_table(DELIVERIES)?
                .insert(id, serde_json::to_string(plan)?.as_str())?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn clear_history(&self) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            let mut records = tx.open_table(RECORDS)?;
            records.retain(|_, value| {
                serde_json::from_str::<Transfer>(value)
                    .map(|t| t.status != "complete" && t.status != "cancelled")
                    .unwrap_or(true)
            })?;
            let mut kept = HashSet::new();
            for row in records.iter()? {
                kept.insert(row?.0.value().to_owned());
            }
            // Saved files stay where they are; only the record of them goes.
            tx.open_table(DELIVERIES)?
                .retain(|id, _| kept.contains(id))?;
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
        let commit = self.progress_due(&transfer.id);
        sync_chunk(&mut file, commit).await?;
        transfer.offsets[index] += bytes.len() as u64;
        transfer.updated = crate::model::now();
        self.save_progress(transfer, commit)?;
        Ok(())
    }

    pub async fn finish(&self, transfer: &mut Transfer) -> Result<()> {
        ensure!(transfer.status == "receiving", "Transfer is not receiving");
        let destination = self.root.join("received").join(&transfer.id);
        // A delivery that started before a crash or restart: finish moving it.
        if destination.exists() && !self.delivery_plan(&transfer.id)?.is_empty() {
            self.deliver(transfer).await?;
            transfer.offsets = transfer.entries.iter().map(|entry| entry.size).collect();
            transfer.status = "complete".into();
            transfer.updated = crate::model::now();
            return self.save(transfer);
        }
        // History can be cleared independently of received files, or the process
        // can stop after publication but before the completion metadata commits.
        // Verify immutable published files rather than touching their hard links.
        if destination.exists() {
            for entry in &transfer.entries {
                let path = destination.join(&entry.path);
                ensure!(
                    tokio::fs::metadata(&path).await?.len() == entry.size
                        && hash_file(&path).await? == entry.blake3,
                    "Destination conflict: {}",
                    entry.path
                );
            }
            if self.inbox.is_some() {
                self.deliver(transfer).await?;
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
                hash_file(&path).await? == entry.blake3,
                "Checksum mismatch: {}",
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
                    hash_file(&target).await? == entry.blake3,
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
                        hash_file(&target).await? == entry.blake3,
                        "Publication copy failed checksum verification"
                    );
                }
            }
        }
        tokio::fs::create_dir_all(destination.parent().unwrap()).await?;
        if destination.exists() {
            for entry in &transfer.entries {
                ensure!(
                    hash_file(&destination.join(&entry.path)).await? == entry.blake3,
                    "Destination conflict"
                );
            }
        } else {
            tokio::fs::rename(&pending, &destination).await?;
        }
        // One full barrier makes every chunk's plain fsync stable before
        // completion is recorded (see sync_chunk).
        tokio::fs::OpenOptions::new()
            .write(true)
            .open(destination.join(&transfer.entries[0].path))
            .await?
            .sync_data()
            .await?;
        if self.inbox.is_some() {
            self.deliver(transfer).await?;
        }
        transfer.status = "complete".into();
        transfer.updated = crate::model::now();
        self.save(transfer)?;
        // Completion is now durable, so the private partial copies (hard links to
        // the published files on desktop) are no longer needed for recovery.
        // Leaving them would keep deleted received files on disk invisibly.
        let partial = self.root.join("partial").join(&transfer.id);
        if let Err(error) = tokio::fs::remove_dir_all(&partial).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "Could not remove recovery files for {}: {error}",
                transfer.id
            );
        }
        Ok(())
    }
}

/// Top-level items of a manifest in order: each is a file, or a folder whose
/// entries all start with "name/".
fn top_level(entries: &[Entry]) -> Vec<(String, bool)> {
    let mut items: Vec<(String, bool)> = Vec::new();
    for entry in entries {
        let (name, folder) = match entry.path.split_once('/') {
            Some((name, _)) => (name, true),
            None => (entry.path.as_str(), false),
        };
        if !items.iter().any(|(existing, _)| existing == name) {
            items.push((name.to_owned(), folder));
        }
    }
    items
}

/// "name", then "name (2)", "name (3)"… keeping a file's extension last.
fn candidate(name: &str, folder: bool, n: u32) -> String {
    if n == 1 {
        return name.to_owned();
    }
    match name.rsplit_once('.') {
        Some((stem, extension)) if !folder && !stem.is_empty() => {
            format!("{stem} ({n}).{extension}")
        }
        _ => format!("{name} ({n})"),
    }
}

fn free_name(
    inbox: &Path,
    name: &str,
    folder: bool,
    taken: &[(String, String)],
) -> Result<PathBuf> {
    for n in 1..=1000 {
        let path = inbox.join(candidate(name, folder, n));
        let planned = taken
            .iter()
            .any(|(_, existing)| Path::new(existing) == path);
        if !planned && std::fs::symlink_metadata(&path).is_err() {
            return Ok(path);
        }
    }
    anyhow::bail!("Too many files named {name} in {}", inbox.display())
}

/// Create `dir` and any missing ancestors below `base`, refusing to traverse
/// symlinks. A link planted inside the receive folder must not redirect a
/// delivery somewhere else on the disk.
async fn create_dirs_beneath(base: &Path, dir: &Path) -> Result<()> {
    let relative = dir
        .strip_prefix(base)
        .with_context(|| format!("{} is outside {}", dir.display(), base.display()))?;
    let mut current = base.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match tokio::fs::symlink_metadata(&current).await {
            Ok(meta) => ensure!(
                meta.is_dir(),
                "Refusing to deliver through {}: not a plain directory",
                current.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::create_dir(&current).await?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Never replaces an existing file: hard links fail if the target exists, and
/// the copy fallback (other volume, Android) opens with create_new.
async fn link_or_copy(source: &Path, target: &Path) -> std::io::Result<()> {
    match tokio::fs::hard_link(source, target).await {
        Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
            let mut input = tokio::fs::File::open(source).await?;
            let mut output = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target)
                .await?;
            let copied = async {
                tokio::io::copy(&mut input, &mut output).await?;
                output.sync_all().await
            }
            .await;
            if copied.is_err() {
                drop(output);
                let _ = tokio::fs::remove_file(target).await;
            }
            copied
        }
        result => result,
    }
}

impl Store {
    /// Moves a verified, published transfer from `received/<id>` into the inbox
    /// under readable names, without overwriting anything already there. The
    /// chosen names are committed before any file moves, so an interrupted
    /// delivery resumes into the same names. `received/<id>` stays behind empty
    /// as the marker that this manifest was already received.
    async fn deliver(&self, transfer: &Transfer) -> Result<()> {
        let inbox = self
            .inbox
            .clone()
            .context("No receive folder is configured")?;
        tokio::fs::create_dir_all(&inbox)
            .await
            .with_context(|| format!("Could not create the receive folder {}", inbox.display()))?;
        let published = self.root.join("received").join(&transfer.id);
        let mut plan = self.delivery_plan(&transfer.id)?;
        if plan.is_empty() {
            for (name, folder) in top_level(&transfer.entries) {
                let path = free_name(&inbox, &name, folder, &plan)?;
                plan.push((name, path.display().to_string()));
            }
            self.save_delivery_plan(&transfer.id, &plan)?;
        }
        for entry in &transfer.entries {
            let (name, rest) = match entry.path.split_once('/') {
                Some((name, rest)) => (name, Some(rest)),
                None => (entry.path.as_str(), None),
            };
            let root = &plan
                .iter()
                .find(|(planned, _)| planned == name)
                .context("Delivery plan does not match the manifest")?
                .1;
            let target = match rest {
                Some(rest) => Path::new(root).join(rest),
                None => PathBuf::from(root),
            };
            let source = published.join(&entry.path);
            if let Ok(meta) = tokio::fs::symlink_metadata(&target).await {
                ensure!(
                    !meta.file_type().is_symlink(),
                    "Refusing to deliver onto the symlink {}",
                    target.display()
                );
            }
            if !source.exists() {
                // Moved before an interruption; the delivered copy must be intact.
                ensure!(
                    tokio::fs::metadata(&target).await.map(|m| m.len()).ok() == Some(entry.size),
                    "A received file was removed before it could be saved: {}",
                    entry.path
                );
                continue;
            }
            if let Some(parent) = target.parent() {
                create_dirs_beneath(&inbox, parent).await?;
            }
            match link_or_copy(&source, &target).await {
                Ok(()) => {}
                // Same bytes already there: an earlier attempt got this far.
                Err(error)
                    if error.kind() == std::io::ErrorKind::AlreadyExists
                        && tokio::fs::metadata(&target).await?.len() == entry.size
                        && hash_file(&target).await? == entry.blake3 => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("Could not save {} to {}", entry.path, target.display())
                    });
                }
            }
            tokio::fs::remove_file(&source).await?;
        }
        // Only empty directories remain; keep the marker directory itself.
        let mut children = tokio::fs::read_dir(&published).await?;
        while let Some(child) = children.next_entry().await? {
            tokio::fs::remove_dir_all(child.path()).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_names_keep_extensions_and_skip_existing_files() {
        assert_eq!(candidate("photo.jpg", false, 1), "photo.jpg");
        assert_eq!(candidate("photo.jpg", false, 2), "photo (2).jpg");
        assert_eq!(candidate("archive.tar.gz", false, 3), "archive.tar (3).gz");
        assert_eq!(candidate(".env", false, 2), ".env (2)");
        assert_eq!(candidate("Trip.2026", true, 2), "Trip.2026 (2)");
        let inbox = tempfile::tempdir().unwrap();
        std::fs::write(inbox.path().join("a.txt"), b"mine").unwrap();
        let taken = vec![(
            "b.txt".to_owned(),
            inbox.path().join("a (2).txt").display().to_string(),
        )];
        assert_eq!(
            free_name(inbox.path(), "a.txt", false, &taken).unwrap(),
            inbox.path().join("a (3).txt")
        );
        assert_eq!(
            top_level(&[
                Entry {
                    path: "Trip/a.jpg".into(),
                    size: 0,
                    blake3: String::new()
                },
                Entry {
                    path: "notes.txt".into(),
                    size: 0,
                    blake3: String::new()
                },
                Entry {
                    path: "Trip/b/c.jpg".into(),
                    size: 0,
                    blake3: String::new()
                },
            ]),
            vec![("Trip".to_owned(), true), ("notes.txt".to_owned(), false)]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delivery_refuses_to_follow_symlinked_folders() {
        let inbox = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), inbox.path().join("photos")).unwrap();

        let error = create_dirs_beneath(inbox.path(), &inbox.path().join("photos"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("not a plain directory"));
    }
}
