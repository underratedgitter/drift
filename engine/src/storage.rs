use crate::model::{CHUNK, Entry, MAX_FILES, Transfer, TrustedDevice};
use anyhow::{Context, Result, ensure};
use redb::{Database, ReadableTable, TableDefinition};
use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
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

async fn hash_reader<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<String> {
    let mut hash = blake3::Hasher::new();
    let mut chunk = vec![0; CHUNK];
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        hash.update(&chunk[..n]);
    }
    Ok(hash.finalize().to_hex().to_string())
}

pub async fn hash_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    Ok(hash_reader(&mut file).await?)
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
///
/// This path-based fallback is only used on non-Unix systems. Unix delivery
/// opens the receive folder once and does every lookup and create relative to
/// directory descriptors (see [`at`]), which also closes the check-then-use
/// window that this helper leaves open.
#[cfg(not(unix))]
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
#[cfg(not(unix))]
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

#[cfg(unix)]
mod at {
    use std::{
        ffi::{CString, OsStr},
        fs::File,
        io,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
        os::unix::ffi::OsStrExt,
        path::Path,
    };

    fn c_string(value: &OsStr) -> io::Result<CString> {
        CString::new(value.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Path contains a NUL byte"))
    }

    /// A directory descriptor opened without following a symlink. Every
    /// operation takes one plain name and is relative to this descriptor, so a
    /// path swapped for a symlink after it was checked cannot redirect it.
    pub(super) struct Dir(OwnedFd);

    impl Dir {
        pub(super) fn open(path: &Path) -> io::Result<Self> {
            Self::from_fd(unsafe {
                libc::open(
                    c_string(path.as_os_str())?.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            })
        }

        pub(super) fn try_clone(&self) -> io::Result<Self> {
            Ok(Self(self.0.try_clone()?))
        }

        fn from_fd(fd: libc::c_int) -> io::Result<Self> {
            if fd < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
            }
        }

        pub(super) fn open_child(&self, name: &OsStr) -> io::Result<Self> {
            Self::from_fd(unsafe {
                libc::openat(
                    self.0.as_raw_fd(),
                    c_string(name)?.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            })
        }

        pub(super) fn open_or_create_child(&self, name: &OsStr) -> io::Result<Self> {
            match self.open_child(name) {
                Ok(child) => Ok(child),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let encoded = c_string(name)?;
                    if unsafe {
                        libc::mkdirat(self.0.as_raw_fd(), encoded.as_ptr(), 0o777 as libc::mode_t)
                    } < 0
                    {
                        let error = io::Error::last_os_error();
                        if error.kind() != io::ErrorKind::AlreadyExists {
                            return Err(error);
                        }
                    }
                    self.open_child(name)
                }
                Err(error) => Err(error),
            }
        }

        pub(super) fn open_existing_file(&self, name: &OsStr) -> io::Result<File> {
            let fd = unsafe {
                libc::openat(
                    self.0.as_raw_fd(),
                    c_string(name)?.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(unsafe { File::from_raw_fd(fd) })
            }
        }

        pub(super) fn create_file(&self, name: &OsStr) -> io::Result<File> {
            let fd = unsafe {
                libc::openat(
                    self.0.as_raw_fd(),
                    c_string(name)?.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o666,
                )
            };
            if fd < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(unsafe { File::from_raw_fd(fd) })
            }
        }

        /// Hard-links `source` into this directory. `linkat` never replaces an
        /// existing name, including a symlink planted at that name.
        pub(super) fn link(&self, source: &Path, name: &OsStr) -> io::Result<()> {
            let source = c_string(source.as_os_str())?;
            let name = c_string(name)?;
            if unsafe {
                libc::linkat(
                    libc::AT_FDCWD,
                    source.as_ptr(),
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    0,
                )
            } < 0
            {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        pub(super) fn unlink(&self, name: &OsStr) -> io::Result<()> {
            let name = c_string(name)?;
            if unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), 0) } < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(unix)]
fn not_a_directory(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code) if code == libc::ELOOP || code == libc::ENOTDIR
    )
}

/// Opens (and optionally creates) `relative` below the already-open `base`.
/// Each component is opened with `O_DIRECTORY|O_NOFOLLOW`; nothing is ever
/// looked up through a path that can be replaced after validation.
#[cfg(unix)]
fn open_delivery_dir(
    base: &at::Dir,
    base_path: &Path,
    relative: &Path,
    create: bool,
) -> Result<at::Dir> {
    let mut dir = base.try_clone()?;
    let mut shown = base_path.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            anyhow::bail!("Unsafe delivery path {}", relative.display());
        };
        shown.push(name);
        let opened = if create {
            dir.open_or_create_child(name)
        } else {
            dir.open_child(name)
        };
        dir = match opened {
            Ok(child) => child,
            Err(error) if not_a_directory(&error) => {
                anyhow::bail!(
                    "Refusing to deliver through {}: not a plain directory",
                    shown.display()
                )
            }
            Err(error) => {
                return Err(error).with_context(|| format!("Could not open {}", shown.display()));
            }
        };
    }
    Ok(dir)
}

