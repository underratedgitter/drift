export {};
declare global {
  interface Window {
    // Tauri injects this into the Drift app's web view; absent in browsers.
    __TAURI_INTERNALS__?: unknown;
  }
}
