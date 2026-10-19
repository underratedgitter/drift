const { contextBridge, ipcRenderer, webUtils } = require('electron');
contextBridge.exposeInMainWorld('drift', Object.freeze({
  chooseFiles: () => ipcRenderer.invoke('drift:choose-files'),
  chooseFolder: () => ipcRenderer.invoke('drift:choose-folder'),
  importDroppedFiles: files => {
    if (!Array.isArray(files) || !files.length || files.length > 10000) throw new Error('Drop 1–10,000 files or folders.');
    // Derive paths only from real OS-backed File objects. Never expose an API
    // accepting caller-supplied filesystem paths or the native engine token.
    const paths = files.map(file => {
      const source = webUtils.getPathForFile(file);
      if (!source) throw new Error('Drop files from your file manager, or use Choose files.');
      return source;
    });
    return ipcRenderer.invoke('drift:drop-files', paths);
  },
}));