#[cfg(unix)]
fn split_target(relative: &Path) -> Result<(&Path, &std::ffi::OsStr)> {
    ensure!(!relative.as_os_str().is_empty(), "Delivery path is empty");
    for component in relative.components() {
        ensure!(
            matches!(component, Component::Normal(_)),
            "Unsafe delivery path {}",
            relative.display()
        );
    }
    Ok((
        relative.parent().unwrap_or_else(|| Path::new("")),
        relative
            .file_name()
            .context("Delivery path has no file name")?,
    ))
}

/// The receive folder, opened once per delivery. On Unix the descriptor is the
/// authority for every later write: destination paths are never reopened, so a
/// directory swapped for a symlink after validation cannot redirect a file.
/// On non-Unix the path-based fallback keeps the previous checks but retains a
/// smaller check-then-use window (see the fallback methods).
struct Delivery {
    #[cfg(unix)]
    inbox: at::Dir,
    inbox_path: PathBuf,
}

impl Delivery {
    /// Creates the receive folder if needed and rejects one that is a symlink,
    /// a file, or another non-directory. Matching the data directory, symlinks
    /// are refused rather than resolved, so received files cannot land outside
    /// the configured folder.
    async fn open(inbox: &Path) -> Result<Self> {
        // Components removes trailing slashes: opening "link/" would otherwise
        // let the OS follow the link before O_NOFOLLOW is applied.
        let inbox_path: PathBuf = inbox.components().collect();
        tokio::fs::create_dir_all(&inbox_path)
            .await
            .with_context(|| {
                format!(
                    "Could not create the receive folder {}",
                    inbox_path.display()
                )
            })?;
        let meta = tokio::fs::symlink_metadata(&inbox_path)
            .await
            .with_context(|| {
                format!(
                    "Could not inspect the receive folder {}",
                    inbox_path.display()
                )
            })?;
        ensure!(
            !meta.file_type().is_symlink(),
            "The receive folder cannot be a symlink: {}",
            inbox_path.display()
        );
        ensure!(
            meta.is_dir(),
            "The receive folder is not a directory: {}",
            inbox_path.display()
        );
        #[cfg(unix)]
        let inbox = at::Dir::open(&inbox_path).with_context(|| {
            format!(
                "The receive folder must be a real directory: {}",
                inbox_path.display()
            )
        })?;
        Ok(Self {
            #[cfg(unix)]
            inbox,
            inbox_path,
        })
    }

    fn path(&self) -> &Path {
        &self.inbox_path
    }
}

#[cfg(unix)]
impl Delivery {
    async fn save(
        &self,
        relative: &Path,
        source: &Path,
        entry: &Entry,
        target: &Path,
    ) -> Result<()> {
        let (parent_path, name) = split_target(relative)?;
        let parent = open_delivery_dir(&self.inbox, &self.inbox_path, parent_path, true)?;
        let linked = parent.link(source, name);
        match linked {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if Self::matches(&parent, name, entry, target).await? {
                    Ok(())
                } else {
                    Err(anyhow::Error::new(error)).with_context(|| {
                        format!("Could not save {} to {}", entry.path, target.display())
                    })
                }
            }
            // Other filesystems or volumes: copy through an exclusive create,
            // still relative to the already-opened parent directory.
            Err(_) => self.copy_into(&parent, name, source, entry, target).await,
        }
    }

    async fn copy_into(
        &self,
        parent: &at::Dir,
        name: &std::ffi::OsStr,
        source: &Path,
        entry: &Entry,
        target: &Path,
    ) -> Result<()> {
        let output = match parent.create_file(name) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if Self::matches(parent, name, entry, target).await? {
                    return Ok(());
                }
                return Err(anyhow::Error::new(error)).with_context(|| {
                    format!("Could not save {} to {}", entry.path, target.display())
                });
            }
            Err(error) => {
                return Err(anyhow::Error::new(error)).with_context(|| {
                    format!("Could not save {} to {}", entry.path, target.display())
                });
            }
        };
        let copied = async {
            let mut output = tokio::fs::File::from_std(output);
            let mut input = tokio::fs::File::open(source).await?;
            tokio::io::copy(&mut input, &mut output).await?;
            output.sync_all().await
        }
        .await;
        if copied.is_err() {
            let _ = parent.unlink(name);
        }
        copied.with_context(|| format!("Could not save {} to {}", entry.path, target.display()))
    }

    /// True only when the existing name is a plain file with the exact
    /// expected size and hash. The check reads the opened file, not its path.
    async fn matches(
        parent: &at::Dir,
        name: &std::ffi::OsStr,
        entry: &Entry,
        target: &Path,
    ) -> Result<bool> {
        let file = match parent.open_existing_file(name) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
                anyhow::bail!("Refusing to deliver onto the symlink {}", target.display())
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Could not inspect {}", target.display()));
            }
        };
        let mut file = tokio::fs::File::from_std(file);
        let meta = file.metadata().await?;
        if !meta.is_file() || meta.len() != entry.size {
            return Ok(false);
        }
        Ok(hash_reader(&mut file).await? == entry.blake3)
    }

    async fn verify(&self, relative: &Path, entry: &Entry, target: &Path) -> Result<()> {
        let (parent_path, name) = split_target(relative)?;
        let parent = open_delivery_dir(&self.inbox, &self.inbox_path, parent_path, false)
            .with_context(|| {
                format!(
                    "A received file was removed before it could be saved: {}",
                    entry.path
                )
            })?;
        match parent.open_existing_file(name) {
            Ok(file) => {
                let meta = file.metadata()?;
                ensure!(
                    meta.is_file() && meta.len() == entry.size,
                    "A received file was removed before it could be saved: {}",
                    entry.path
                );
                let mut file = tokio::fs::File::from_std(file);
                ensure!(
                    hash_reader(&mut file).await? == entry.blake3,
                    "A delivered file failed integrity verification: {}",
                    entry.path
                );
                Ok(())
            }
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
                anyhow::bail!("Refusing to deliver onto the symlink {}", target.display())
            }
            Err(error) => Err(error).with_context(|| {
                format!(
                    "A received file was removed before it could be saved: {}",
                    entry.path
                )
            }),
        }
    }
}

