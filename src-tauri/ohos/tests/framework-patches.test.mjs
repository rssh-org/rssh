import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const sourceRoot = fileURLToPath(new URL('../../target/ohos-sources/', import.meta.url));
const patchRoot = fileURLToPath(new URL('../patches/', import.meta.url));

test('checked-in framework patches include every modified or added generated source', () => {
  for (const name of ['ability', 'tao', 'wry', 'tauri']) {
    const repository = path.join(sourceRoot, name);
    const patch = path.join(patchRoot, `${name}.patch`);
    const included = new Set(Array.from(readFileSync(patch, 'utf8').matchAll(/^diff --git a\/(.+) b\/(.+)$/gm), (match) => match[2]));
    const changed = execFileSync('git', ['-C', repository, 'ls-files', '-m', '-o', '--exclude-standard'], { encoding: 'utf8' }).trim().split('\n');
    for (const file of changed) {
      if (!file || file === '.rssh-source.json') continue;
      assert.ok(included.has(file), `${name}/${file} is missing from its patch; git diff omits new files unless they are added with intent-to-add`);
    }
    execFileSync('git', ['-C', repository, 'apply', '--reverse', '--check', patch]);
  }
});

test('replayed Tao and Wry entry points have all declared platform modules', () => {
  for (const entry of ['tao/src/platform_impl/ohos/mod.rs', 'wry/src/ohos/mod.rs']) {
    const file = path.join(sourceRoot, entry);
    for (const [, module] of readFileSync(file, 'utf8').matchAll(/^mod ([a-z0-9_]+);$/gm)) {
      assert.ok(
        existsSync(path.join(path.dirname(file), `${module}.rs`)) || existsSync(path.join(path.dirname(file), module, 'mod.rs')),
        `${entry} declares missing module ${module}; verify the patch also creates its source file`,
      );
    }
  }
});
