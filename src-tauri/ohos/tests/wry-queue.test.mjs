import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

// Run the patched upstream queue's pure-std tests on the build host. This
// checks real queue code without loading ArkUI, a WebView, or the OHOS app.
test('Wry queue orders operations and rejects stale generations', (context) => {
  const directory = mkdtempSync(path.join(tmpdir(), 'rssh-wry-queue-'));
  try {
    const source = fileURLToPath(new URL('../../target/ohos-sources/wry/src/ohos/queue.rs', import.meta.url));
    const harness = path.join(directory, 'queue-tests.rs');
    const binary = path.join(directory, process.platform === 'win32' ? 'queue-tests.exe' : 'queue-tests');
    writeFileSync(harness, `#[path = ${JSON.stringify(source)}]\nmod queue;\n`);
    execFileSync('rustc', ['--edition=2021', '--test', harness, '-o', binary], { stdio: 'pipe' });
    const output = execFileSync(binary, { encoding: 'utf8' });
    context.diagnostic(output.split('\n').find((line) => line.startsWith('test result:')));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
