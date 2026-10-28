#!/usr/bin/env python3
"""Build and package Home as a release archive with its installer and resources."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]


def run(*args, capture=False):
    result = subprocess.run(args, cwd=ROOT, check=True, text=True,
                            stdout=subprocess.PIPE if capture else None)
    return result.stdout.strip() if capture else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--no-build', action='store_true', help='Package an already-built locked workspace')
    args = parser.parse_args()
    spec = json.loads((ROOT / 'packaging.json').read_text())
    manifest = ROOT / spec['manifest']
    if not args.no_build:
        for command in spec.get('prepare', []):
            run(*command)
        command = ['cargo', 'build', '--release', '--locked', '--manifest-path', str(manifest)]
        for feature in os.environ.get('NYX_BUILD_FEATURES', '').split(','):
            if feature:
                command += ['--features', feature]
        for package in spec['packages']:
            command += ['-p', package]
        run(*command)
    metadata = json.loads(run('cargo', 'metadata', '--locked', '--no-deps', '--format-version',
                              '1', '--manifest-path', str(manifest), capture=True))
    binaries = set(spec.get('binaries', []))
    for package in metadata['packages']:
        if package['name'] in spec['packages']:
            binaries.update(t['name'] for t in package['targets'] if 'bin' in t['kind'])
    resources = {}
    names = run('git', 'ls-files', capture=True).splitlines()
    for name in names:
        source = ROOT / name
        if source.is_file():
            resources[name] = source
    for binary in binaries:
        source = Path(metadata['target_directory']) / 'release' / binary
        if not source.is_file():
            raise RuntimeError(f'Missing built binary {source}')
        resources['target/release/' + binary] = source
    revision = run('git', 'rev-parse', 'HEAD', capture=True)
    build_info = {'revision': revision, 'application': spec['application'],
                  'rustc': run('rustc', '--version', capture=True),
                  'cargo': run('cargo', '--version', capture=True),
                  'target': platform.machine(), 'features': os.environ.get('NYX_BUILD_FEATURES', ''),
                  'environment': json.loads((ROOT / 'build-environment.json').read_text())}
    (ROOT / 'dist').mkdir(exist_ok=True)
    (ROOT / 'dist/build-info.json').write_text(json.dumps(build_info, indent=2) + '\n')
    epoch = int(os.environ.get('SOURCE_DATE_EPOCH', run('git', 'log', '-1', '--format=%ct', capture=True)))
    output = ROOT / 'dist' / f"{spec['application']}-{platform.machine()}.tar.gz"
    output.parent.mkdir(exist_ok=True)
    with output.open('wb') as raw, gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode='w') as archive:
            marker = tarfile.TarInfo('.prebuilt')
            marker.mtime, marker.mode = epoch, 0o644
            archive.addfile(marker, io.BytesIO(b''))
            for name, source in sorted(resources.items()):
                info = tarfile.TarInfo(name)
                info.size = source.stat().st_size
                info.mode = 0o755 if os.access(source, os.X_OK) else 0o644
                info.mtime = epoch
                with source.open('rb') as content:
                    archive.addfile(info, content)
    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    write = output.with_suffix(output.suffix + '.sha256')
    write.write_text(f'{digest}  {output.name}\n')
    print(f'{digest}  {output}', flush=True)


if __name__ == '__main__':
    main()
