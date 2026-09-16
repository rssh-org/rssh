import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { prepareWorkspace } from './prepare-ohos.mjs';

test('OHOS workspace derives shared dependencies without changing the normal workspace', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh ohos '));
  try {
    const source = path.join(root, 'src-tauri');
    for (const dir of ['src', 'capabilities', 'icons', 'bin', 'ohos']) {
      mkdirSync(path.join(source, dir), { recursive: true });
    }
    const manifest = `[package]\nname = "rssh"\nversion = "0.0.1"\n[lib]\nname = "rssh_lib"\n[build-dependencies]\ntauri-build = { version = "2", features = [] }\n[dependencies]\ntauri = { version = "2", features = ["protocol-asset"] }\nnew-shared-dependency = "1"\n`;
    writeFileSync(path.join(source, 'Cargo.toml'), manifest);
    writeFileSync(path.join(source, 'Cargo.lock'), 'normal lock');
    writeFileSync(path.join(source, 'build.rs'), 'fn main() {}');
    writeFileSync(path.join(source, 'src/lib.rs'), '// shared source');
    writeFileSync(path.join(source, 'ohos/overlay.toml'), '[workspace]\n[patch.crates-io]\nwry = { path = "@OHOS_SOURCES@/wry" }\n');
    writeFileSync(path.join(source, 'ohos/Cargo.lock'), 'ohos lock');
    writeFileSync(path.join(source, 'tauri.conf.json'), JSON.stringify({
      build: { frontendDist: '../dist' }, bundle: { resources: { 'bin/*': 'bin/' } },
    }));

    const workspace = path.join(source, 'target/ohos-workspace');
    prepareWorkspace(root, workspace);
    const generated = readFileSync(path.join(workspace, 'Cargo.toml'), 'utf8');
    assert.match(generated, /tauri-build = \{ version = "=2\.6\.3"/);
    assert.match(generated, /tauri = \{ version = "=2\.11\.5"/);
    assert.match(generated, /new-shared-dependency = "1"/);
    assert.match(generated, /\[workspace\]/);
    assert.ok(generated.includes(`wry = { path = ${JSON.stringify(path.join(source, 'target/ohos-sources/wry'))} }`));
    assert.doesNotMatch(generated, /@OHOS_SOURCES@/);
    assert.equal(readFileSync(path.join(workspace, 'src/lib.rs'), 'utf8'), '// shared source');
    assert.equal(readFileSync(path.join(workspace, 'Cargo.lock'), 'utf8'), 'ohos lock');
    assert.equal(readFileSync(path.join(source, 'Cargo.toml'), 'utf8'), manifest);
    assert.equal(readFileSync(path.join(source, 'Cargo.lock'), 'utf8'), 'normal lock');
    assert.equal(JSON.parse(readFileSync(path.join(workspace, 'tauri.conf.json'))).build.frontendDist, path.join(source, 'target/ohos-frontend'));
    assert.equal(JSON.parse(readFileSync(path.join(source, 'tauri.conf.json'))).build.frontendDist, '../dist');
    prepareWorkspace(root, workspace);
    assert.equal(readFileSync(path.join(workspace, 'Cargo.toml'), 'utf8'), generated);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
