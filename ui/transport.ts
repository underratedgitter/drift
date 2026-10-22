// One way to reach Drift's control API, whichever shell hosts this page:
// - the Drift app (Tauri): IPC into the engine running in the same process;
// - a browser on this computer (the `drift-engine` CLI): HTTP on loopback,
//   authorised by the private capability in the URL fragment.
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type SelectedBatch = { id: string; files: { path: string; size: number }[] };
type Reply = { status: number; body: unknown };
type Dropped = { kind: 'selected'; selection: SelectedBatch } | { kind: 'failed'; error: string };

export const inApp = isTauri();

// Browser mode only. The secret stays in this tab's memory: not localStorage,
// cookies, or any URL sent anywhere. Reloading needs the private URL again.
const token = inApp ? '' : location.hash.slice(1);
if (!inApp) {
  history.replaceState(null, '', location.pathname);
  window.addEventListener('hashchange', () => {
    if (/^#[a-f0-9]{64}$/.test(location.hash)) location.reload();
  });
}

function failure(status: number, body: unknown): Error {
  if (body && typeof body === 'object' && 'error' in body && typeof body.error === 'string') return new Error(body.error);
  if (typeof body === 'string' && body) return new Error(body);
  return new Error(`Drift could not complete that (${status}).`);
}

export async function api<T>(path: string, body?: unknown, method = 'POST'): Promise<T> {
  const verb = body === undefined && method === 'POST' ? 'GET' : method;
  if (inApp) {
    const reply = await invoke<Reply>('api', { method: verb, path, body: body ?? null });
    if (reply.status >= 400) throw failure(reply.status, reply.body);
    return reply.body as T;
  }
  const response = await fetch(`/api${path}`, {
    method: verb,
    headers: { Authorization: `Bearer ${token}`, ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  let parsed: unknown = text;
  try { parsed = JSON.parse(text); } catch { /* Plain-text error bodies are reported as they are. */ }
  if (!response.ok) throw failure(response.status, parsed);
  return parsed as T;
}

/** Copies one piece of a file picked in this page into the engine's outbox. */
export async function stage(batch: string, index: number, offset: number, piece: Blob): Promise<void> {
  const path = `/stage/${batch}/${index}?offset=${offset}`;
  if (inApp) {
    const reply = await invoke<Reply>('api_bytes', new Uint8Array(await piece.arrayBuffer()), { headers: { 'x-drift-path': path } });
    if (reply.status >= 400) throw failure(reply.status, reply.body);
    return;
  }
  const response = await fetch(`/api${path}`, { method: 'PUT', headers: { Authorization: `Bearer ${token}` }, body: piece });
  if (!response.ok) throw failure(response.status, await response.text());
}

/** The system file or folder picker; null when cancelled. Desktop app only. */
export function chooseNative(folder: boolean): Promise<SelectedBatch | null> {
  return invoke<SelectedBatch | null>('choose', { folder });
}

/** Shows a received transfer's saved files in the system file manager. */
export function reveal(transfer: string): Promise<void> {
  return invoke('reveal', { transfer });
}

type AppEvents = {
  dragging: (active: boolean) => void;
  dropped: (result: Dropped) => void;
  invitation: (url: string) => void;
};

/** Subscribes to the app shell's events; returns the unsubscribe function. */
export function onAppEvents(handlers: AppEvents): () => void {
  if (!inApp) return () => {};
  // Links wait in the app until taken, so one that opened Drift before this
  // page was listening is not lost, and none is handled twice.
  const take = () => void invoke<string | null>('take_invitation').then(url => { if (url) handlers.invitation(url); });
  const pending: Promise<UnlistenFn>[] = [
    listen<boolean>('dragging', event => handlers.dragging(event.payload)),
    listen<Dropped>('selection', event => handlers.dropped(event.payload)),
    listen('invitation', take),
  ];
  take();
  return () => { for (const unlisten of pending) void unlisten.then(stop => stop()); };
}
