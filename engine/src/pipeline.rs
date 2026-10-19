//! One-chunk lookahead overlaps source reads with the current durable TLS upload.
//! Reserving queue capacity BEFORE allocation bounds payload buffers to two windows:
//! one in flight and one queued/being read. Dropping the consumer aborts the reader.
use anyhow::{Result, ensure};
use axum::body::Bytes;
use std::path::PathBuf;
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::mpsc,
    task::JoinHandle,
};

pub(crate) struct Chunk {
    pub offset: u64,
    pub bytes: Bytes,
}

pub(crate) struct FilePipeline {
    receiver: mpsc::Receiver<Result<Chunk>>,
    reader: JoinHandle<()>,
}

impl FilePipeline {
    pub fn new(path: PathBuf, offset: u64, size: u64, window: usize) -> Result<Self> {
        ensure!(offset <= size, "Resume offset exceeds source size");
        ensure!(
            window == crate::model::CHUNK || window == crate::model::WINDOW,
            "Unsupported transfer window"
        );
        let (sender, receiver) = mpsc::channel(1);
        let reader = tokio::spawn(async move {
            let result = async {
                let mut file = tokio::fs::File::open(path).await?;
                file.seek(std::io::SeekFrom::Start(offset)).await?;
                let mut position = offset;
                while position < size {
                    let Ok(permit) = sender.reserve().await else {
                        return Ok(());
                    };
                    let count = window.min((size - position) as usize);
                    let mut bytes = vec![0; count];
                    file.read_exact(&mut bytes).await?;
                    permit.send(Ok(Chunk {
                        offset: position,
                        bytes: Bytes::from(bytes),
                    }));
                    position += count as u64;
                }
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                let _ = sender.send(Err(error)).await;
            }
        });
        Ok(Self { receiver, reader })
    }

    pub async fn next(&mut self) -> Result<Option<Chunk>> {
        self.receiver.recv().await.transpose()
    }
}

impl Drop for FilePipeline {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resume_reads_exact_manifest_range_without_trailing_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source");
        tokio::fs::write(&path, b"prefixpayloadextra")
            .await
            .unwrap();
        let mut pipe = FilePipeline::new(path, 6, 13, crate::model::CHUNK).unwrap();
        let chunk = pipe.next().await.unwrap().unwrap();
        assert_eq!(chunk.offset, 6);
        assert_eq!(chunk.bytes.as_ref(), b"payload");
        assert!(pipe.next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn chunks_are_ordered_bounded_and_backpressured() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source");
        let size = crate::model::WINDOW as u64 * 2 + 17;
        let file = tokio::fs::File::create(&path).await.unwrap();
        file.set_len(size).await.unwrap();
        drop(file);
        let mut pipe = FilePipeline::new(path, 0, size, crate::model::WINDOW).unwrap();
        assert_eq!(pipe.receiver.max_capacity(), 1);
        let mut position = 0;
        while let Some(chunk) = pipe.next().await.unwrap() {
            assert_eq!(chunk.offset, position);
            assert!(chunk.bytes.len() <= crate::model::WINDOW);
            assert!(chunk.bytes.iter().all(|byte| *byte == 0));
            position += chunk.bytes.len() as u64;
        }
        assert_eq!(position, size);
    }

    #[tokio::test]
    async fn truncated_or_missing_sources_are_errors_not_silent_eof() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source");
        tokio::fs::write(&path, b"short").await.unwrap();
        let mut pipe = FilePipeline::new(path, 0, 100, crate::model::CHUNK).unwrap();
        assert!(pipe.next().await.is_err());
        let mut pipe =
            FilePipeline::new(root.path().join("missing"), 0, 1, crate::model::CHUNK).unwrap();
        assert!(pipe.next().await.is_err());
        assert!(
            FilePipeline::new(root.path().join("anything"), 2, 1, crate::model::CHUNK).is_err()
        );
        assert!(FilePipeline::new(root.path().join("anything"), 0, 1, usize::MAX).is_err());
    }

    #[tokio::test]
    async fn dropping_pipeline_stops_backpressured_reader() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source");
        let file = tokio::fs::File::create(&path).await.unwrap();
        file.set_len(crate::model::CHUNK as u64 * 8).await.unwrap();
        drop(file);
        let mut pipe =
            FilePipeline::new(path, 0, crate::model::CHUNK as u64 * 8, crate::model::CHUNK)
                .unwrap();
        let _chunk = pipe.next().await.unwrap().unwrap();
        let task = pipe.reader.abort_handle();
        drop(pipe);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
