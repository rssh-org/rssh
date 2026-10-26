import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { prepareAndroid } from './prepare.mjs';
import { runTauri } from '../../scripts/tauri.mjs';

function fixture() {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh android '));
  const write = (relative, content) => {
    const file = path.join(root, relative);
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, content);
  };
  write('android/app/src/main/java/Main.kt', 'source');
  write('android/app/build.gradle.kts', '// application');
  write('android/build.gradle.kts', '// project');
  write('android/settings.gradle', '// settings');
  write('android/gradlew', '#!/bin/sh\n');
  write('android/gradle/wrapper/gradle-wrapper.jar', 'wrapper');
  write('icons/android/mipmap-hdpi/ic_launcher.png', 'launcher');
  write('icons/android/values/ic_launcher_background.xml', 'background');
  return { root, write };
}

test('Android prepares native inputs without copying caches, credentials or generated files', () => {
  const { root, write } = fixture();
  try {
    const excluded = [
      '.gradle/cache', 'app/build/outputs/old.apk', 'buildSrc/build/old.jar',
      'app/src/main/jniLibs/arm64-v8a/old.so', 'app/src/main/java/generated/Old.kt',
      'app/src/main/assets/tauri.conf.json', 'app/src/main/assets/bin/rssh-cli',
      'tauri.settings.gradle', 'app/tauri.build.gradle.kts', 'app/tauri.properties',
      'key.properties', 'release.jks', 'scripts/helper.mjs',
    ];
    for (const file of excluded) write(`android/${file}`, 'excluded');
    write('android/local.properties', 'sdk.dir=local-sdk');
    write('android/app/src/main/res/values/colors.xml', 'app colors');
    write('icons/android/mipmap-anydpi-v26/ic_launcher.xml', 'unused adaptive icon');
    write('src-tauri/gen/android/app/build/outputs/current.apk', 'keep current output');
    write('src-tauri/gen/android/app/src/main/assets/bin/rssh-cli', 'old desktop CLI');
    prepareAndroid(root);
    const generated = path.join(root, 'src-tauri/gen/android');
    assert.equal(readFileSync(path.join(generated, 'app/src/main/java/Main.kt'), 'utf8'), 'source');
    assert.equal(readFileSync(path.join(generated, 'gradle/wrapper/gradle-wrapper.jar'), 'utf8'), 'wrapper');
    assert.equal(readFileSync(path.join(generated, 'local.properties'), 'utf8'), 'sdk.dir=local-sdk');
    assert.equal(readFileSync(path.join(generated, 'app/build/outputs/current.apk'), 'utf8'), 'keep current output');
    assert.equal(existsSync(path.join(generated, 'app/src/main/assets/bin')), false);
    assert.equal(readFileSync(path.join(generated, 'app/src/main/res/mipmap-hdpi/ic_launcher.png'), 'utf8'), 'launcher');
    assert.equal(readFileSync(path.join(generated, 'app/src/main/res/values/colors.xml'), 'utf8'), 'app colors');
    assert.equal(existsSync(path.join(generated, 'app/src/main/res/mipmap-anydpi-v26/ic_launcher.xml')), false);
    for (const file of excluded) assert.equal(existsSync(path.join(generated, file)), false, file);

    rmSync(path.join(root, 'android/app/src/main/java/Main.kt'));
    write('icons/android/mipmap-hdpi/ic_launcher.png', 'updated launcher');
    prepareAndroid(root);
    assert.equal(existsSync(path.join(generated, 'app/src/main/java/Main.kt')), false);
    assert.equal(readFileSync(path.join(generated, 'app/src/main/res/mipmap-hdpi/ic_launcher.png'), 'utf8'), 'updated launcher');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('the CLI wrapper prepares Android before all native entry points and forwards arguments', () => {
  for (const nativeArgs of [['dev'], ['build'], ['build', '--open'], ['android-studio-script']]) {
    const { root, write } = fixture();
    try {
      write('node_modules/@tauri-apps/cli/tauri.js', `
        const fs = require('node:fs');
        if (!fs.existsSync('src-tauri/gen/android/app/src/main/java/Main.kt')) process.exit(9);
        console.log(JSON.stringify(process.argv.slice(2)));
      `);
      const args = ['android', ...nativeArgs, '--target', 'aarch64', '--config', '{"build":{"devUrl":"http://localhost:1420"}}'];
      const result = runTauri(root, args, { stdio: 'pipe' });
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(JSON.parse(result.stdout), args);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }
});

test('help and non-Android commands do not prepare the native project', () => {
  const { root, write } = fixture();
  try {
    write('node_modules/@tauri-apps/cli/tauri.js', 'process.exit(0)');
    for (const args of [['--help'], ['android', '--help'], ['android', 'build', '--help'], ['dev'], ['build', '--help']]) {
      assert.equal(runTauri(root, args, { stdio: 'pipe' }).status, 0);
      assert.equal(existsSync(path.join(root, 'src-tauri/gen/android')), false);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
