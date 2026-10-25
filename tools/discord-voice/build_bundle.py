#!/usr/bin/env python3
"""Build an offline Linux development bundle; never installs, enables or logs in."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

SOURCE = Path(__file__).resolve().parent
MODULES = ('worker', 'driver', 'connector', 'credentials', 'identify', 'host-io',
           'action-sequence', 'receive-bridge', 'receiver', 'playback-queue', 'playback-feedback', 'playback', 'playback-binding', 'codec', 'check')


def build(output, node, mode="receive"):
    if mode not in ("receive", "playback", "duplex"):
        raise ValueError("Unsupported voice direction")
    label = {"receive": "Listen", "playback": "Speak", "duplex": "Listen + speak"}[mode]
    output = Path(output).absolute()
    node = Path(node).resolve(strict=True)
    if output.exists():
        raise ValueError('Output already exists; choose a new bundle directory')
    output.parent.mkdir(parents=True, exist_ok=True)
    info = json.loads(subprocess.check_output([str(node), '-p',
        'JSON.stringify({platform:process.platform,arch:process.arch,node:process.version,abi:process.versions.modules})'], env={}, timeout=10))
    if info['platform'] != 'linux' or tuple(map(int, info['node'][1:].split('.'))) < (22, 12, 0):
        raise ValueError('Requires Linux Node >=22.12')
    package = json.loads((SOURCE / 'package.json').read_text())
    lock = json.loads((SOURCE / 'package-lock.json').read_text())
    for name, version in package['dependencies'].items():
        installed = json.loads((SOURCE / 'node_modules' / name / 'package.json').read_text())
        if installed['version'] != version or lock['packages']['node_modules/' + name]['version'] != version:
            raise ValueError('Installed dependency differs from pinned version: ' + name)
    with tempfile.TemporaryDirectory(prefix='.voice-bundle-', dir=output.parent) as temporary:
        stage = Path(temporary) / 'bundle'
        stage.mkdir()
        for module in MODULES:
            shutil.copy2(SOURCE / (module + '.mjs'), stage)
        for name in ('package.json', 'package-lock.json', 'README.md'):
            shutil.copy2(SOURCE / name, stage)
        dependencies = SOURCE / 'node_modules'
        for source in dependencies.rglob('*'):
            relative = source.relative_to(dependencies)
            if any(part == '.bin' or part.startswith('build-tmp-') for part in relative.parts):
                continue
            if source.is_symlink():
                raise ValueError('Unexpected dependency symlink: ' + str(relative))
            if source.is_dir() or source.suffix in ('.map', '.ts', '.mts', '.cts') or source.name == '.package-lock.json':
                continue
            if not source.is_file():
                raise ValueError('Nonregular dependency entry: ' + str(relative))
            target = stage / 'node_modules' / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
        runtime = stage / 'runtime'
        runtime.mkdir()
        shutil.copy2(node, runtime / 'node')
        shutil.copy2(node.parent.parent / 'LICENSE', runtime / 'LICENSE')
        (stage / 'run.sh').write_text('#!/bin/sh\nbase=${0%/*}\nexec "$base/runtime/node" "$base/worker.mjs" "$@"\n')
        (stage / 'run.sh').chmod(0o755)
        manifest = {
            'api_version': 1, 'id': 'studio.integration.discord-voice', 'version': '0.1.0',
            'name': f'Discord voice · {label} (experimental)',
            'description': {
                'receive': 'Listen to participants using your transcription settings. Does not speak in the call.',
                'playback': 'Speak in one voice channel using your character voice. Does not listen to participants.',
                'duplex': 'Listen to participants and speak replies using your character voice.',
            }[mode] + ' Requires a Discord bot.',
            'provenance': {'author': 'Waifu Studio contributors', 'license': 'MIT'},
            'process': {'program': 'run.sh'},
            'capabilities': ((['audio.transcribe', 'message.deliver', 'lifecycle.observe'] if mode != 'playback' else [])
                             + (['audio.playback'] if mode != 'receive' else [])),
            'credentials': [{'id': 'bot_token', 'label': 'Discord bot token'}],
            'settings': [dict(id=key, label=setting_label, type='text', default='', max_length=20, description=help_text)
                         for key, setting_label, help_text in [
                             ('guild_id', 'Server ID', 'In Discord developer mode, right-click the server and copy its ID.'),
                             ('channel_id', 'Voice channel ID', 'Right-click the voice channel and copy its ID.')]]}
        (stage / 'extension.json').write_text(json.dumps(manifest, indent=2) + '\n')
        (stage / 'bundle-platform.json').write_text(json.dumps(info, indent=2) + '\n')
        entries = list(stage.rglob('*'))
        if len(entries) > 4096 or any(len(p.relative_to(stage).parts) > 17 for p in entries):
            raise ValueError('Bundle exceeds installer entry/depth limits')
        if sum(p.stat().st_size for p in entries if p.is_file()) > 2 * 1024**3:
            raise ValueError('Bundle exceeds installer size limit')
        # Preflight imports and executes real codecs only, never a Client/login.
        result = subprocess.run([str(runtime / 'node'), str(stage / 'check.mjs')],
                                env={}, check=True, capture_output=True, text=True, timeout=30)
        receipt = json.loads(result.stdout)
        if receipt.get('status') != 'ok' or receipt.get('network_used') is not False:
            raise ValueError('Bundle preflight failed')
        stage.rename(output)
        return dict(output=str(output), entries=len(entries), platform=info, preflight=receipt)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--node', default=shutil.which('node'))
    parser.add_argument('--mode', choices=('receive', 'playback', 'duplex'), default='receive')
    args = parser.parse_args()
    print(json.dumps(build(args.output, args.node, args.mode), indent=2))
