"""Build and execute public CLI sources using only generated test inputs."""
from pathlib import Path
import datetime
import hashlib
import json
import os
import platform
import re
import struct
import subprocess
import tempfile



ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT
OUT = ROOT / 'ci-output/kurosaki'
OUT.mkdir(parents=True, exist_ok=True)
ENV = dict(os.environ, CARGO_INCREMENTAL='0')
COMMANDS = []


def run(name, args, env=ENV):
    log = OUT / (name + '.log')
    with log.open('wb') as stream:
        result = subprocess.run(args, cwd=SOURCE, env=env, stdout=stream,
                                stderr=subprocess.STDOUT, timeout=1500)
    output = log.read_text(encoding='utf-8', errors='replace')
    row = {'name': name, 'arguments': args, 'exit_code': result.returncode,
           'test_count': sum(map(int, re.findall(r'test result: ok\. (\d+) passed;', output)))}
    COMMANDS.append(row)
    print(json.dumps(row), flush=True)
    if result.returncode:
        print(output[-6000:], flush=True)
        raise SystemExit(result.returncode)
    return output


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=SOURCE, text=True).strip()
assert commit == os.environ['KUROSAKI_SOURCE_COMMIT']
tracked = subprocess.check_output(['git', 'ls-files', '-z'], cwd=SOURCE).decode().split('\0')
source_files = [{'path': p, 'sha256': sha(SOURCE / p)} for p in tracked
                if p and Path(p).suffix in {'.rs', '.toml', '.lock'}]
for row in source_files:
    blob = subprocess.check_output(['git', 'show', 'HEAD:' + row['path']], cwd=SOURCE)
    assert hashlib.sha256(blob).hexdigest() == row['sha256'], 'Checkout changed source bytes: ' + row['path']
toolchain = run('toolchain', ['rustc', '--version', '--verbose'])
run('tests', ['cargo', 'test', '--locked', '--workspace', '--all-targets', '--exclude', 'kurosaki-py'])
run('clippy', ['cargo', 'clippy', '--locked', '--workspace', '--all-targets', '--exclude',
               'kurosaki-py', '--', '-D', 'warnings'])
release_env = dict(ENV)
if platform.system() == 'Windows':
    release_env['RUSTFLAGS'] = '-C target-feature=+crt-static'
run('release', ['cargo', 'build', '--locked', '--release', '-p', 'kurosaki-cli',
                '--bin', 'kurosaki'], release_env)
target = SOURCE / 'target/release'
executable = target / ('kurosaki.exe' if platform.system() == 'Windows' else 'kurosaki')
run('cli-help', [str(executable), '--help'])
native = {'checks': [{'case': 'cli-help', 'passed': True}]}


def invoke(args):
    result = subprocess.run([str(executable), *map(str, args)], capture_output=True, timeout=90)
    assert result.returncode == 0, result.stderr.decode(errors='replace')[-2000:]


def exact(left, right):
    if isinstance(left, float):
        return isinstance(right, float) and struct.pack('>d', left) == struct.pack('>d', right)
    if isinstance(left, dict):
        return left.keys() == right.keys() and all(exact(v, right[k]) for k, v in left.items())
    if isinstance(left, list):
        return len(left) == len(right) and all(exact(a, b) for a, b in zip(left, right))
    return left == right


with tempfile.TemporaryDirectory(prefix='kurosaki-precision-') as folder:
    work = Path(folder)
    rom = bytearray([0x60] * (16 + 16384))
    rom[:16] = bytes(16)
    rom[:4] = b'NES\x1a'
    rom[4] = 1
    code = bytes.fromhex('A9 08 8D 15 40 A9 1F 8D 0C 40 A9 07 8D 0E 40 A9 08 8D 0F 40 4C 14 80')
    rom[16:16 + len(code)] = code
    for offset, pc in [(0x3FFA, 0x8030), (0x3FFC, 0x8000), (0x3FFE, 0x8038)]:
        rom[16 + offset:18 + offset] = pc.to_bytes(2, 'little')
    rom_path = work / 'synthetic.nes'
    rom_path.write_bytes(rom)
    baseline = work / 'baseline.json'
    invoke(['snapshot-save', rom_path, '--frames', '2', '--out', baseline])
    state = json.loads(baseline.read_text())
    # Trace history is observation metadata, not resumed execution state.
    state['last_event_count'] = 0
    bits = 0x4D595DF4D0F33173
    for index in range(32):
        bits = (bits * 6364136223846793005 + 1) & ((1 << 64) - 1)
        fraction = struct.unpack('>d', ((1020 << 52) | (bits & ((1 << 52) - 1))).to_bytes(8, 'big'))[0]
        state['bus']['apu']['sample_clock_accum'] = fraction
        state['bus']['apu']['noise_clock_accum'] = fraction
        start, loaded, resumed = [work / (s + '.json') for s in ['start', 'loaded', 'resumed']]
        start.write_text(json.dumps(state), encoding='utf-8')
        invoke(['snapshot-load', start, '--json', loaded])
        assert exact(state, json.loads(loaded.read_text()))
        invoke(['snapshot-resume', rom_path, start, '--frames', '0', '--out', resumed])
        assert exact(state, json.loads(resumed.read_text()))
    state['cpu']['pc'] = 0x8040
    start.write_text(json.dumps(state), encoding='utf-8')
    report = work / 'decompile.json'
    invoke(['decompile', rom_path, '--snapshot', start, '--max-instructions', '64',
            '--format', 'json', '--out', report])
    functions = json.loads(report.read_text())['functions']
    current = next(f for f in functions if f['start']['cpu_addr'] == 0x8040)
    assert current['source_kind'] == 'current_pc' and current['name'] != 'Reset'
    assert any(f['start']['cpu_addr'] == 0x8000 and f['source_kind'] == 'reset_vector'
               for f in functions)

if platform.system() == 'Darwin':
    dependencies = subprocess.check_output(['otool', '-L', str(executable)], text=True)
elif platform.system() == 'Linux':
    dependencies = subprocess.check_output(['ldd', str(executable)], text=True)
else:
    dependencies = None
result = {'passed': True, 'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
          'source_commit': commit, 'validation_commit': os.environ.get('GITHUB_SHA'),
          'system': platform.system(), 'machine': platform.machine(), 'toolchain': toolchain,
          'scope': 'Public CLI/core; generated fixtures only; GUI and Python binary not packaged',
          'commands': COMMANDS, 'source_files': source_files, 'native_smoke': native,
          'release_fraction_cases': 32, 'release_current_pc_classification_verified': True,
          'binary_sha256': sha(executable), 'executable_dependencies': dependencies}
(OUT / 'verification.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
(OUT / executable.name).write_bytes(executable.read_bytes())
print(json.dumps({'passed': True, 'tests': sum(r['test_count'] for r in COMMANDS),
                  'system': result['system'], 'machine': result['machine']}), flush=True)
