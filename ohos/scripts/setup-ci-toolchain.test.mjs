import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = fileURLToPath(new URL('../..', import.meta.url));
const downloadUrl = 'https://contentcenter-vali-drcn.dbankcdn.cn/fixture.zip?signature=must-not-appear-in-logs';

function fixture(t) {
  const root = realpathSync(mkdtempSync(path.join(tmpdir(), 'rssh ohos toolchain ')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scripts = path.join(root, 'ohos/scripts');
  const tools = path.join(root, 'fake-tools');
  const runner = path.join(root, 'runner');
  for (const directory of [scripts, tools, runner]) mkdirSync(directory, { recursive: true });
  const script = path.join(scripts, 'setup-ci-toolchain.sh');
  copyFileSync(path.join(repo, 'ohos/scripts/setup-ci-toolchain.sh'), script);
  const config = JSON.parse(readFileSync(path.join(repo, 'ohos/ci-toolchain.json'), 'utf8'));
  const archive = path.join(root, 'fixture.zip');
  const files = {
    'tool/node/bin/node': `#!/bin/sh\necho v${config.nodeVersion}\n`,
    'bin/ohpm': '#!/bin/sh\nexit 0\n',
    'sdk/default/openharmony/native/llvm/bin/clang': '#!/bin/sh\nexit 0\n',
    'hvigor/bin/hvigorw.js': '// fixture\n',
    'sdk/default/openharmony/toolchains/lib/hap-sign-tool.jar': 'fixture signing tool',
    'sdk/default/openharmony/toolchains/lib/app_packing_tool.jar': 'fixture packing tool',
    'sdk/default/sdk-pkg.json': JSON.stringify({ data: { version: config.sdkVersion, apiVersion: config.apiVersion } }),
    'hvigor/hvigor/package.json': JSON.stringify({ version: config.hvigorVersion }),
    'ohpm/package.json': JSON.stringify({ version: config.ohpmVersion }),
  };
  const pack = spawnSync('python3', ['-c', `
import json, sys, zipfile
with zipfile.ZipFile(sys.argv[1], 'w') as archive:
    for name, content in json.load(sys.stdin).items():
        info = zipfile.ZipInfo('command-line-tools/' + name)
        info.create_system = 3
        info.external_attr = 0o100755 << 16
        archive.writestr(info, content)
`, archive], { encoding: 'utf8', input: JSON.stringify(files) });
  assert.equal(pack.status, 0, pack.stdout + pack.stderr);
  config.sha256 = createHash('sha256').update(readFileSync(archive)).digest('hex');
  const manifest = path.join(root, 'ohos/ci-toolchain.json');
  writeFileSync(manifest, JSON.stringify(config));
  const mockTool = `#!${process.execPath}
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const tool = path.basename(process.argv[1]);
const args = process.argv.slice(2);
if (tool === 'uname') {
  console.log(args[0] === '-s' ? 'Linux' : 'x86_64');
} else if (tool === 'curl') {
  fs.appendFileSync(process.env.FIXTURE_CALLS, 'download\\n');
  if (process.env.FAIL_DOWNLOAD === '1') process.exit(22);
  const output = args[args.indexOf('--output') + 1];
  if (!output || !args.includes('--fail') || !args.includes('--location')) process.exit(9);
  fs.copyFileSync(process.env.FIXTURE_ARCHIVE, output);
} else if (tool === 'sha256sum') {
  // macOS's sha256sum does not implement GNU's stdin check mode.
  if (args.length !== 1 || args[0] !== '-c') process.exit(9);
  const [expected, filename] = fs.readFileSync(0, 'utf8').trimEnd().split(/ {2}/);
  const actual = crypto.createHash('sha256').update(fs.readFileSync(filename)).digest('hex');
  if (actual !== expected) process.exit(1);
} else if (tool !== 'java') {
  process.exit(9);
}
`;
  for (const name of ['uname', 'curl', 'java', 'sha256sum']) {
    const file = path.join(tools, name);
    writeFileSync(file, mockTool);
    chmodSync(file, 0o755);
  }
  const envFile = path.join(runner, 'env');
  const pathFile = path.join(runner, 'path');
  const callsFile = path.join(root, 'calls');
  return {
    root, runner, manifest, config,
    run(overrides = {}) {
      writeFileSync(envFile, '');
      writeFileSync(pathFile, '');
      const result = spawnSync('bash', [script], {
        encoding: 'utf8',
        env: {
          ...process.env,
          PATH: `${tools}${path.delimiter}${process.env.PATH}`,
          FIXTURE_ARCHIVE: archive,
          FIXTURE_CALLS: callsFile,
          FAIL_DOWNLOAD: '',
          RUNNER_TEMP: runner,
          GITHUB_ENV: envFile,
          GITHUB_PATH: pathFile,
          OHOS_COMMAND_LINE_TOOLS_URL: downloadUrl,
          ...overrides,
        },
      });
      assert.doesNotMatch(result.stdout + result.stderr, /must-not-appear-in-logs/);
      assert.ok(!readdirSync(runner, { recursive: true }).some((file) => file.endsWith('.zip')), 'temporary archives must be removed');
      return {
        ...result,
        environment: readFileSync(envFile, 'utf8'),
        paths: readFileSync(pathFile, 'utf8'),
        downloads: existsSync(callsFile) ? readFileSync(callsFile, 'utf8').split('\n').filter(Boolean).length : 0,
      };
    },
  };
}

function assertFailure(result, message) {
  assert.notEqual(result.status, 0, result.stdout + result.stderr);
  assert.equal(result.environment, '', 'failed installation must not publish environment variables');
  assert.equal(result.paths, '', 'failed installation must not publish executable paths');
  assert.doesNotMatch(result.stdout, /is ready/);
  if (message) assert.match(result.stderr, message);
}

test('toolchain installation verifies a small archive and exports SDK, Node and packaging paths', (t) => {
  const setup = fixture(t);
  const result = setup.run();
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(result.downloads, 1);
  const installed = path.join(setup.runner, 'rssh-ohos-toolchain');
  const tools = path.join(installed, 'command-line-tools');
  const sdk = path.join(tools, 'sdk/default/openharmony');
  const node = path.join(tools, 'tool/node');
  const expected = {
    OHOS_HOME: sdk,
    OHOS_NDK_HOME: sdk,
    DEVECO_SDK_HOME: path.join(tools, 'sdk'),
    DEVECO_NODE_HOME: node,
    NODE_HOME: node,
    HVIGOR_BIN: path.join(tools, 'hvigor/bin/hvigorw.js'),
    OHPM_BIN: path.join(tools, 'bin/ohpm'),
  };
  assert.deepEqual(Object.fromEntries(result.environment.trim().split('\n').map((line) => line.split('='))), expected);
  assert.deepEqual(result.paths.trim().split('\n'), [path.join(node, 'bin'), path.join(tools, 'bin'), path.join(sdk, 'toolchains')]);
  assert.equal(readFileSync(path.join(installed, '.archive-sha256'), 'utf8').trim(), setup.config.sha256);
  assert.match(result.stdout, /is ready/);
});

test('a verified toolchain cache works without downloading or requiring the URL secret', (t) => {
  const setup = fixture(t);
  const first = setup.run();
  assert.equal(first.status, 0, first.stdout + first.stderr);
  const cached = setup.run({ OHOS_COMMAND_LINE_TOOLS_URL: '', FAIL_DOWNLOAD: '1' });
  assert.equal(cached.status, 0, cached.stdout + cached.stderr);
  assert.equal(cached.downloads, 1);
  assert.equal(cached.environment, first.environment);
  assert.equal(cached.paths, first.paths);
});

test('missing download credentials stop before contacting the CDN', (t) => {
  const result = fixture(t).run({ OHOS_COMMAND_LINE_TOOLS_URL: '' });
  assertFailure(result, /OHOS_COMMAND_LINE_TOOLS_URL/);
  assert.equal(result.downloads, 0);
});

test('a failed download cleans staging and cannot produce an installed toolchain', (t) => {
  const setup = fixture(t);
  const result = setup.run({ FAIL_DOWNLOAD: '1' });
  assertFailure(result);
  assert.equal(result.downloads, 1);
  assert.deepEqual(readdirSync(setup.runner).sort(), ['env', 'path']);
});

test('an archive checksum mismatch stops before extraction and clears staging', (t) => {
  const setup = fixture(t);
  writeFileSync(setup.manifest, JSON.stringify({ ...setup.config, sha256: '0'.repeat(64) }));
  const result = setup.run();
  assertFailure(result, /SHA-256 does not match/);
  assert.equal(result.downloads, 1);
  assert.deepEqual(readdirSync(setup.runner).sort(), ['env', 'path']);
});

test('an archive with the wrong SDK cannot publish build environment variables', (t) => {
  const setup = fixture(t);
  writeFileSync(setup.manifest, JSON.stringify({ ...setup.config, sdkVersion: 'wrong-sdk' }));
  const result = setup.run();
  assertFailure(result, /SDK version does not match/);
  assert.equal(result.downloads, 1);
});
