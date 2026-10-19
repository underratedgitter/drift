import { isAbsolute, resolve } from 'node:path';

export function packagedDesktopExecutable(
  platform: string = process.platform,
  architecture: string = process.arch,
  override: string | undefined = process.env.DRIFT_TEST_PACKAGED_EXECUTABLE,
): string | null {
  if (override) {
    if (!isAbsolute(override)) throw new Error('DRIFT_TEST_PACKAGED_EXECUTABLE must be an absolute path.');
    return override;
  }
  if (platform === 'darwin' && ['x64', 'arm64'].includes(architecture)) {
    return resolve(`release/${architecture === 'arm64' ? 'mac-arm64' : 'mac'}/Drift.app/Contents/MacOS/Drift`);
  }
  if (platform === 'win32' && architecture === 'x64') return resolve('release/win-unpacked/Drift.exe');
  return null;
}
