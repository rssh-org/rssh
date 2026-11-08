import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import sharp from 'sharp';
import { runTauri } from './tauri.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
const icons = path.join(root, 'icons');
const source = path.join(icons, 'icon-source-hmos.png');
const densities = [['mdpi', 1], ['hdpi', 1.5], ['xhdpi', 2], ['xxhdpi', 3], ['xxxhdpi', 4]];

function generateTauriIcons(input, output) {
  const result = runTauri(root, ['icon', input, '--output', output], { stdio: 'pipe' });
  if (result.status !== 0) throw new Error(result.stderr || result.stdout || 'Tauri icon generation failed');
  // Tauri emits identical ICNS representations in random map order. Preserve
  // the encoded chunks, but order their type tags to avoid meaningless diffs.
  const file = path.join(output, 'icon.icns');
  const icns = readFileSync(file);
  const chunks = [];
  for (let offset = 8; offset < icns.length;) {
    const length = icns.readUInt32BE(offset + 4);
    if (length <= 8 || offset + length > icns.length) throw new Error('Invalid generated ICNS chunk');
    chunks.push(icns.subarray(offset, offset + length));
    offset += length;
  }
  chunks.sort((a, b) => a.subarray(0, 4).compare(b.subarray(0, 4)));
  writeFileSync(file, Buffer.concat([icns.subarray(0, 8), ...chunks]));
}

async function desktopArtwork() {
  // Apple's traditional macOS canvas has a centered 824px body in 1024px.
  // The grayscale mask is exported from Icon Composer, not a hand-drawn radius.
  const alpha = await sharp(path.join(icons, 'macos-mask.png'))
    .resize(824).toColourspace('b-w').raw().toBuffer();
  const artwork = await sharp(source).resize(824).removeAlpha().png().toBuffer();
  const body = await sharp(artwork)
    .joinChannel(alpha, { raw: { width: 824, height: 824, channels: 1 } }).png().toBuffer();
  return sharp(body).extend({
    top: 100, bottom: 100, left: 100, right: 100,
    background: { r: 0, g: 0, b: 0, alpha: 0 },
  }).png().toBuffer();
}

async function generateIcons() {
  const metadata = await sharp(source).metadata();
  if (metadata.width !== 1024 || metadata.height !== 1024 || metadata.hasAlpha) {
    throw new Error('icons/icon-source-hmos.png must be a full-bleed 1024×1024 RGB PNG without alpha');
  }
  const temporary = mkdtempSync(path.join(tmpdir(), 'rssh-icons-'));
  try {
    const desktop = await desktopArtwork();
    const desktopSource = path.join(temporary, 'desktop.png');
    await sharp(desktop).toFile(desktopSource);
    const desktopOutput = path.join(temporary, 'desktop');
    const mobileOutput = path.join(temporary, 'mobile');
    generateTauriIcons(desktopSource, desktopOutput);
    generateTauriIcons(source, mobileOutput);

    // Keep Tauri's platform filenames and ICNS/ICO encoders. Only root files
    // belong to desktop; its generated mobile subdirectories are not installed.
    for (const entry of readdirSync(desktopOutput, { withFileTypes: true })) {
      if (entry.isFile()) copyFileSync(path.join(desktopOutput, entry.name), path.join(icons, entry.name));
    }
    await sharp(desktop).resize(64).png().toFile(path.join(icons, '64x64.png'));

    mkdirSync(path.join(icons, 'ios'), { recursive: true });
    for (const name of readdirSync(path.join(mobileOutput, 'ios'))) {
      if (!name.endsWith('.png')) continue;
      const { width } = await sharp(path.join(mobileOutput, 'ios', name)).metadata();
      // Encode RGB explicitly: even an all-255 alpha channel is invalid for
      // the default App Store icon. Do not reuse the desktop mask or margins.
      await sharp(source).resize(width).removeAlpha().png().toFile(path.join(icons, 'ios', name));
    }

    const circle = Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024"><circle cx="512" cy="512" r="512" fill="white"/></svg>');
    const round = await sharp(source).ensureAlpha().composite([{ input: circle, blend: 'dest-in' }]).png().toBuffer();
    for (const [density, scale] of densities) {
      const directory = path.join(icons, 'android', `mipmap-${density}`);
      mkdirSync(directory, { recursive: true });
      await sharp(desktop).resize(48 * scale).png().toFile(path.join(directory, 'ic_launcher.png'));
      await sharp(round).resize(48 * scale).png().toFile(path.join(directory, 'ic_launcher_round.png'));
      // This master is flattened artwork. Keep it full-bleed in the 108dp
      // adaptive foreground; its centered mark fits inside the 66dp safe zone.
      // Android applies the mask. Never reuse the pre-masked legacy PNG here.
      await sharp(source).resize(108 * scale).removeAlpha().png()
        .toFile(path.join(directory, 'ic_launcher_foreground.png'));
    }
    copyFileSync(source, path.join(icons, 'icon-source.png'));
    copyFileSync(source, path.join(root, 'ohos/app/AppScope/resources/base/media/rssh_icon.png'));
    console.log('Generated desktop, iOS, Android and HarmonyOS icons from icon-source-hmos.png');
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

generateIcons().catch((error) => {
  console.error(error.message);
  process.exitCode = 1;
});
