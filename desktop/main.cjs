const { app, BrowserWindow, Menu, dialog, ipcMain, session } = require('electron');
const { spawn } = require('node:child_process');
const { randomBytes } = require('node:crypto');
const { existsSync } = require('node:fs');
const { readdir, lstat } = require('node:fs/promises');
const path = require('node:path');
const os = require('node:os');

let window;
let child;
let base;
let controlToken;
let quitting = false;
const nativeToken = randomBytes(32).toString('hex');
if (!app.requestSingleInstanceLock()) app.quit();
else {
  app.on('second-instance', () => { if (window) { window.show(); window.focus(); } });
  app.whenReady().then(start).catch(error => { dialog.showErrorBox('Drift could not start', error.message); app.quit(); });
}

async function start() {
  const root = path.resolve(__dirname, '..');
  const extension = process.platform === 'win32' ? '.exe' : '';
  const executable = process.env.DRIFT_ENGINE_PATH || (app.isPackaged ? path.join(process.resourcesPath, `drift-engine${extension}`) : path.join(root, `engine/target/debug/drift-engine${extension}`));
  if (!existsSync(executable)) throw new Error('The Rust engine is missing. Use npm run desktop:dev or rebuild the application.');
  const assets = app.isPackaged ? path.join(process.resourcesPath, 'ui') : path.join(root, 'dist/ui');
  const data = process.env.DRIFT_DATA_DIR || path.join(app.getPath('userData'), 'engine');
  const args = ['--ui', assets, '--data', data, '--ui-port', '0', '--phone-port', process.env.DRIFT_PHONE_PORT || '9443', '--name', os.hostname()];
  if (process.env.DRIFT_DISABLE_DISCOVERY === '1') args.push('--no-discovery');
  child = spawn(executable, args, { stdio: ['pipe', 'pipe', 'pipe'], env: { ...process.env, DRIFT_NATIVE_TOKEN: nativeToken, DRIFT_DESKTOP_PARENT: '1' }, windowsHide: true });
  const url = await new Promise((resolve, reject) => {
    let output = '', errors = '';
    const timeout = setTimeout(() => reject(new Error('Engine startup timed out. Close other Drift instances and try again.')), 30_000);
    child.on('error', error => { clearTimeout(timeout); reject(error); });
    child.stderr.on('data', data => { errors = (errors + data.toString()).slice(-4000); });
    child.once('exit', code => { clearTimeout(timeout); if (!base) reject(new Error(`Engine stopped (${code}). ${errors}`)); else if (!quitting) { dialog.showErrorBox('Transfer engine stopped', 'Transfers are paused. Quit and reopen Drift, then pair again to resume.'); app.quit(); } });
    child.stdout.on('data', data => {
      output = (output + data.toString()).slice(-8000);
      const match = output.match(/http:\/\/127\.0\.0\.1:\d+\/#([a-f0-9]{64})/);
      if (match) { clearTimeout(timeout); const parsed = new URL(match[0]); base = parsed.origin; controlToken = parsed.hash.slice(1); resolve(match[0]); }
    });
  });
  window = new BrowserWindow({ width: 1280, height: 880, minWidth: 760, minHeight: 600, show: false, icon: path.join(assets, 'icons/phone-512.png'),
    backgroundColor: '#000000', title: 'Drift', webPreferences: { preload: path.join(__dirname, 'preload.cjs'), sandbox: true, contextIsolation: true, nodeIntegration: false, webSecurity: true, spellcheck: false } });
  window.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  window.webContents.on('will-navigate', (event, url) => { if (new URL(url).origin !== base) event.preventDefault(); });
  session.defaultSession.setPermissionRequestHandler((contents, permission, callback) => {
    if (contents !== window.webContents || new URL(contents.getURL()).origin !== base || permission !== 'notifications') { callback(false); return; }
    void dialog.showMessageBox(window, { type: 'question', message: 'Allow Drift transfer notifications?', buttons: ['Allow', 'Not now'], defaultId: 1, cancelId: 1 }).then(result => callback(result.response === 0));
  });
  session.defaultSession.setPermissionCheckHandler((_contents, permission, origin) => origin === base && permission === 'clipboard-sanitized-write');
  session.defaultSession.on('will-download', (_event, item) => { item.setSaveDialogOptions({ title: 'Export public Drift phone certificate', defaultPath: path.join(app.getPath('downloads'), item.getFilename()) }); });
  const trustedSelection = event => {
    if (event.sender !== window.webContents || event.senderFrame !== window.webContents.mainFrame || new URL(event.senderFrame.url).origin !== base) throw new Error('Untrusted file-selection request');
  };
  const importSources = async (event, sources) => {
    trustedSelection(event);
    if (!Array.isArray(sources) || !sources.length || sources.length > 10000 || sources.some(source => typeof source !== 'string' || !path.isAbsolute(source) || source.length > 32768)) throw new Error('Invalid native file selection');
    const files = [];
    const walk = async (source, relative, depth) => {
      if (depth > 32 || files.length >= 10000) throw new Error('Choose at most 10,000 files with folders no more than 32 levels deep.');
      const stat = await lstat(source);
      if (stat.isSymbolicLink()) throw new Error('Symlinks are not transferred. Choose their target files explicitly.');
      if (stat.isFile()) files.push({ source, path: relative.split(path.sep).join('/') });
      else if (stat.isDirectory()) for (const name of await readdir(source)) await walk(path.join(source, name), path.join(relative, name), depth + 1);
    };
    for (const source of sources) await walk(source, path.basename(source), 0);
    if (!files.length) throw new Error('This selection contains no regular files. Empty directories are not transferred.');
    const response = await fetch(`${base}/api/native/batch`, { method: 'POST', headers: { Authorization: `Bearer ${controlToken}`, 'X-Drift-Native': nativeToken, 'Content-Type': 'application/json' }, body: JSON.stringify(files) });
    const body = await response.json();
    if (!response.ok) throw new Error(body.error || 'File selection failed');
    return body;
  };
  const choose = async (event, folder) => {
    trustedSelection(event);
    const selection = await dialog.showOpenDialog(window, { title: folder ? 'Choose a folder to send' : 'Choose files to send', properties: folder ? ['openDirectory'] : ['openFile', 'multiSelections'] });
    if (selection.canceled) return null;
    return importSources(event, selection.filePaths);
  };
  ipcMain.handle('drift:choose-files', event => choose(event, false));
  ipcMain.handle('drift:choose-folder', event => choose(event, true));
  ipcMain.handle('drift:drop-files', importSources);
  Menu.setApplicationMenu(Menu.buildFromTemplate([
    { label: 'Drift', submenu: [{ role: 'about' }, { type: 'separator' }, { role: 'hide' }, { role: 'hideOthers' }, { role: 'unhide' }, { type: 'separator' }, { role: 'quit' }] },
    { label: 'Edit', submenu: [{ role: 'undo' }, { role: 'redo' }, { type: 'separator' }, { role: 'cut' }, { role: 'copy' }, { role: 'paste' }, { role: 'selectAll' }] },
    { label: 'View', submenu: [{ role: 'resetZoom' }, { role: 'zoomIn' }, { role: 'zoomOut' }, { role: 'togglefullscreen' }] },
    { label: 'Window', submenu: [{ role: 'minimize' }, { role: 'zoom' }, { role: 'front' }] },
  ]));
  window.on('close', event => { if (process.platform === 'darwin' && !quitting) { event.preventDefault(); window.hide(); } });
  await window.loadURL(url); window.show();
}

app.on('activate', () => { if (window) window.show(); });
app.on('window-all-closed', () => { if (process.platform !== 'darwin') app.quit(); });
app.on('before-quit', event => {
  quitting = true;
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  event.preventDefault();
  child.stdin.end();
  const timeout = setTimeout(() => { child.kill('SIGKILL'); app.exit(0); }, 3000);
  child.once('exit', () => { clearTimeout(timeout); app.exit(0); });
});
