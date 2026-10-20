import React, { useCallback, useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import QRCode from 'qrcode';
import { PhonePanel } from './PhonePanel.js';
import './style.css';

type Invitation = { version: number; host: string; port: number; code: string; expires: number };
type Entry = { path: string; size: number; sha256: string };
type Transfer = { id: string; entries: Entry[]; offsets: number[]; status: string; peer: string; updated: number; error: string | null; direction: string };
type Session = { id: string; name: string; approved: boolean; confirmed: boolean };
type Device = { id: string; name: string; os?: string; host?: string; port?: number; confirmed?: boolean; phone?: boolean };
type SelectedFile = { file?: File; path: string; size: number };
type Snapshot = { name: string; os: string; invitation: Invitation; sessions: Session[]; connections: Device[]; transfers: Transfer[]; peers: Device[]; discoveryError: string | null; lastError: string | null; receiveDirectory: string };

const token = location.hash.slice(1);
// The secret stays in this tab's memory, not localStorage, cookies, URLs sent
// to the engine, or external sites. Refresh requires reopening the private URL.
history.replaceState(null, '', location.pathname);
window.addEventListener('hashchange', () => {
  if (/^#[a-f0-9]{64}$/.test(location.hash)) location.reload();
});

async function api<T>(path: string, body?: unknown, method = 'POST'): Promise<T> {
  const response = await fetch(`/api${path}`, {
    method: body === undefined && method === 'POST' ? 'GET' : method,
    headers: { Authorization: `Bearer ${token}`, ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!response.ok) {
    const text = await response.text();
    try { throw new Error((JSON.parse(text) as { error: string }).error); } catch (e) { if (e instanceof SyntaxError) throw new Error(text, { cause: e }); throw e; }
  }
  return response.json() as Promise<T>;
}
const size = (bytes: number) => {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes / 1024, index = 0;
  while (value >= 1024 && index < units.length - 1) { value /= 1024; index++; }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[index]}`;
};
const total = (t: Transfer) => t.entries.reduce((sum, e) => sum + e.size, 0);
const done = (t: Transfer) => t.offsets.reduce((sum, n) => sum + n, 0);
const inviteUrl = (invitation: Invitation) => `drift://pair?invite=${encodeURIComponent(JSON.stringify(invitation))}`;
const codeFormat = (code: string) => code.match(/.{1,5}/g)?.join(' ') ?? code;

async function walkDropped(entries: FileSystemEntry[]): Promise<SelectedFile[]> {
  const result: SelectedFile[] = [];
  const walk = async (entry: FileSystemEntry, prefix: string, depth: number): Promise<void> => {
    if (depth > 32 || result.length >= 10_000) throw new Error('Choose at most 10,000 files, with folders no more than 32 levels deep.');
    const path = `${prefix}${entry.name}`;
    if (entry.isFile) {
      const file = await new Promise<File>((resolve, reject) => (entry as FileSystemFileEntry).file(resolve, reject));
      result.push({ file, path, size: file.size });
    } else if (entry.isDirectory) {
      const reader = (entry as FileSystemDirectoryEntry).createReader();
      for (;;) {
        const children = await new Promise<FileSystemEntry[]>((resolve, reject) => reader.readEntries(resolve, reject));
        if (!children.length) break;
        for (const child of children) await walk(child, `${path}/`, depth + 1);
      }
    }
  };
  for (const entry of entries) await walk(entry, '', 0);
  if (!result.length) throw new Error('This selection has no files. Empty directories are not transferred.');
  return result;
}

function Icon({ name, className = '' }: { name: string; className?: string }) {
  const paths: Record<string, React.ReactNode> = {
    send: <><path d="m4 12 16-8-6 16-3-7-7-1Z"/><path d="m11 13 9-9"/></>,
    devices: <><rect x="3" y="4" width="13" height="11" rx="2"/><path d="M7 20h5m-2-5v5"/><rect x="16" y="10" width="5" height="10" rx="1"/></>,
    history: <><path d="M3 11a9 9 0 1 1 3 8M3 4v7h7"/><path d="M12 7v5l3 2"/></>,
    settings: <><path d="M4 7h16M4 17h16"/><circle cx="8" cy="7" r="3"/><circle cx="16" cy="17" r="3"/></>,
    upload: <><path d="M12 16V3m-5 5 5-5 5 5M4 15v5h16v-5"/></>,
    folder: <path d="M3 7V5h7l2 3h9v12H3V7Z"/>,
    shield: <><path d="m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6l8-3Z"/><path d="m8 12 3 3 5-6"/></>,
    plus: <path d="M12 4v16M4 12h16"/>,
    check: <path d="m5 12 4 4L19 6"/>,
    close: <path d="m6 6 12 12M6 18 18 6"/>,
    file: <><path d="M5 3h9l5 5v13H5V3Z"/><path d="M14 3v6h5"/></>,
    wifi: <><path d="M3 8a15 15 0 0 1 18 0M6 12a10 10 0 0 1 12 0M9 16a5 5 0 0 1 6 0"/><circle cx="12" cy="20" r=".5"/></>,
    sun: <><circle cx="12" cy="12" r="4"/><path d="M12 1v2m0 18v2M1 12h2m18 0h2M4 4l2 2m12 12 2 2M4 20l2-2M18 6l2-2"/></>,
  };
  return <svg className={`icon ${className}`} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{paths[name] ?? paths.devices}</svg>;
}

function App() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [page, setPage] = useState('send');
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [pairOpen, setPairOpen] = useState(false);
  const [host, setHost] = useState('');
  const [port, setPort] = useState('');
  const [code, setCode] = useState('');
  const [inviteText, setInviteText] = useState('');
  const [qr, setQr] = useState('');
  const [selected, setSelected] = useState('');
  const [files, setFiles] = useState<SelectedFile[]>([]);
  const [batch, setBatch] = useState('');
  const [busy, setBusy] = useState('');
  const [dragging, setDragging] = useState(false);
  const [notifications, setNotifications] = useState(false);
  const [rates, setRates] = useState<Record<string, number>>({});
  const previous = useRef<{ time: number; transfers: Transfer[] } | null>(null);
  const filePicker = useRef<HTMLInputElement>(null);
  const folderPicker = useRef<HTMLInputElement>(null);
  const dialog = useRef<HTMLDialogElement>(null);

  const refresh = useCallback(async () => {
    const next = await api<Snapshot>('/state');
    if (next.lastError) setError(next.lastError);
    const timestamp = performance.now();
    const before = previous.current;
    if (before) {
      const elapsed = (timestamp - before.time) / 1000;
      const nextRates: Record<string, number> = {};
      for (const transfer of next.transfers) {
        const old = before.transfers.find(t => t.id === transfer.id);
        if (old && elapsed > 0) nextRates[transfer.id] = Math.max(0, (done(transfer) - done(old)) / elapsed);
        if (old && transfer.status !== old.status && ['complete', 'interrupted', 'sending', 'receiving'].includes(transfer.status)) {
          const message = `${transfer.entries[0]?.path ?? 'Transfer'}: ${transfer.status}`;
          setNotice(message);
          if (notifications && Notification.permission === 'granted') new Notification('Drift', { body: message });
        }
      }
      setRates(nextRates);
    }
    previous.current = { time: timestamp, transfers: next.transfers };
    setSnapshot(next);
  }, [notifications]);

  useEffect(() => {
    let stopped = false;
    let timeout: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try { if (!stopped) await refresh(); } catch (e) { if (!stopped) setError((e as Error).message); }
      if (!stopped) timeout = setTimeout(() => void poll(), 1000);
    };
    void poll();
    return () => { stopped = true; clearTimeout(timeout); };
  }, [refresh]);
  useEffect(() => { document.documentElement.dataset.theme = 'dark'; }, []);
  useEffect(() => {
    const invitation = () => { const value = window.__driftInvitation; if (value?.startsWith('drift://pair?') && value.length <= 8192) { setInviteText(value); setPairOpen(true); } window.__driftInvitation = undefined; };
    const saved = (event: Event) => { const result = (event as CustomEvent<{ok: boolean; error?: string}>).detail; if (result?.ok) setNotice('File exported to your selected location.'); else setError(result?.error || 'Could not export the file.'); };
    invitation(); window.addEventListener('drift-invitation', invitation); window.addEventListener('drift-save-result', saved);
    return () => { window.removeEventListener('drift-invitation', invitation); window.removeEventListener('drift-save-result', saved); };
  }, []);
  useEffect(() => { if (pairOpen) dialog.current?.showModal(); else dialog.current?.close(); }, [pairOpen]);
  useEffect(() => {
    if (snapshot?.invitation) void QRCode.toDataURL(inviteUrl(snapshot.invitation), { margin: 2, width: 220, errorCorrectionLevel: 'M', color: { dark: '#1c2844', light: '#ffffff' } }).then(setQr).catch(() => setError('Could not generate QR code. Use the pairing code instead.'));
  }, [snapshot?.invitation.code, snapshot?.invitation.host, snapshot?.invitation.port]);

  const action = async (fn: () => Promise<unknown>, success?: string) => {
    setError('');
    try { await fn(); if (success) setNotice(success); await refresh(); } catch (e) { setNotice(''); setError((e as Error).message); }
  };
  const choose = (next: File[]) => { setFiles(next.map(file => ({ file, path: file.webkitRelativePath || file.name, size: file.size }))); setBatch(''); setError(''); };
  const pick = async (folder: boolean) => {
    if (!window.drift) { (folder ? folderPicker : filePicker).current?.click(); return; }
    await action(async () => {
      const selected = await (folder ? window.drift!.chooseFolder() : window.drift!.chooseFiles());
      if (selected) { setFiles(selected.files.map(file => ({ path: file.path, size: file.size }))); setBatch(selected.id); }
    });
  };
  const drop = (data: DataTransfer) => {
    const dropped = Array.from(data.files);
    if (window.drift && dropped.length) {
      void action(async () => {
        const selected = await window.drift!.importDroppedFiles(dropped);
        setFiles(selected.files.map(file => ({ path: file.path, size: file.size }))); setBatch(selected.id);
      }, 'Files selected directly. No staging copy.');
      return;
    }
    const entries = Array.from(data.items).map(item => item.webkitGetAsEntry?.()).filter((entry): entry is FileSystemEntry => !!entry);
    if (entries.length) void action(async () => { setFiles(await walkDropped(entries)); setBatch(''); });
    else choose(dropped);
  };
  const connect = async () => {
    setPairOpen(false);
    setNotice('Authenticating the other device…');
    let invitation: Invitation;
    if (inviteText.trim()) {
      const input = inviteText.trim();
      invitation = JSON.parse(input.startsWith('drift:') ? new URL(input).searchParams.get('invite') ?? '' : input) as Invitation;
    } else invitation = { version: 1, host: host.trim(), port: Number(port), code: code.replace(/\s|-/g, '').toLowerCase(), expires: 0 };
    const reply = await api<{ id: string }>('/pair', invitation);
    setSelected(reply.id); setPairOpen(false); setNotice('Request sent. Approve it on the other device, then confirm here.');
  };
  const send = async () => {
    if (!selected || !files.length) { setError('Choose a connected device and at least one file.'); return; }
    setError('');
    try {
      let batchId = batch;
      if (!batchId) {
        setBusy('Preparing files…');
        const reply = await api<{ id: string }>('/batch', files.map(f => ({ path: f.path, size: f.size })));
        batchId = reply.id;
        for (let index = 0; index < files.length; index++) {
          const file = files[index].file;
          if (!file) throw new Error('Select these files again in the desktop picker.');
          for (let offset = 0; offset < file.size; offset += 1024 * 1024) {
            setBusy(`Preparing ${file.name} · ${Math.round(offset / file.size * 100)}%`);
            const response = await fetch(`/api/stage/${batchId}/${index}?offset=${offset}`, { method: 'PUT', headers: { Authorization: `Bearer ${token}` }, body: file.slice(offset, offset + 1024 * 1024) });
            if (!response.ok) throw new Error(`Could not prepare ${file.name}. Check disk space and select it again.`);
          }
        }
        setBatch(batchId);
      }
      if (selected.startsWith('phone:')) {
        setBusy('Checking files for your phone…');
        await api('/phone/offer', { phone: selected.slice(6), batch: batchId });
      } else await api('/send', { connection: selected, batch: batchId });
      setNotice('Files queued. Approve this file list on the receiving device.');
    } catch (e) { setError((e as Error).message); }
    finally { setBusy(''); }
  };
  const mobileNative = snapshot?.os === 'android' || snapshot?.os === 'ios';
  const localLinkMissing = snapshot?.invitation.host.startsWith('127.');
  const active = snapshot?.transfers.filter(t => !['complete', 'cancelled'].includes(t.status)) ?? [];
  const recent = snapshot?.transfers.filter(t => ['complete', 'cancelled'].includes(t.status)) ?? [];
  const pending = snapshot?.sessions.filter(s => !s.approved) ?? [];

  return <div className="app-shell">
    <aside className="sidebar">
      <a className="brand" href="#" onClick={e => { e.preventDefault(); setPage('send'); }} aria-label="Drift home">drift<span className="preview-label">Preview</span></a>
      <nav aria-label="Main navigation">{[['send', 'Send & receive', 'send'], ['devices', 'Devices', 'devices'], ['history', 'History', 'history'], ['settings', 'Settings', 'settings']].map(([id, title, icon]) => <button key={id} aria-label={title} aria-current={page === id ? 'page' : undefined} className={`nav-button ${page === id ? 'active' : ''}`} onClick={() => setPage(id)}><Icon name={icon}/>{title}{id === 'devices' && !!snapshot?.connections.length && <span className="count" aria-hidden="true">{snapshot.connections.length}</span>}</button>)}</nav>
      <div className="own-device"><span className="device-avatar"><Icon name="devices"/></span><div><strong>{snapshot?.name ?? 'Connecting…'}</strong><small><span className="status-dot"/>This device · {snapshot?.os ?? 'local'}</small></div></div>
    </aside>
    <main>
      <header className="topbar"><span><span className="status-dot"/> {snapshot ? (localLinkMissing ? 'Local link not detected' : 'Local network ready') : 'Connecting to engine'}</span></header>
      <div className="content">
        <div className="page-heading"><div><h1>{page === 'send' ? 'Send & receive.' : page === 'devices' ? 'Your devices.' : page === 'history' ? 'Transfer history.' : 'Settings.'}</h1><p>{page === 'send' ? 'Choose your files. Connect a device. Send directly.' : page === 'devices' ? 'Pair without an account. Approve every connection.' : page === 'history' ? 'Files sent and received on this device.' : 'Your preferences, stored locally.'}</p></div>{page !== 'settings' && <button className="button secondary" onClick={() => setPairOpen(true)}><Icon name="plus"/>Pair a device</button>}</div>
        {error && <div className="alert error" role="alert"><span>{error}</span><button className="icon-button" aria-label="Dismiss error" onClick={() => setError('')}><Icon name="close"/></button></div>}
        {notice && <div className="alert" role="status"><span>{notice}</span><button className="icon-button" aria-label="Dismiss notification" onClick={() => setNotice('')}><Icon name="close"/></button></div>}
        {localLinkMissing && <p className="subtle">No local Wi-Fi or hotspot address was detected. Connect your devices to the same Wi-Fi or hotspot, then restart Drift. If discovery is blocked, use manual IP pairing; for a phone hotspot, its address is usually the laptop’s default gateway. A 127.x address works only on this device.</p>}
        {pending.map(session => <div className="approval" key={session.id}><Icon name="shield"/><div><strong>{session.name} wants to connect</strong><p>Only approve if you started pairing on that device. Names are not proof of identity.</p></div><button className="button secondary" onClick={() => void action(() => api(`/session/${session.id}`, { approve: false }))}>Decline</button><button className="button" onClick={() => void action(() => api(`/session/${session.id}`, { approve: true }), 'Connection approved. Confirm on the sending device.')}>Approve</button></div>)}

        {page === 'send' && snapshot?.connections.some(c => !c.confirmed) && <div className="alert"><span>Approve on the other device, then confirm the connection here.</span><button className="button secondary" onClick={() => setPage('devices')}>Review connections</button></div>}
        {page === 'send' && <>
          <div className="send-grid"><section className={`drop-zone ${dragging ? 'dragging' : ''}`} onDragOver={e => { e.preventDefault(); setDragging(true); }} onDragLeave={() => setDragging(false)} onDrop={e => { e.preventDefault(); setDragging(false); drop(e.dataTransfer); }} aria-label="File drop area">
            <h2>{files.length ? `${files.length} ${files.length === 1 ? 'file' : 'files'} ready to go` : 'Drop files here.'}</h2><p>{files.length ? `${size(files.reduce((sum, f) => sum + f.size, 0))} selected · directory structure preserved` : 'Or choose them below.'}</p>
            <div className="drop-actions"><button className="button" onClick={() => void pick(false)} disabled={!!busy}><Icon name="plus"/>Choose files</button>{!mobileNative && <button className="text-button" onClick={() => void pick(true)} disabled={!!busy}><Icon name="folder"/>Choose folder</button>}</div>
            <input ref={filePicker} type="file" multiple hidden onChange={e => choose(Array.from(e.target.files ?? []))}/><input ref={folderPicker} type="file" multiple hidden {...{ webkitdirectory: '' }} onChange={e => choose(Array.from(e.target.files ?? []))}/>
            {files.length > 0 && <div className="send-controls"><label htmlFor="recipient">Send to</label><select id="recipient" value={selected} onChange={e => setSelected(e.target.value)}><option value="">Choose a connected device</option>{snapshot?.connections.map(d => <option key={d.id} value={d.id}>{d.name}</option>)}</select><button className="button" disabled={!!busy || !selected} onClick={() => void send()}>{busy || (batch ? 'Send / resume files' : 'Send files')}</button></div>}
            <div className="drop-footer"><Icon name="shield"/>Encrypted in transit. Verified on arrival.</div>
          </section>
          <section className="nearby"><div className="section-heading"><h2>Nearby devices</h2></div><p className="section-description">On the same network as you</p>
            {snapshot?.discoveryError && <p className="inline-error">Discovery unavailable. Pair using an address and code instead.</p>}
            {snapshot?.peers.length ? <div className="device-list">{snapshot.peers.map(device => <button className="device-row" key={device.id} onClick={() => { setHost(device.host ?? ''); setPort(String(device.port ?? '')); setPairOpen(true); }}><span className="device-avatar"><Icon name="devices"/></span><span><strong>{device.name}</strong><small>{device.os} · Approval required</small></span><span className="connect-plus">+</span></button>)}</div> : <div className="nearby-empty"><p>No devices found yet.</p><small>Open Drift on the same Wi-Fi, or pair with a code.</small></div>}
            <button className="button secondary full" onClick={() => setPairOpen(true)}>Connect with a code</button>
          </section></div>
          <section className="transfers-section"><div className="section-heading"><h2>Transfers <span className="muted">{active.length || ''}</span></h2></div>{active.length ? active.map(t => <TransferRow key={t.id} transfer={t} rate={rates[t.id] ?? 0} onAction={(path, body) => void action(() => api(path, body))}/>) : <p className="empty-copy">No active transfers.</p>}</section>
          {recent.length > 0 && <section className="recent-section"><div className="section-heading"><h2>Recent transfers</h2><button className="text-button" onClick={() => setPage('history')}>View history</button></div>{recent.slice(0, 3).map(t => <HistoryRow key={t.id} transfer={t}/>)}</section>}
        </>}

        {page === 'devices' && <>
          {!mobileNative && <PhonePanel request={api} report={setError}/>}
          <section className="panel"><h2>Connected this session</h2><p>Sessions expire after 24 hours or when Drift closes. Persistent trust is not enabled.</p>
          {snapshot?.connections.length ? snapshot.connections.map(device => <div className="device-row" key={device.id}><span className="device-avatar"><Icon name="devices"/></span><div><strong>{device.name}</strong><small>{device.confirmed ? 'Confirmed on both devices' : device.phone ? 'Confirm on the phone after approving here.' : 'Approve on that device, then confirm here.'}</small></div>{!device.phone && <button className="button" disabled={device.confirmed} onClick={() => void action(() => api(`/confirm/${device.id}`, {}), 'Both devices confirmed. You can now send files.')}>{device.confirmed ? 'Connected' : 'Confirm connection'}</button>}</div>) : <div className="empty-line">No paired devices yet. Use “Pair a device” or the phone invitation above.</div>}
          <h3>Incoming connections</h3>{snapshot?.sessions.filter(s => s.approved).map(s => <div className="device-row" key={s.id}><Icon name="devices"/><div><strong>{s.name}</strong><small>{s.confirmed ? 'Confirmed on both devices' : 'Waiting for sender confirmation'}</small></div><button className="button secondary" onClick={() => void action(() => api(`/session/${s.id}`, { approve: false }), 'Connection revoked.')}>Revoke</button></div>)}</section>
        </>}

        {page === 'history' && <section className="panel"><div className="section-heading"><h2>Transfer history</h2><button className="text-button" onClick={() => void action(() => api('/history/clear', {}), 'Completed history cleared. Received files were not deleted.')}>Clear completed history</button></div>{snapshot?.transfers.length ? snapshot.transfers.map(t => <HistoryRow key={t.id} transfer={t}/>) : <p className="empty-copy">No transfers yet.</p>}<p className="subtle">Received files: {snapshot?.receiveDirectory}</p></section>}

        {page === 'settings' && <section className="panel settings"><div className="setting-row"><div><h3>Transfer notifications</h3><p>Ask your browser to notify you when a transfer changes state.</p></div><button className="button secondary" onClick={() => void action(async () => { if (!('Notification' in window)) throw new Error('This browser does not support desktop notifications.'); const permission = await Notification.requestPermission(); setNotifications(permission === 'granted'); if (permission !== 'granted') throw new Error('Notifications are blocked in browser settings.'); })}>{notifications ? 'Enabled for this session' : 'Enable notifications'}</button></div><div className="setting-row"><div><h3>Receive location</h3><p className="path">{snapshot?.receiveDirectory}</p><p>Each transfer gets its own folder. Existing files are never overwritten.</p></div></div><div className="setting-row"><div><h3>Network & privacy</h3><p>LAN only. TLS encryption. Explicit approval. No telemetry or cloud uploads.</p><p>Phone browser access is available in Devices after certificate setup. Native APK/IPA, remote transfer and persistent trusted devices remain unfinished.</p></div></div><div className="setting-row"><div><h3>Storage</h3><p>The desktop app’s native picker reads selected files directly. Browser selection and drag-and-drop stage a local copy before sending. Recovery copies are deleted once a transfer completes, and leftover prepared copies are cleared when Drift starts.</p></div></div></section>}
      </div><footer className="app-footer"><span>Drift · LAN preview</span></footer>
    </main>
    <dialog ref={dialog} onCancel={() => setPairOpen(false)} onClick={e => { if (e.target === dialog.current) setPairOpen(false); }} aria-labelledby="pair-title"><div className="dialog-content"><div className="section-heading"><h2 id="pair-title">A new connection.</h2><button className="icon-button" aria-label="Close pairing" onClick={() => setPairOpen(false)}><Icon name="close"/></button></div><p>Share this invitation privately with your other device. It expires in five minutes and works once.</p><div className="pair-invitation">{qr && <img src={qr} width="184" height="184" alt="Temporary Drift pairing invitation QR code"/>}<div><strong>This device</strong><p>{snapshot?.invitation.host}:{snapshot?.invitation.port}</p><code>{codeFormat(snapshot?.invitation.code ?? '')}</code><small>{snapshot ? `Expires ${new Date(snapshot.invitation.expires * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}` : ''}</small><button className="text-button" onClick={() => void action(() => api('/invitation', {}), 'A new invitation is ready.')}>Generate new code</button><button className="text-button" onClick={() => void action(() => navigator.clipboard.writeText(inviteUrl(snapshot!.invitation)), 'Invitation copied. Share it privately.')}>Copy invitation</button></div></div><form onSubmit={e => { e.preventDefault(); void action(connect); }}><h3>Connect to another device</h3><label htmlFor="invite">Paste its invitation or QR contents</label><textarea id="invite" rows={2} value={inviteText} onChange={e => setInviteText(e.target.value)} placeholder="drift://pair?invite=…"/><span className="form-or">Or enter its address and temporary code</span><div className="address-fields"><div><label htmlFor="host">Local IP address</label><input id="host" value={host} onChange={e => setHost(e.target.value)} placeholder="192.168.1.24"/></div><div><label htmlFor="port">Port</label><input id="port" inputMode="numeric" value={port} onChange={e => setPort(e.target.value)} placeholder="49152"/></div></div><label htmlFor="code">Pairing code</label><input id="code" value={code} onChange={e => setCode(e.target.value)} autoComplete="off" spellCheck={false} placeholder="The 30-character code on the other device"/><p className="subtle">No camera scanner in this preview. Paste scanned QR contents here. Never share this code publicly.</p><button className="button full" type="submit">Request connection</button></form></div></dialog>
  </div>;
}

function TransferRow({ transfer: t, rate, onAction }: { transfer: Transfer; rate: number; onAction: (path: string, body: unknown) => void }) {
  const percent = total(t) ? Math.min(100, done(t) / total(t) * 100) : 0;
  const eta = rate > 0 ? Math.ceil((total(t) - done(t)) / rate) : null;
  return <article className="transfer-card"><div className="transfer-top"><span className="file-avatar"><Icon name={t.entries.length > 1 ? 'folder' : 'file'}/></span><div className="transfer-title"><strong>{t.entries[0]?.path}{t.entries.length > 1 ? ` + ${t.entries.length - 1} more` : ''}</strong><small>{t.direction === 'send' ? 'To' : 'From'} {t.peer} · Direct LAN · {t.status}</small></div><strong className="transfer-percent">{Math.round(percent)}%</strong></div><progress value={done(t)} max={total(t) || 1} aria-label={`Transfer ${t.entries[0]?.path}`}/><div className="transfer-bottom"><span>{size(done(t))} of {size(total(t))}</span><span>{rate > 0 ? `${size(rate)}/s` : '—'}{eta !== null && eta > 0 ? ` · ${eta < 60 ? `${eta}s` : `${Math.ceil(eta / 60)} min`} left` : ''}</span></div>{t.error && <p className="inline-error">{t.error}</p>}<div className="transfer-actions">{t.direction === 'receive' && ['pending', 'paused'].includes(t.status) && <button className="button" onClick={() => onAction(`/receive/${t.id}`, { approve: true })}>Approve {t.entries.length} {t.entries.length === 1 ? 'file' : 'files'} ({size(total(t))})</button>}{t.direction === 'receive' && <details><summary>Review file list</summary><ul>{t.entries.map(e => <li key={e.path}>{e.path} — {size(e.size)}</li>)}</ul></details>}{['sending', 'receiving', 'awaiting approval'].includes(t.status) && <button className="text-button" onClick={() => onAction(`/control/${t.id}`, { action: 'paused' })}>Pause</button>}<button className="text-button" onClick={() => onAction(`/control/${t.id}`, { action: 'cancelled' })}>Cancel</button>{t.direction === 'send' && ['interrupted', 'paused'].includes(t.status) && <small>Use “Send / resume files” above to retry the selected batch.</small>}</div></article>;
}
function HistoryRow({ transfer: t }: { transfer: Transfer }) {
  return <div className="history-row"><span className="file-avatar small"><Icon name="file"/></span><div><strong>{t.entries[0]?.path}</strong><small>{t.direction === 'send' ? 'To' : 'From'} {t.peer} · {size(total(t))} · {new Date(t.updated * 1000).toLocaleString()}</small>{t.direction === 'receive' && t.status === 'complete' && window.driftMobile && <details><summary>Save received files</summary>{t.entries.map((entry,index) => <button className="text-button" key={entry.path} onClick={() => window.driftMobile?.saveReceived(t.id,index)}>Save {entry.path}</button>)}</details>}</div><span className={`history-status ${t.status === 'complete' ? 'verified' : ''}`}>{t.status === 'complete' ? <><Icon name="check"/>SHA-256 verified</> : t.status}</span></div>;
}

createRoot(document.getElementById('root')!).render(<React.StrictMode><App/></React.StrictMode>);
