export {};
declare global {
  interface Window {
    driftMobile?: { saveReceived: (transferId: string, index: number) => void };
    __driftInvitation?: string;
    drift?: {
      chooseFiles: () => Promise<{ id: string; files: { path: string; size: number }[] } | null>;
      chooseFolder: () => Promise<{ id: string; files: { path: string; size: number }[] } | null>;
      importDroppedFiles: (files: File[]) => Promise<{ id: string; files: { path: string; size: number }[] }>;
    };
  }
}
