import React, { useCallback, useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { api, chooseNative, inApp, onAppEvents, reveal, stage, type SelectedBatch } from './transport.js';
import './style.css';

type Invitation = { version: number; host: string; port: number; code: string; expires: number };
type Entry = { path: string; size: number; blake3: string };
type Transfer = { id: string; entries: Entry[]; offsets: number[]; status: string; peer: string; updated: number; error: string | null; direction: string };
// code: for a tapped nearby device, the six digits both screens must show.
type Session = { id: string; name: string; approved: boolean; confirmed: boolean; code?: string };
// remembered: paired and kept across restarts. online: seen on this network
// right now (null when discovery is off). own: its files arrive without asking.
type Device = { id: string; name: string; os?: string; host?: string; port?: number; confirmed?: boolean; remembered?: boolean; own?: boolean; online?: boolean | null; code?: string | null };
type SelectedFile = { file?: File; path: string; size: number };
type Snapshot = { name: string; os: string; invitation: Invitation; sessions: Session[]; connections: Device[]; transfers: Transfer[]; saved: Record<string, string[]>; peers: Device[]; discoveryError: string | null; lastError: string | null; receiveDirectory: string };
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
const platform = (os?: string) => os === 'macos' ? 'Mac' : os ? os[0].toUpperCase() + os.slice(1) : 'Device';
// macOS reports a refused Local Network permission as an unreachable host, and
// a refused Downloads permission as "Operation not permitted". Say what to do.
const explain = (message: string) => {
  if (/no route to host|host is unreachable|network is unreachable/i.test(message)) return `${message} If the other Mac is on the same Wi-Fi, allow Drift in System Settings › Privacy & Security › Local Network.`;
  if (/operation not permitted|permission denied/i.test(message) && /receive folder|save/i.test(message)) return `${message} Allow Drift to use your Downloads folder in System Settings › Privacy & Security › Files and Folders.`;
  return message;
};
const presence = (device: Device) => device.online === true ? 'On this network' : device.online === false ? 'Not on this network right now' : 'Ready';

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
  const [selected, setSelected] = useState('');
  const [files, setFiles] = useState<SelectedFile[]>([]);
  const [batch, setBatch] = useState('');
  const [busy, setBusy] = useState('');
  const [dragging, setDragging] = useState(false);
  const [notifications, setNotifications] = useState(false);
  const [ownChoice, setOwnChoice] = useState<Record<string, boolean>>({});
  const [rates, setRates] = useState<Record<string, number>>({});
  const previous = useRef<{ time: number; transfers: Transfer[] } | null>(null);
  const filePicker = useRef<HTMLInputElement>(null);
  const folderPicker = useRef<HTMLInputElement>(null);
  const dialog = useRef<HTMLDialogElement>(null);

  const refresh = useCallback(async () => {
    const next = await api<Snapshot>('/state');
    if (next.lastError) setError(explain(next.lastError));
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
          if (!inApp && notifications && Notification.permission === 'granted') new Notification('Drift', { body: message });
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
  useEffect(() => onAppEvents({
    dragging: setDragging,
    dropped: result => {
      if (result.kind === 'failed') { setError(result.error); return; }
      adoptSelection(result.selection);
    },
    invitation: url => { setInviteText(url); setPairOpen(true); },
  }), []);
  useEffect(() => { if (pairOpen) dialog.current?.showModal(); else dialog.current?.close(); }, [pairOpen]);

  const action = async (fn: () => Promise<unknown>, success?: string) => {
    setError('');
    try { await fn(); if (success) setNotice(success); await refresh(); } catch (e) { setNotice(''); setError(explain((e as Error).message)); }
  };
  const choose = (next: File[]) => { setFiles(next.map(file => ({ file, path: file.webkitRelativePath || file.name, size: file.size }))); setBatch(''); setError(''); };
  // Chosen with the system picker or dropped on the app: read in place, no copy.
  const adoptSelection = (selection: SelectedBatch) => { setFiles(selection.files.map(file => ({ path: file.path, size: file.size }))); setBatch(selection.id); setError(''); };
  const pick = async (folder: boolean) => {
    if (!inApp) { (folder ? folderPicker : filePicker).current?.click(); return; }
    await action(async () => { const selection = await chooseNative(folder); if (selection) adoptSelection(selection); });
  };
  const drop = (data: DataTransfer) => {
    const dropped = Array.from(data.files);
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
    await api<{ id: string }>('/pair', invitation);
    setPairOpen(false); setNotice('Request sent. Approve it on the other device; it will appear here as ready.');
  };
  // Tap-to-pair: both screens show six digits; the person accepts on the other device.
  const introduce = (device: Device) => void action(async () => {
    setNotice(`Contacting ${device.name}…`);
    const reply = await api<{ code: string; name: string }>('/introduce', { host: device.host, port: device.port, id: device.id });
    setNotice(`Check that ${reply.name} shows ${reply.code}, then accept there.`);
  });
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
          if (!file) throw new Error('Choose these files again.');
          for (let offset = 0; offset < file.size; offset += 1024 * 1024) {
            setBusy(`Preparing ${file.name} · ${Math.round(offset / file.size * 100)}%`);
            try { await stage(batchId, index, offset, file.slice(offset, offset + 1024 * 1024)); }
            catch (cause) { throw new Error(`Could not prepare ${file.name}. Check free space and choose it again.`, { cause }); }
          }
        }
        setBatch(batchId);
      }
      await api('/send', { connection: selected, batch: batchId });
      const own = ready.find(device => device.id === selected)?.own;
      setNotice(own === false ? 'Sent for approval. The other device will review the file list.' : 'Sending…');
    } catch (e) { setError((e as Error).message); }
    finally { setBusy(''); }
  };
  const localLinkMissing = snapshot?.invitation.host.startsWith('127.');
  const ready = snapshot?.connections.filter(c => c.confirmed) ?? [];
  const waiting = snapshot?.connections.filter(c => !c.confirmed) ?? [];
  // With one device ready, there is nothing to choose; with none, nothing chosen.
  useEffect(() => {
    if (ready.some(device => device.id === selected)) return;
    setSelected(ready.length === 1 ? ready[0].id : '');
  }, [ready.map(device => device.id).join(), selected]);
  const active = snapshot?.transfers.filter(t => !['complete', 'cancelled'].includes(t.status)) ?? [];
  const recent = snapshot?.transfers.filter(t => ['complete', 'cancelled'].includes(t.status)) ?? [];
  const pending = snapshot?.sessions.filter(s => !s.approved) ?? [];

  return <div className="app-shell">
    <aside className="sidebar">
      <a className="brand" href="#" onClick={e => { e.preventDefault(); setPage('send'); }} aria-label="Drift home">drift</a>
      <nav aria-label="Main navigation">{[['send', 'Send & receive', 'send'], ['devices', 'Devices', 'devices'], ['history', 'History', 'history'], ['settings', 'Settings', 'settings']].map(([id, title, icon]) => <button key={id} aria-label={title} aria-current={page === id ? 'page' : undefined} className={`nav-button ${page === id ? 'active' : ''}`} onClick={() => setPage(id)}><Icon name={icon}/>{title}{id === 'devices' && !!ready.length && <span className="count" aria-hidden="true">{ready.length}</span>}</button>)}</nav>
      <div className="own-device"><span className="device-avatar"><Icon name="devices"/></span><div><strong>{snapshot?.name ?? 'Connecting…'}</strong><small><span className="status-dot"/>This device · {snapshot ? platform(snapshot.os) : 'starting'}</small></div></div>
    </aside>
    <main>
      <header className="topbar"><span><span className="status-dot"/> {snapshot ? (localLinkMissing ? 'Not connected to Wi-Fi' : 'Ready to send and receive') : 'Starting…'}</span></header>
      <div className="content">
        <div className="page-heading"><div><h1>{page === 'send' ? 'Send & receive.' : page === 'devices' ? 'Your devices.' : page === 'history' ? 'Transfer history.' : 'Settings.'}</h1><p>{page === 'send' ? 'Choose your files. Pick a Mac. Send directly.' : page === 'devices' ? 'Pair once. Your Macs stay paired, in both directions.' : page === 'history' ? 'Files sent and received on this device.' : 'Your preferences, stored locally.'}</p></div>{page !== 'settings' && <button className="button secondary" onClick={() => setPairOpen(true)}><Icon name="plus"/>Pair a device</button>}</div>
        {error && <div className="alert error" role="alert"><span>{error}</span><button className="icon-button" aria-label="Dismiss error" onClick={() => setError('')}><Icon name="close"/></button></div>}
        {notice && <div className="alert" role="status"><span>{notice}</span><button className="icon-button" aria-label="Dismiss notification" onClick={() => setNotice('')}><Icon name="close"/></button></div>}
        {localLinkMissing && <p className="subtle">No Wi-Fi or Ethernet address was found. Connect both Macs to the same network, then quit and reopen Drift.</p>}
        {pending.map(session => { const own = ownChoice[session.id] ?? true; return <div className="approval" key={session.id}><Icon name="shield"/><div><strong>{session.name} wants to connect</strong>{session.code ? <p>Accept only if {session.name} shows the same code: <code className="short-code">{session.code}</code></p> : <p>Only approve if you started pairing on that device. Names are not proof of identity.</p>}<label className="check"><input type="checkbox" checked={own} onChange={e => setOwnChoice({ ...ownChoice, [session.id]: e.target.checked })}/>This is one of my devices: accept its files without asking</label></div><button className="button secondary" onClick={() => void action(() => api(`/session/${session.id}`, { approve: false }))}>Decline</button><button className="button" onClick={() => void action(() => api(`/session/${session.id}`, { approve: true, own }), `${session.name} is paired. Either device can now send to the other.`)}>Approve</button></div>; })}

        {page === 'send' && waiting.map(device => <div className="alert" key={device.id}><span>{device.code ? <>Waiting for {device.name} to accept. Both screens should show <code className="short-code">{device.code}</code></> : `Waiting for ${device.name} to approve the connection…`}</span></div>)}
        {page === 'send' && <>
          <div className="send-grid"><section className={`drop-zone ${dragging ? 'dragging' : ''}`} onDragOver={e => { e.preventDefault(); setDragging(true); }} onDragLeave={() => setDragging(false)} onDrop={e => { e.preventDefault(); setDragging(false); drop(e.dataTransfer); }} aria-label="File drop area">
            <h2>{files.length ? `${files.length} ${files.length === 1 ? 'file' : 'files'} ready to go` : 'Drop files here.'}</h2><p>{files.length ? `${size(files.reduce((sum, f) => sum + f.size, 0))} selected · directory structure preserved` : 'Or choose them below.'}</p>
            <div className="drop-actions"><button className="button" onClick={() => void pick(false)} disabled={!!busy}><Icon name="plus"/>Choose files</button><button className="text-button" onClick={() => void pick(true)} disabled={!!busy}><Icon name="folder"/>Choose folder</button></div>
            <input ref={filePicker} type="file" multiple hidden onChange={e => choose(Array.from(e.target.files ?? []))}/><input ref={folderPicker} type="file" multiple hidden {...{ webkitdirectory: '' }} onChange={e => choose(Array.from(e.target.files ?? []))}/>
            {files.length > 0 && <div className="send-controls"><label htmlFor="recipient">Send to</label><select id="recipient" value={selected} onChange={e => setSelected(e.target.value)}><option value="">{ready.length ? 'Choose a device' : 'Pair a device first'}</option>{ready.map(d => <option key={d.id} value={d.id}>{d.name}{d.online === false ? ' (not on this network)' : ''}</option>)}</select><button className="button" disabled={!!busy || !selected} onClick={() => void send()}>{busy || (batch ? 'Send / resume files' : 'Send files')}</button></div>}
            <div className="drop-footer"><Icon name="shield"/>Encrypted in transit. Verified on arrival.</div>
          </section>
          <section className="nearby">{ready.length > 0 && <><div className="section-heading"><h2>Your devices</h2></div><div className="device-list">{ready.map(device => <button className="device-row" key={device.id} onClick={() => setSelected(device.id)} aria-pressed={selected === device.id}><span className="device-avatar"><Icon name="devices"/></span><span><strong>{device.name}</strong><small>{`${platform(device.os)} · ${presence(device)}`}</small></span>{selected === device.id && <Icon name="check" className="connect-plus"/>}</button>)}</div></>}
            <div className="section-heading"><h2>Nearby devices</h2></div><p className="section-description">Macs running Drift on your network</p>
            {snapshot?.discoveryError && <p className="inline-error">Discovery unavailable. Pair using an address and code instead.</p>}
            {snapshot?.peers.length ? <div className="device-list">{snapshot.peers.map(device => <button className="device-row" key={device.id} onClick={() => introduce(device)}><span className="device-avatar"><Icon name="devices"/></span><span><strong>{device.name}</strong><small>{platform(device.os)} · Tap to pair</small></span><span className="connect-plus">+</span></button>)}</div> : <div className="nearby-empty"><p>No Macs found yet.</p><small>Open Drift on your other Mac, on the same Wi-Fi. If it never appears, pair with a code.</small></div>}
            <button className="button secondary full" onClick={() => setPairOpen(true)}>Connect with a code</button>
          </section></div>
          <section className="transfers-section"><div className="section-heading"><h2>Transfers <span className="muted">{active.length || ''}</span></h2></div>{active.length ? active.map(t => <TransferRow key={t.id} transfer={t} rate={rates[t.id] ?? 0} onAction={(path, body) => void action(() => api(path, body))}/>) : <p className="empty-copy">No active transfers.</p>}</section>
          {recent.length > 0 && <section className="recent-section"><div className="section-heading"><h2>Recent transfers</h2><button className="text-button" onClick={() => setPage('history')}>View history</button></div>{recent.slice(0, 3).map(t => <HistoryRow key={t.id} transfer={t} saved={snapshot?.saved[t.id]} onError={setError}/>)}</section>}
        </>}

        {page === 'devices' && <>
          <section className="panel"><h2>Paired devices</h2><p>Paired devices stay paired after restarts and can send to each other in both directions, until you forget them here.</p>
          {snapshot?.connections.length ? snapshot.connections.map(device => <div className="device-row" key={device.id}><span className="device-avatar"><Icon name="devices"/></span><div><strong>{device.name}</strong><small>{device.remembered ? `${platform(device.os)} · ${presence(device)}` : device.code ? `Waiting for approval · code ${device.code}` : 'Waiting for approval on that device…'}</small>{device.remembered && <label className="check"><input type="checkbox" checked={!!device.own} onChange={e => void action(() => api(`/devices/${device.id}`, { own: e.target.checked }), e.target.checked ? `Files from ${device.name} will arrive without asking.` : `You will be asked before files from ${device.name} arrive.`)}/>My device: accept its files without asking</label>}</div>{device.remembered && <button className="button secondary" onClick={() => void action(() => api(`/devices/${device.id}/forget`, {}), `${device.name} was forgotten on this device${device.online ? ' and on itself' : ''}.`)}>Forget</button>}</div>) : <div className="empty-line">No paired devices yet. Choose “Pair a device” to connect one.</div>}
</section>
        </>}

        {page === 'history' && <section className="panel"><div className="section-heading"><h2>Transfer history</h2><button className="text-button" onClick={() => void action(() => api('/history/clear', {}), 'Completed history cleared. Received files were not deleted.')}>Clear completed history</button></div>{snapshot?.transfers.length ? snapshot.transfers.map(t => <HistoryRow key={t.id} transfer={t} saved={snapshot.saved[t.id]} onError={setError}/>) : <p className="empty-copy">No transfers yet.</p>}<p className="subtle">Received files are saved in {snapshot?.receiveDirectory}</p></section>}

        {page === 'settings' && <section className="panel settings"><div className="setting-row"><div><h3>Notifications</h3><p>{inApp ? 'Drift notifies you when a device wants to connect, when files are waiting for your approval and when files arrive. Change this in System Settings › Notifications.' : 'Ask your browser to notify you when a transfer changes state.'}</p></div>{!inApp && <button className="button secondary" onClick={() => void action(async () => { if (!('Notification' in window)) throw new Error('This browser does not support desktop notifications.'); const permission = await Notification.requestPermission(); setNotifications(permission === 'granted'); if (permission !== 'granted') throw new Error('Notifications are blocked in browser settings.'); })}>{notifications ? 'Enabled for this session' : 'Enable notifications'}</button>}</div><div className="setting-row"><div><h3>Received files</h3><p className="path">{snapshot?.receiveDirectory}</p><p>Files and folders keep their names. If a name is taken, Drift adds a number, like “photo (2).jpg”. Nothing already there is ever replaced.</p></div></div><div className="setting-row"><div><h3>Network & privacy</h3><p>Files go directly between your Macs over your network, encrypted and verified on arrival. No account, no cloud, no tracking.</p>{inApp && <p>Closing the window keeps Drift running in the menu bar, so your other Macs can still send to it. Quit it from there.</p>}</div></div><div className="setting-row"><div><h3>Storage</h3><p>{inApp ? 'Files you choose or drop are read where they are, without a copy.' : 'Files you choose here are copied once into Drift before sending.'} Copies kept for resuming are deleted once a transfer completes.</p></div></div></section>}
      </div><footer className="app-footer"><span>Drift</span></footer>
    </main>
    <dialog ref={dialog} onCancel={() => setPairOpen(false)} onClick={e => { if (e.target === dialog.current) setPairOpen(false); }} aria-labelledby="pair-title"><div className="dialog-content"><div className="section-heading"><h2 id="pair-title">Pair with a code.</h2><button className="icon-button" aria-label="Close pairing" onClick={() => setPairOpen(false)}><Icon name="close"/></button></div><p>For when the other Mac does not appear under Nearby devices. This code expires in five minutes and works once.</p><div className="pair-invitation"><div><strong>This Mac</strong><p>{snapshot?.invitation.host}:{snapshot?.invitation.port}</p><code>{codeFormat(snapshot?.invitation.code ?? '')}</code><small>{snapshot ? `Expires ${new Date(snapshot.invitation.expires * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}` : ''}</small><button className="text-button" onClick={() => void action(() => api('/invitation', {}), 'A new code is ready.')}>Generate new code</button><button className="text-button" onClick={() => void action(() => navigator.clipboard.writeText(inviteUrl(snapshot!.invitation)), 'Invitation copied. With Universal Clipboard, paste it on your other Mac.')}>Copy invitation</button></div></div><form onSubmit={e => { e.preventDefault(); void action(connect); }}><h3>Connect to the other Mac</h3><label htmlFor="invite">Paste its invitation</label><textarea id="invite" rows={2} value={inviteText} onChange={e => setInviteText(e.target.value)} placeholder="drift://pair?invite=…"/><span className="form-or">Or enter its address and code</span><div className="address-fields"><div><label htmlFor="host">Local IP address</label><input id="host" value={host} onChange={e => setHost(e.target.value)} placeholder="192.168.1.24"/></div><div><label htmlFor="port">Port</label><input id="port" inputMode="numeric" value={port} onChange={e => setPort(e.target.value)} placeholder="49152"/></div></div><label htmlFor="code">Pairing code</label><input id="code" value={code} onChange={e => setCode(e.target.value)} autoComplete="off" spellCheck={false} placeholder="The 30-character code on the other Mac"/><p className="subtle">Never share this code publicly.</p><button className="button full" type="submit">Request connection</button></form></div></dialog>
  </div>;
}

