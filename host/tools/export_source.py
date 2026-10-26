#!/usr/bin/env python3
"""Create a source snapshot without Git history, local data, or vendor downloads."""
import argparse
import subprocess
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def source_files(directory, prefix=Path()):
    output = subprocess.check_output(['git', '-C', str(directory), 'ls-files', '-z', '--cached', '--others', '--exclude-standard'])
    for name in filter(None, output.decode().split('\0')):
        relative = prefix / name
        file = ROOT / relative
        if file.is_symlink():
            yield relative
        elif file.is_dir() and (file / '.git').exists():
            yield from source_files(file, relative)
        elif file.is_file():
            yield relative


def include(path):
    name = path.as_posix()
    if path.suffix.lower() == '.pdf':
        return False
    if name.startswith('doc/vendor/') and path.suffix.lower() not in {'.md'}:
        return False
    if name.startswith('doc/ProDoc_'):
        return False
    return not any(part in {'.git', '.aws', '.codex', '.agents', '.serial_mcp', 'target', 'build', 'BUILD', 'RELEASE'} for part in path.parts)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    files = sorted(set(path for path in source_files(ROOT) if include(path)))
    with tarfile.open(output, 'w:gz') as archive:
        for path in files:
            if (ROOT / path).resolve() == output:
                continue
            archive.add(ROOT / path, arcname=str(Path('PicoHarness') / path), recursive=False)
    print(f'{output}: {len(files)} source files; history and vendor downloads excluded')


if __name__ == '__main__':
    main()
