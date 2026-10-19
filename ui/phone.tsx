import { useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { sha256 } from '@noble/hashes/sha2.js';
import './phone.css';

type Entry = { path: string; size: number; sha256: string };
type Offer = { id: string; entries: Entry[]; offsets: number[]; verified: boolean[]; status: string };
type Session = { approved: boolean; confirmed: boolean };
type Snapshot = { name: string; offers: Offer[]; uploads: { id: string; status: string }[] };
const CHUNK = 1024 * 1024;
const initialInvite = location.hash.slice(1);
history.replaceState(null, '', '/phone');
const hex = (bytes: Uint8Array) => Array.from(bytes, value => value.toString(16).padStart(2, '0')).join('');
const bytes = (value: number) => value < 1024 ? `${value} B` : value < CHUNK ? `${(value / 1024).toFixed(1)} KB` : `${(value / CHUNK).toFixed(1)} MB`;
const delay = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

function Phone() {
  const [invite, setInvite] = useState(initialInvite);
  const [name, setName] = useState(/iPhone|iPad/.test(navigator.userAgent) ? 'My iPhone / iPad' : 'My phone');
  const [session, setSession] = useState<Session | null>(null);
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [files, setFiles] = useState<File[]>([]);
  const [busy, setBusy] = useState('');
  const [progress, setProgress] = useState({ done: 0, total: 0, rate: 0 });
  const [saved, setSaved] = useState<{ name: string; url: string; file: File }[]>([]);
  const cap = useRef('');
  const stopped = useRef(false);
  const picker = useRef<HTMLInputElement>(null);
  const activeUpload = useRef('');
  const urls = useRef<string[]>([]);
  const downloadWorker = useRef<Worker | null>(null);

  async function request<T>(path: string, body?: unknown, method?: string): Promise<T> {
    const response = await fetch(`/mobile${path}`, { method: method ?? (body === undefined ? 'GET' : 'POST'),
      headers: { ...(cap.current ? { Authorization: `Bearer ${cap.current}` } : {}), ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
      body: body === undefined ? undefined : JSON.stringify(body) });
    if (!response.ok) {
      let message = `Connection failed (${response.status}). Keep Drift open on the Mac and check Wi-Fi.`;
      try { message = ((await response.json()) as { error: string }).error || message; } catch { /* Non-JSON proxy error; use the actionable default. */ }
      throw new Error(message);
    }
    return response.json() as Promise<T>;
  }
  useEffect(() => {
    let running = true;
    let timeout: ReturnType<typeof setTimeout>;
    const poll = async () => {
      if (cap.current) try {
        const current = await request<Session>('/session');
        if (running) setSession(current);
        if (current.approved && current.confirmed) {
          const state = await request<Snapshot>('/state');
          if (running) setSnapshot(state);
        }
      } catch (e) { if (running) setError((e as Error).message); }
      if (running) timeout = setTimeout(() => void poll(), 1000);
    };
    void poll();
    if ('serviceWorker' in navigator) void navigator.serviceWorker.register('/phone-sw.js').catch(() => {});
    return () => { running = false; clearTimeout(timeout); urls.current.forEach(url => URL.revokeObjectURL(url)); };
  }, []);
  async function action(fn: () => Promise<void>) {
    setError('');
    try { await fn(); } catch (e) { setError((e as Error).message); }
  }
  async function pair() {
    if (!window.isSecureContext || location.protocol !== 'https:') throw new Error('Open the trusted HTTPS invitation from your Mac. Do not use an HTTP link or bypass certificate warnings.');
    let token = invite.trim();
    if (token.startsWith('https:')) { const url = new URL(token); if (url.origin !== location.origin) throw new Error('Open that invitation directly: it belongs to a different Mac or address.'); token = url.hash.slice(1); }
    if (!/^[a-f0-9]{48}$/.test(token)) throw new Error('Scan or paste a new phone invitation from Devices on your Mac.');
    const reply = await request<{ capability: string; name: string }>('/pair', { token, name });
    cap.current = reply.capability;
    setInvite(''); setSession({ approved: false, confirmed: false });
    setNotice(`Approve this phone on ${reply.name}, then confirm here.`);
  }
  function checkStopped() { if (stopped.current) throw new Error('Paused. Select Send / resume or Receive / resume to continue.'); }
  async function hash(file: File): Promise<string> {
    const hasher = sha256.create();
    for (let offset = 0; offset < file.size; offset += CHUNK) { checkStopped(); hasher.update(new Uint8Array(await file.slice(offset, offset + CHUNK).arrayBuffer())); }
    return hex(hasher.digest());
  }
  async function send() {
    if (!files.length) throw new Error('Choose files first.');
    stopped.current = false; setBusy('Checking files…');
    try {
      const entries: Entry[] = [];
      for (const file of files) { setBusy(`Checking ${file.name}…`); entries.push({ path: file.webkitRelativePath || file.name, size: file.size, sha256: await hash(file) }); }
      let transfer = await request<Offer>('/offer', entries);
      activeUpload.current = transfer.id; setBusy('Waiting for approval on the Mac…');
      let attempts = 0;
      while (transfer.status === 'pending' || transfer.status === 'paused') { checkStopped(); if (++attempts > 600) throw new Error('Approval timed out. Tap Send / resume to try again.'); await delay(500); transfer = await request<Offer>(`/transfer/${transfer.id}`); }
      if (transfer.status === 'complete') { setNotice('These files are already verified on your Mac.'); return; }
      if (transfer.status !== 'receiving') throw new Error(`The Mac ${transfer.status} this transfer.`);
      const total = entries.reduce((sum, entry) => sum + entry.size, 0);
      let completed = transfer.offsets.reduce((sum, offset) => sum + offset, 0);
      const initial = completed, started = performance.now();
      for (let index = 0; index < files.length; index++) {
        let offset = transfer.offsets[index]; let failures = 0;
        while (offset < files[index].size) {
          checkStopped(); setBusy(`Sending ${files[index].name}…`);
          const part = files[index].slice(offset, offset + 4 * CHUNK);
          try {
            const response = await fetch(`/mobile/transfer/${transfer.id}/${index}?offset=${offset}`, { method: 'PUT', headers: { Authorization: `Bearer ${cap.current}` }, body: part });
            if (!response.ok) throw new Error('The Mac could not accept this chunk.');
            const ack = await response.json() as { offset: number };
            if (ack.offset !== offset + part.size) throw new Error('Invalid transfer acknowledgement.');
            offset = ack.offset; completed += part.size; failures = 0;
          } catch (cause) {
            if (++failures > 4) throw new Error('Connection interrupted. Keep both devices on Wi-Fi, then tap Send / resume.', { cause });
            await delay(250 * 2 ** failures);
            const remote = await request<Offer>(`/transfer/${transfer.id}`);
            if (remote.status !== 'receiving') throw new Error(`The Mac ${remote.status} this transfer.`, { cause });
            if (remote.offsets[index] < 0 || remote.offsets[index] > files[index].size) throw new Error('Invalid resume offset.', { cause });
            completed += remote.offsets[index] - offset; offset = remote.offsets[index];
          }
          setProgress({ done: completed, total, rate: (completed - initial) / Math.max(0.001, (performance.now() - started) / 1000) });
        }
      }
      checkStopped(); setBusy('Verifying files on the Mac…');
      await request(`/transfer/${transfer.id}/finish`, {});
      setNotice('Files received and SHA-256 verified on the Mac.');
    } finally { setBusy(''); activeUpload.current = ''; }
  }
  async function receive(offer: Offer) {
    if (!navigator.storage?.getDirectory) throw new Error('This browser cannot save a verified streaming download. Update Safari/Chrome or use a supported browser.');
    stopped.current = false; setBusy('Preparing secure storage…');
    try {
      const accepted = await request<Offer>(`/receive/${offer.id}`, { approve: true });
      await new Promise<void>((resolve, reject) => {
        const worker = new Worker(new URL('./phone-download.worker.ts', import.meta.url), { type: 'module' });
        downloadWorker.current = worker;
        worker.onerror = () => reject(new Error('Download worker failed. Update your browser and retry.'));
        worker.onmessage = event => {
          const data = event.data as { type: string; label: string; done: number; total: number; rate: number; name: string; file: File; message: string };
          if (data.type === 'progress') { setBusy(data.label); setProgress({ done: data.done, total: data.total, rate: data.rate }); }
          else if (data.type === 'file') { const url = URL.createObjectURL(data.file); urls.current.push(url); setSaved(previous => [...previous, { name: data.name, url, file: data.file }]); }
          else if (data.type === 'error') reject(new Error(data.message));
          else if (data.type === 'done') resolve();
        };
        worker.postMessage({ type: 'receive', id: accepted.id, entries: accepted.entries, capability: cap.current });
      });
      setNotice('SHA-256 verified. Use Save or Share below to keep your files outside browser storage.');
    } finally { downloadWorker.current?.terminate(); downloadWorker.current = null; setBusy(''); }
  }

  const ready = session?.approved && session.confirmed;
  const percent = progress.total ? Math.min(100, progress.done / progress.total * 100) : 0;
  return <main className="phone-app"><header><a href="/phone" onClick={e => e.preventDefault()} className="phone-brand">drift</a><span>For your phone</span></header>
    <h1>Send & receive.</h1><p className="phone-intro">Files directly between your phone and Mac.</p>
    {error && <div className="phone-error" role="alert">{error}<button onClick={() => setError('')} aria-label="Dismiss error">×</button></div>}
    {notice && <p className="phone-notice" role="status">{notice}</p>}
    {!session ? <section><h2>Connect to your Mac</h2><p>Use the phone QR in Drift → Devices. Your phone must trust the certificate exported from that Mac first.</p><label htmlFor="phone-name">This phone’s name</label><input id="phone-name" value={name} onChange={e => setName(e.target.value)}/><label htmlFor="phone-invite">Phone invitation</label><input id="phone-invite" value={invite} onChange={e => setInvite(e.target.value)} autoComplete="off" placeholder="Scan the QR, or paste its HTTPS link"/><button className="primary" onClick={() => void action(pair)}>Request connection</button></section>
      : !ready ? <section><h2>{session.approved ? 'One last confirmation.' : 'Check your Mac.'}</h2><p>{session.approved ? 'If you approved this phone on your Mac, confirm the connection here.' : 'Approve the incoming connection in Drift on your Mac. Device names alone are not authentication.'}</p>{session.approved && <button className="primary" onClick={() => void action(async () => { setSession(await request<Session>('/confirm', {})); setNotice('Connected. Choose files to send, or accept files from your Mac.'); })}>Confirm connection</button>}</section>
      : <><div className="connection-label"><span/>Connected to {snapshot?.name ?? 'your Mac'} over HTTPS</div><section><h2>Send to your Mac</h2><p>Choose photos or files. Review the incoming file list on your Mac before it starts.</p><input ref={picker} type="file" multiple hidden onChange={e => setFiles(Array.from(e.target.files ?? []))}/><button className="secondary" disabled={!!busy} onClick={() => picker.current?.click()}>Choose files</button>{!!files.length && <p>{files.length} files · {bytes(files.reduce((sum, file) => sum + file.size, 0))}</p>}<button className="primary" disabled={!!busy || !files.length} onClick={() => void action(send)}>Send / resume</button></section>
      <section><h2>From your Mac</h2>{snapshot?.offers.filter(offer => !['cancelled', 'complete'].includes(offer.status)).map(offer => <article key={offer.id}><strong>{offer.entries.length} files · {bytes(offer.entries.reduce((sum, entry) => sum + entry.size, 0))}</strong><ul>{offer.entries.map(entry => <li key={entry.path}>{entry.path}</li>)}</ul><button className="primary" disabled={!!busy} onClick={() => void action(() => receive(offer))}>Receive / resume</button><button className="secondary" disabled={!!busy} onClick={() => void action(async () => { await request(`/receive/${offer.id}`, { approve: false }); })}>Decline</button></article>)}{!snapshot?.offers.some(offer => !['cancelled', 'complete'].includes(offer.status)) && <p>On your Mac, choose files and select this phone as the recipient.</p>}</section></>}
    {busy && <section className="phone-activity" role="status"><strong>{busy}</strong><progress value={progress.done} max={progress.total || 1} aria-label="File transfer progress"/><p>{Math.round(percent)}% · {bytes(progress.done)} / {bytes(progress.total)}{progress.rate > 0 ? ` · ${bytes(progress.rate)}/s · ${Math.ceil(Math.max(0, progress.total - progress.done) / progress.rate)}s left` : ''}</p><button className="secondary" onClick={() => { stopped.current = true; downloadWorker.current?.postMessage({ type: 'pause' }); if (activeUpload.current) void request(`/transfer/${activeUpload.current}/control`, { action: 'paused' }).catch(() => {}); }}>Pause</button></section>}
    {!!saved.length && <section><h2>Verified and ready to save</h2>{saved.map((file, index) => <article key={index}><strong>{file.name}</strong><div className="save-actions"><a className="primary" href={file.url} download={file.file.name}>Save file</a>{navigator.canShare?.({ files: [file.file] }) && <button className="secondary" onClick={() => void action(() => navigator.share({ files: [file.file] }))}>Share / Save to Files</button>}</div></article>)}</section>}
    <details><summary>Keep in mind</summary><p>Keep Drift and this page in the foreground while transferring. Mobile operating systems may pause background activity. Verified downloads use private browser storage and must fit its available quota; save them to Files/Downloads afterwards.</p><p>To install: Android Chrome → menu → Install / Add to Home screen. iPhone Safari → Share → Add to Home Screen. Reopening the app requires a fresh phone invitation.</p><button className="secondary" disabled={!!busy} onClick={() => void action(async () => { urls.current.forEach(url => URL.revokeObjectURL(url)); urls.current = []; setSaved([]); const root = await navigator.storage.getDirectory(); await root.removeEntry('drift-phone-v1', { recursive: true }); setNotice('Drift’s browser copies cleared. Saved files in Files/Downloads were not deleted.'); })}>Clear Drift browser copies</button></details>
    <footer>Encrypted locally. Approved by you.</footer></main>;
}
createRoot(document.getElementById('root')!).render(<Phone/>);