function TransferRow({ transfer: t, rate, onAction }: { transfer: Transfer; rate: number; onAction: (path: string, body: unknown) => void }) {
  const percent = total(t) ? Math.min(100, done(t) / total(t) * 100) : 0;
  const eta = rate > 0 ? Math.ceil((total(t) - done(t)) / rate) : null;
  return <article className="transfer-card"><div className="transfer-top"><span className="file-avatar"><Icon name={t.entries.length > 1 ? 'folder' : 'file'}/></span><div className="transfer-title"><strong>{t.entries[0]?.path}{t.entries.length > 1 ? ` + ${t.entries.length - 1} more` : ''}</strong><small>{t.direction === 'send' ? 'To' : 'From'} {t.peer} · Direct LAN · {t.status}</small></div><strong className="transfer-percent">{Math.round(percent)}%</strong></div><progress value={done(t)} max={total(t) || 1} aria-label={`Transfer ${t.entries[0]?.path}`}/><div className="transfer-bottom"><span>{size(done(t))} of {size(total(t))}</span><span>{rate > 0 ? `${size(rate)}/s` : '—'}{eta !== null && eta > 0 ? ` · ${eta < 60 ? `${eta}s` : `${Math.ceil(eta / 60)} min`} left` : ''}</span></div>{t.error && <p className="inline-error">{t.error}</p>}<div className="transfer-actions">{t.direction === 'receive' && ['pending', 'paused'].includes(t.status) && <button className="button" onClick={() => onAction(`/receive/${t.id}`, { approve: true })}>Approve {t.entries.length} {t.entries.length === 1 ? 'file' : 'files'} ({size(total(t))})</button>}{t.direction === 'receive' && <details><summary>Review file list</summary><ul>{t.entries.map(e => <li key={e.path}>{e.path} — {size(e.size)}</li>)}</ul></details>}{['sending', 'receiving', 'awaiting approval'].includes(t.status) && <button className="text-button" onClick={() => onAction(`/control/${t.id}`, { action: 'paused' })}>Pause</button>}<button className="text-button" onClick={() => onAction(`/control/${t.id}`, { action: 'cancelled' })}>Cancel</button>{t.direction === 'send' && ['interrupted', 'paused'].includes(t.status) && <small>Use “Send / resume files” above to retry the selected batch.</small>}</div></article>;
}
function HistoryRow({ transfer: t, saved, onError }: { transfer: Transfer; saved?: string[]; onError: (message: string) => void }) {
  const name = t.entries[0]?.path.split('/')[0] ?? '';
  const label = t.entries.length > 1 && !t.entries.every(e => e.path.startsWith(`${name}/`)) ? `${name} + ${t.entries.length - 1} more` : name;
  return <div className="history-row"><span className="file-avatar small"><Icon name={t.entries.length > 1 ? 'folder' : 'file'}/></span><div><strong>{label}</strong><small>{t.direction === 'send' ? 'To' : 'From'} {t.peer} · {size(total(t))} · {new Date(t.updated * 1000).toLocaleString()}</small>{inApp && saved?.length ? <button className="text-button" onClick={() => void reveal(t.id).catch((e: Error) => onError(e.message))}><Icon name="folder"/>Show in folder</button> : null}</div><span className={`history-status ${t.status === 'complete' ? 'verified' : ''}`}>{t.status === 'complete' ? <><Icon name="check"/>Verified</> : t.status}</span></div>;
}

createRoot(document.getElementById('root')!).render(<React.StrictMode><App/></React.StrictMode>);
