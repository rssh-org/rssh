import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import sharp from 'sharp';

const root = fileURLToPath(new URL('..', import.meta.url));
const icons = path.join(root, 'icons');
const source = path.join(icons, 'icon-source-hmos.png');

async function blueMark(file) {
  const { data, info } = await sharp(file).removeAlpha().raw().toBuffer({ resolveWithObject: true });
  let left = info.width, top = info.height, right = 0, bottom = 0, radius = 0;
  for (let y = 0; y < info.height; y++) {
    for (let x = 0; x < info.width; x++) {
      const offset = (y * info.width + x) * info.channels;
      const [r, g, b] = data.subarray(offset, offset + 3);
      // Isolate the blue terminal mark from its dark textured background.
      if (b < 144 || b - r < 64 || b - g < 32) continue;
      left = Math.min(left, x);
      top = Math.min(top, y);
      right = Math.max(right, x + 1);
      bottom = Math.max(bottom, y + 1);
      radius = Math.max(radius, Math.hypot(x + 0.5 - info.width / 2, y + 0.5 - info.height / 2));
    }
  }
  assert.ok(right > left && bottom > top, `missing blue mark: ${file}`);
  return { left, top, right, bottom, radius, width: info.width, height: info.height };
}

test('all source aliases and HarmonyOS use the canonical full-bleed artwork', async () => {
  const metadata = await sharp(source).metadata();
  assert.equal(metadata.width, 1024);
  assert.equal(metadata.height, 1024);
  assert.equal(metadata.hasAlpha, false);
  for (const relative of ['icons/icon-source.png', 'ohos/app/AppScope/resources/base/media/rssh_icon.png']) {
    assert.ok(readFileSync(path.join(root, relative)).equals(readFileSync(source)), relative);
  }
});

test('iOS icons have no alpha channel or baked-in inset tile', async () => {
  const directory = path.join(icons, 'ios');
  const names = readdirSync(directory).filter((name) => name.endsWith('.png'));
  assert.equal(names.length, 18);
  for (const name of names) {
    const file = path.join(directory, name);
    const metadata = await sharp(file).metadata();
    assert.equal(metadata.hasAlpha, false, name);
    const expected = await sharp(source).resize(metadata.width, metadata.height).removeAlpha().raw().toBuffer();
    assert.ok((await sharp(file).raw().toBuffer()).equals(expected), name);
  }
});

test('Android icons have correct sizes and opaque adaptive layers at every density', async () => {
  for (const [density, scale] of [['mdpi', 1], ['hdpi', 1.5], ['xhdpi', 2], ['xxhdpi', 3], ['xxxhdpi', 4]]) {
    const directory = path.join(icons, 'android', `mipmap-${density}`);
    for (const name of ['ic_launcher.png', 'ic_launcher_round.png']) {
      const metadata = await sharp(path.join(directory, name)).metadata();
      assert.equal(metadata.width, 48 * scale, `${density}/${name}`);
      assert.equal(metadata.height, 48 * scale, `${density}/${name}`);
    }
    const foreground = path.join(directory, 'ic_launcher_foreground.png');
    const metadata = await sharp(foreground).metadata();
    assert.equal(metadata.width, 108 * scale);
    assert.equal(metadata.height, 108 * scale);
    assert.equal(metadata.hasAlpha, false, density);
  }
});

test('Android keeps the master mark scale and position inside the launcher viewport', async () => {
  const master = await blueMark(source);
  for (const density of ['mdpi', 'hdpi', 'xhdpi', 'xxhdpi', 'xxxhdpi']) {
    const mark = await blueMark(path.join(icons, 'android', `mipmap-${density}`, 'ic_launcher_foreground.png'));
    // AdaptiveIconDrawable expands each layer by 25% on every side:
    // a 108dp layer therefore has a centered 72dp launcher viewport.
    const viewport = mark.width * 72 / 108;
    const inset = (mark.width - viewport) / 2;
    for (const [start, end, size] of [['left', 'right', 'width'], ['top', 'bottom', 'height']]) {
      const actualSize = (mark[end] - mark[start]) / viewport;
      const expectedSize = (master[end] - master[start]) / master[size];
      assert.ok(Math.abs(actualSize - expectedSize) < 0.02,
        `${density} ${size}: mark occupies ${(actualSize * 100).toFixed(1)}% of the viewport, expected ${(expectedSize * 100).toFixed(1)}%`);
      const actualCenter = ((mark[start] + mark[end]) / 2 - inset) / viewport;
      const expectedCenter = (master[start] + master[end]) / 2 / master[size];
      assert.ok(Math.abs(actualCenter - expectedCenter) < 0.02, `${density} ${size}: mark is off-center`);
    }
    assert.ok(mark.radius <= mark.width * 33 / 108, `${density}: mark leaves the 66dp safe circle`);
  }
});

test('the packaged macOS ICNS has an 824px body centered in its 1024px canvas', async () => {
  const icns = readFileSync(path.join(icons, 'icon.icns'));
  assert.equal(icns.toString('ascii', 0, 4), 'icns');
  let frame;
  for (let offset = 8; offset < icns.length;) {
    const length = icns.readUInt32BE(offset + 4);
    assert.ok(length > 8 && offset + length <= icns.length);
    if (icns.toString('ascii', offset, offset + 4) === 'ic10') frame = icns.subarray(offset + 8, offset + length);
    offset += length;
  }
  assert.ok(frame, 'ICNS must include its 1024px representation');
  const { data, info } = await sharp(frame).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  assert.equal(info.width, 1024);
  assert.equal(info.height, 1024);
  const bounds = [1024, 1024, 0, 0];
  for (let y = 0; y < 1024; y++) {
    for (let x = 0; x < 1024; x++) {
      if (data[(y * 1024 + x) * 4 + 3] < 128) continue;
      bounds[0] = Math.min(bounds[0], x);
      bounds[1] = Math.min(bounds[1], y);
      bounds[2] = Math.max(bounds[2], x + 1);
      bounds[3] = Math.max(bounds[3], y + 1);
    }
  }
  assert.deepEqual(bounds, [100, 100, 924, 924]);
  // The body is a scaled copy of the master, not the former tilted dollar sign.
  const expected = await sharp(source).resize(824).removeAlpha().raw().toBuffer();
  for (const [x, y] of [[300, 300], [450, 450], [600, 550]]) {
    const actualOffset = ((y + 100) * 1024 + x + 100) * 4;
    const expectedOffset = (y * 824 + x) * 3;
    assert.deepEqual(data.subarray(actualOffset, actualOffset + 3), expected.subarray(expectedOffset, expectedOffset + 3));
  }
});
