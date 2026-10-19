"""Checksum-pinned LLVM/MinGW build toolchain; no changes to system compilers."""
import hashlib
import json
import platform
import subprocess
import tarfile
from pathlib import Path

if platform.system() != 'Darwin':
    raise SystemExit('This bootstrap is for macOS cross-builds. Windows runners use their native Rust toolchain.')
root = Path(__file__).resolve().parent.parent / '.toolchains/windows'
root.mkdir(parents=True, exist_ok=True)
name = 'llvm-mingw-20261006-ucrt-macos-universal'
archive = root / f'{name}.tar.xz'
url = f'https://github.com/mstorsjo/llvm-mingw/releases/download/20261006/{name}.tar.xz'
expected = 'a7a19b11048e89372637d26279cdb7c430ed3cf8c1e224310421312bf986ebf6'
if not archive.exists():
    subprocess.run(['curl', '--proto', '=https', '--proto-redir', '=https', '--tlsv1.2', '-fsSL', '--retry', '3', url, '-o', str(archive)], check=True)
hasher = hashlib.sha256()
with archive.open('rb') as source:
    for block in iter(lambda: source.read(1024 * 1024), b''):
        hasher.update(block)
if hasher.hexdigest() != expected:
    raise SystemExit('LLVM/MinGW checksum mismatch; no tools were executed.')
directory = root / name
if not directory.exists():
    with tarfile.open(archive) as source:
        source.extractall(root, filter='data')
compiler = directory / 'bin/x86_64-w64-mingw32-clang'
subprocess.run([str(compiler), '--version'], check=True)
(root / 'environment.json').write_text(json.dumps({'directory': str(directory)}, indent=2))