#[cfg(not(unix))]
impl Delivery {
    async fn save(
        &self,
        _relative: &Path,
        source: &Path,
        entry: &Entry,
        target: &Path,
    ) -> Result<()> {
        if let Ok(meta) = tokio::fs::symlink_metadata(target).await {
            ensure!(
                !meta.file_type().is_symlink(),
                "Refusing to deliver onto the symlink {}",
                target.display()
            );
        }
        if let Some(parent) = target.parent() {
            create_dirs_beneath(&self.inbox_path, parent).await?;
        }
        match link_or_copy(source, target).await {
            Ok(()) => Ok(()),
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists
                    && tokio::fs::metadata(target).await.map(|m| m.len()).ok()
                        == Some(entry.size)
                    && hash_file(target).await? == entry.blake3 =>
            {
                Ok(())
            }
            Err(error) => Err(error)
                .with_context(|| format!("Could not save {} to {}", entry.path, target.display())),
        }
    }

    async fn verify(&self, _relative: &Path, entry: &Entry, target: &Path) -> Result<()> {
        ensure!(
            tokio::fs::metadata(target).await.map(|m| m.len()).ok() == Some(entry.size)
                && hash_file(target).await? == entry.blake3,
            "A received file was removed before it could be saved: {}",
            entry.path
        );
        Ok(())
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
        let delivery = Delivery::open(&inbox).await?;
        let inbox = delivery.path().to_owned();
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
            let relative = target.strip_prefix(&inbox).with_context(|| {
                format!(
                    "Delivery plan path {} is outside the receive folder {}",
                    target.display(),
                    inbox.display()
                )
            })?;
            if !source.exists() {
                // Moved before an interruption; the delivered copy must be intact.
                delivery.verify(relative, entry, &target).await?;
                continue;
            }
            delivery
                .save(relative, &source, entry, &target)
                .await
                .with_context(|| {
                    format!("Could not save {} to {}", entry.path, target.display())
                })?;
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

        let delivery = Delivery::open(inbox.path()).await.unwrap();
        let error =
            match open_delivery_dir(&delivery.inbox, inbox.path(), Path::new("photos"), true) {
                Ok(_) => panic!("followed a symlink"),
                Err(error) => error,
            };

        assert!(error.to_string().contains("not a plain directory"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_inbox_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let inbox = root.path().join("inbox");
        std::os::unix::fs::symlink(outside.path(), &inbox).unwrap();
        assert!(Delivery::open(&inbox).await.is_err());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn opened_parent_cannot_be_redirected_by_symlink_swap() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let delivery = Delivery::open(root.path()).await.unwrap();
        let parent =
            open_delivery_dir(&delivery.inbox, root.path(), Path::new("photos"), true).unwrap();
        std::fs::rename(root.path().join("photos"), root.path().join("original")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("photos")).unwrap();
        let mut file = parent
            .create_file(std::ffi::OsStr::new("safe.txt"))
            .unwrap();
        file.write_all(b"safe").unwrap();
        assert!(!outside.path().join("safe.txt").exists());
        assert_eq!(
            std::fs::read(root.path().join("original/safe.txt")).unwrap(),
            b"safe"
        );
        let source = root.path().join("source");
        std::fs::write(&source, b"linked").unwrap();
        parent
            .link(&source, std::ffi::OsStr::new("linked.txt"))
            .unwrap();
        assert!(!outside.path().join("linked.txt").exists());
        assert_eq!(
            std::fs::read(root.path().join("original/linked.txt")).unwrap(),
            b"linked"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resumed_delivery_rejects_same_size_corruption() {
        let inbox = tempfile::tempdir().unwrap();
        let delivery = Delivery::open(inbox.path()).await.unwrap();
        let target = inbox.path().join("file");
        std::fs::write(&target, b"bad").unwrap();
        let entry = Entry {
            path: "file".into(),
            size: 3,
            blake3: blake3::hash(b"yes").to_hex().to_string(),
        };
        assert!(
            delivery
                .verify(Path::new("file"), &entry, &target)
                .await
                .is_err()
        );
    }
}
