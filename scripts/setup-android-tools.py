"""Install scoped Android build tools; never silently accepts license terms."""
import argparse
import hashlib
import json
import os
import platform
import stat
import subprocess
import tarfile
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOOLS = ROOT / '.toolchains'


def download(url, destination):
    if not url.startswith('https://'):
        raise ValueError('Build tools must use HTTPS')
    destination.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(['curl', '--proto', '=https', '--proto-redir', '=https', '--tlsv1.2', '--fail', '--location', '--retry', '3', '--connect-timeout', '30', url, '-o', str(destination)], check=True)


def verify(path, expected, algorithm):
    hasher = hashlib.new(algorithm)
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            hasher.update(block)
    if hasher.hexdigest() != expected.strip().lower():
        raise ValueError(f'Checksum mismatch: {path.name}')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--accept-android-license', action='store_true', help='Explicitly authorize acceptance of https://developer.android.com/studio/terms')
    options = parser.parse_args()
    if not options.accept_android_license:
        parser.error('Read the SDK terms and explicitly pass --accept-android-license, or install the SDK manually.')
    if platform.system() not in ('Darwin', 'Linux'):
        parser.error('This local bootstrap supports macOS/Linux; Windows builds can use runner-provided SDK tools.')
    TOOLS.mkdir(exist_ok=True)
    system = 'mac' if platform.system() == 'Darwin' else 'linux'
    architecture = 'aarch64' if platform.machine() in ('arm64', 'aarch64') else 'x64'
    metadata = TOOLS / 'jdk.json'
    download(f'https://api.adoptium.net/v3/assets/latest/21/hotspot?architecture={architecture}&os={system}&image_type=jdk', metadata)
    package = json.loads(metadata.read_text())[0]['binary']['package']
    archive = TOOLS / 'jdk.tar.gz'
    if not archive.exists():
        download(package['link'], archive)
    verify(archive, package['checksum'], 'sha256')
    jdk = TOOLS / 'jdk'
    jdk.mkdir(exist_ok=True)
    if not list(jdk.rglob('bin/java')):
        with tarfile.open(archive) as source:
            source.extractall(jdk, filter='data')
    java = next(jdk.rglob('bin/java')).parent.parent
    index = TOOLS / 'android-index.xml'
    download('https://dl.google.com/android/repository/repository2-3.xml', index)
    tree = ET.parse(index)
    remote = next(node for node in tree.iter() if node.tag.endswith('remotePackage') and node.attrib.get('path') == 'cmdline-tools;latest')
    archive_node = next(node for node in remote.findall('./archives/archive') if node.findtext('host-os') == ('macosx' if system == 'mac' else 'linux'))
    complete = archive_node.find('complete')
    sdk_archive = TOOLS / 'android-command-tools.zip'
    download('https://dl.google.com/android/repository/' + complete.findtext('url'), sdk_archive)
    checksum = complete.find('checksum')
    verify(sdk_archive, checksum.text, checksum.attrib.get('type', 'sha1'))
    sdk = TOOLS / 'android-sdk'
    command = sdk / 'cmdline-tools/latest'
    command.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(sdk_archive) as source:
        for member in source.infolist():
            relative = Path(member.filename)
            if not relative.parts or relative.parts[0] != 'cmdline-tools':
                continue
            relative = Path(*relative.parts[1:])
            destination = command / relative
            if not destination.resolve().is_relative_to(command.resolve()):
                raise ValueError('Unsafe SDK archive entry')
            if member.is_dir():
                destination.mkdir(parents=True, exist_ok=True)
            else:
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(source.read(member))
                mode = member.external_attr >> 16
                if stat.S_ISLNK(mode):
                    raise ValueError('Unexpected SDK symlink')
                destination.chmod((mode & 0o777) or 0o644)
    environment = {**os.environ, 'JAVA_HOME': str(java), 'ANDROID_HOME': str(sdk), 'ANDROID_SDK_ROOT': str(sdk)}
    manager = str(command / 'bin/sdkmanager')
    with (TOOLS / 'android-setup.log').open('w') as log:
        subprocess.run([manager, f'--sdk_root={sdk}', '--licenses'], input='y\n' * 100, text=True, stdout=log, stderr=subprocess.STDOUT, env=environment, check=True)
        print('Android SDK licenses accepted with explicit user authorization. Installing build packages…', flush=True)
        subprocess.run([manager, f'--sdk_root={sdk}', 'platform-tools', 'platforms;android-35', 'build-tools;35.0.0', 'ndk;28.0.13004108'], stdout=log, stderr=subprocess.STDOUT, env=environment, check=True)
    (TOOLS / 'environment.json').write_text(json.dumps({'JAVA_HOME': str(java), 'ANDROID_HOME': str(sdk), 'ANDROID_SDK_ROOT': str(sdk)}, indent=2))
    print('JDK and Android SDK ready. Build scripts read .toolchains/environment.json; no global shell configuration was changed.')


if __name__ == '__main__':
    main()
