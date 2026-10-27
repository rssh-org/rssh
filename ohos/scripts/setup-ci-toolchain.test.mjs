import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = fileURLToPath(new URL('../..', import.meta.url));
const downloadUrl = 'https://contentcenter-vali-drcn.dbankcdn.cn/fixture.zip?signature=must-not-appear-in-logs';

function install(t, failure) {
  const root = realpathSync(mkdtempSync(path.join(tmpdir(), 'rssh ohos toolchain ')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const name of ['ohos/scripts', 'tools', 'runner']) mkdirSync(path.join(root, name), { recursive: true });
  const script = path.join(root, 'ohos/scripts/setup-ci-toolchain.sh');
  copyFileSync(path.join(repo, 'ohos/scripts/setup-ci-toolchain.sh'), script);
  const archive = path.join(root, 'fixture.zip');
  const packed = spawnSync('python3', ['-c', `
import sys, zipfile
with zipfile.ZipFile(sys.argv[1], 'w') as archive:
    archive.writestr('command-line-tools/sdk/fixture', 'official bundle fixture')
`, archive], { encoding: 'utf8' });
  assert.equal(packed.status, 0, packed.stderr);
  const config = JSON.parse(readFileSync(path.join(repo, 'ohos/ci-toolchain.json')));
  config.sha256 = failure === 'checksum' ? '0'.repeat(64) : createHash('sha256').update(readFileSync(archive)).digest('hex');
  writeFileSync(path.join(root, 'ohos/ci-toolchain.json'), JSON.stringify(config));
  const mockTool = `#!${process.execPath}
const fs = require('node:fs');
if (process.argv[1].endsWith('/uname')) {
  console.log(process.argv[2] === '-s' ? 'Linux' : 'x86_64');
} else {
  if (process.env.FAIL_DOWNLOAD === 'true') process.exit(22);
  const args = process.argv.slice(2);
  fs.copyFileSync(process.env.FIXTURE_ARCHIVE, args[args.indexOf('--output') + 1]);
}
`;
  for (const name of ['uname', 'curl']) {
    const file = path.join(root, 'tools', name);
    writeFileSync(file, mockTool);
    chmodSync(file, 0o755);
  }
  const runner = path.join(root, 'runner');
  for (const name of ['env', 'path']) writeFileSync(path.join(runner, name), '');
  const result = spawnSync('bash', [script], {
    encoding: 'utf8',
    env: {
      ...process.env,
      PATH: `${root}/tools${path.delimiter}${process.env.PATH}`,
      FIXTURE_ARCHIVE: archive,
      FAIL_DOWNLOAD: String(failure === 'download'),
      RUNNER_TEMP: runner,
      GITHUB_ENV: path.join(runner, 'env'),
      GITHUB_PATH: path.join(runner, 'path'),
      OHOS_COMMAND_LINE_TOOLS_URL: failure === 'missing-url' ? '' : downloadUrl,
    },
  });
  assert.doesNotMatch(result.stdout + result.stderr, /must-not-appear-in-logs/);
  assert.ok(!readdirSync(runner, { recursive: true }).some((file) => file.endsWith('.zip')));
  return {
    ...result, runner,
    environment: readFileSync(path.join(runner, 'env'), 'utf8'),
    paths: readFileSync(path.join(runner, 'path'), 'utf8'),
  };
}

test('verified SDK installation extracts the archive and exports all build paths', (t) => {
  const result = install(t);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  const environment = Object.fromEntries(result.environment.trim().split('\n').map((line) => line.split('=')));
  const tools = path.dirname(environment.DEVECO_SDK_HOME);
  assert.equal(readFileSync(path.join(tools, 'sdk/fixture'), 'utf8'), 'official bundle fixture');
  assert.deepEqual(environment, {
    OHOS_HOME: `${tools}/sdk/default/openharmony`,
    OHOS_NDK_HOME: `${tools}/sdk/default/openharmony`,
    DEVECO_SDK_HOME: `${tools}/sdk`,
    DEVECO_NODE_HOME: `${tools}/tool/node`,
    NODE_HOME: `${tools}/tool/node`,
    HVIGOR_BIN: `${tools}/hvigor/bin/hvigorw.js`,
    OHPM_BIN: `${tools}/bin/ohpm`,
  });
  assert.deepEqual(result.paths.trim().split('\n'), [
    `${tools}/tool/node/bin`, `${tools}/bin`, `${tools}/sdk/default/openharmony/toolchains`,
  ]);
});

for (const [failure, message] of [
  ['missing-url', /OHOS_COMMAND_LINE_TOOLS_URL/],
  ['download', null],
  ['checksum', /SHA-256 does not match/],
]) {
  test(`${failure} failure leaves no SDK or exported build environment`, (t) => {
    const result = install(t, failure);
    assert.notEqual(result.status, 0);
    assert.equal(result.environment, '');
    assert.equal(result.paths, '');
    assert.deepEqual(readdirSync(result.runner).sort(), ['env', 'path']);
    if (message) assert.match(result.stderr, message);
  });
}
