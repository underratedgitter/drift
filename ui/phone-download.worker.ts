/// <reference lib="webworker" />
import { blake3 } from '@noble/hashes/blake3.js';
type Entry = { path: string; size: number; blake3: string };
type Job = { type: 'receive'; id: string; entries: Entry[]; capability: string };
type Access = { getSize(): number; read(buffer: Uint8Array, options: { at: number }): number; write(buffer: Uint8Array, options: { at: number }): number; truncate(size: number): void; flush(): void; close(): void };
type SyncFile = FileSystemFileHandle & { createSyncAccessHandle?: () => Promise<Access> };
const worker = self as unknown as DedicatedWorkerGlobalScope;
const CHUNK = 1024 * 1024;
let paused = false;
worker.onmessage = event => {
  if (event.data.type === 'pause') { paused = true; return; }
  void receive(event.data as Job).catch((error: Error) => worker.postMessage({ type: 'error', message: error.message }));
};
const delay = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
function check() { if (paused) throw new Error('Paused. Tap Receive / resume to continue from the saved browser copy.'); }
async function receive(job: Job) {
  const root = await navigator.storage.getDirectory();
  const directory = await root.getDirectoryHandle('drift-phone-v1', { create: true });
  const total = job.entries.reduce((sum, entry) => sum + entry.size, 0);
  let done = 0, fetched = 0, emitted = 0;
  const start = performance.now();
  const report = (label: string, force = false) => {
    if (force || performance.now() - emitted >= 100) {
      worker.postMessage({ type: 'progress', label, done, total, rate: fetched / Math.max(0.001, (performance.now() - start) / 1000) }); emitted = performance.now();
    }
  };
  const progress = async (index: number, offset: number, sha?: string) => {
    const response = await fetch(`/mobile/progress/${job.id}`, { method: 'POST', headers: { Authorization: `Bearer ${job.capability}`, 'Content-Type': 'application/json' }, body: JSON.stringify({ index, offset, blake3: sha }) });
    if (!response.ok) throw new Error('The Mac paused, cancelled or rejected verification. Ask it to send a fresh offer.');
    await response.json();
  };
  for (let index = 0; index < job.entries.length; index++) {
    check(); const entry = job.entries[index];
    const handle = await directory.getFileHandle(`${entry.blake3}-${entry.size}.part`, { create: true }) as SyncFile;
    if (!handle.createSyncAccessHandle) throw new Error('This browser lacks streaming private-file access. Update Safari/Chrome. No unverified or whole-memory fallback is used.');
    const file = await handle.createSyncAccessHandle();
    const hasher = blake3.create();
    try {
      if (file.getSize() > entry.size) file.truncate(0);
      let offset = file.getSize();
      // Rehash the saved prefix in bounded buffers; do not assume stored data is intact.
      const buffer = new Uint8Array(CHUNK);
      for (let position = 0; position < offset;) {
        check(); const count = file.read(buffer.subarray(0, Math.min(CHUNK, offset - position)), { at: position });
        if (!count) throw new Error('The saved partial file is unreadable. Clear Drift browser copies and retry.');
        hasher.update(buffer.subarray(0, count)); position += count;
        if (position % (8 * CHUNK) === 0) await delay(0);
      }
      done += offset; let checkpoint = offset;
      while (offset < entry.size) {
        check(); let bytes: Uint8Array<ArrayBuffer> | null = null;
        for (let attempt = 0; attempt < 4; attempt++) {
          try {
            const response = await fetch(`/mobile/download/${job.id}/${index}?offset=${offset}`, { headers: { Authorization: `Bearer ${job.capability}` } });
            if (!response.ok) throw new Error('Mac paused, cancelled or disconnected.');
            bytes = new Uint8Array(await response.arrayBuffer());
            if (!bytes.length || bytes.length > CHUNK || offset + bytes.length > entry.size) throw new Error('Invalid download chunk.');
            break;
          } catch (cause) { if (attempt === 3) throw new Error('Download interrupted. Keep Drift open on the Mac and tap Receive / resume.', { cause }); await delay(250 * 2 ** attempt); check(); }
        }
        if (!bytes) throw new Error('No download data.');
        let written = 0;
        while (written < bytes.length) {
          const count = file.write(bytes.subarray(written), { at: offset + written });
          if (!count) throw new Error('Browser storage is full. Free space and retry.'); written += count;
        }
        hasher.update(bytes); offset += bytes.length; done += bytes.length; fetched += bytes.length;
        report(`Receiving ${entry.path}…`);
        if (offset - checkpoint >= 8 * CHUNK && offset < entry.size) { file.flush(); await progress(index, offset); checkpoint = offset; }
      }
      file.flush();
      const digest = Array.from(hasher.digest(), value => value.toString(16).padStart(2, '0')).join('');
      if (digest !== entry.blake3) { file.truncate(0); file.flush(); throw new Error(`Checksum mismatch for ${entry.path}. Nothing was offered for saving. Retry the download.`); }
      await progress(index, offset, digest);
    } finally { try { file.flush(); } finally { file.close(); } }
    const verified = await handle.getFile();
    worker.postMessage({ type: 'file', name: entry.path, file: new File([verified], entry.path.split('/').pop()!, { type: 'application/octet-stream' }) });
  }
  report('All files verified.', true); worker.postMessage({ type: 'done' });
}
