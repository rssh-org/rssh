import { copyFileSync, existsSync, readFileSync, rmSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { stageProject } from '../../scripts/stage-project.mjs';

const projectEntries = new Set([
  'app', 'buildSrc', 'gradle', '.editorconfig', '.gitignore',
  'build.gradle.kts', 'gradle.properties', 'gradlew', 'gradlew.bat', 'settings.gradle',
]);
const generatedDirectories = new Set([
  'build', '.gradle', '.kotlin', '.idea', '.tauri', '.cxx', '.externalNativeBuild', 'generated', 'jniLibs',
]);
const generatedFiles = new Set([
  'tauri.settings.gradle', 'app/tauri.build.gradle.kts', 'app/proguard-tauri.pro',
  'app/tauri.properties', 'app/src/main/assets/tauri.conf.json',
]);
// Keep the existing launcher resource set: the optional adaptive-icon XML was
// never part of this Android app and would change its launcher appearance.
const launcherResources = new Set([
  ...['hdpi', 'mdpi', 'xhdpi', 'xxhdpi', 'xxxhdpi'].flatMap((density) =>
    ['ic_launcher.png', 'ic_launcher_foreground.png', 'ic_launcher_round.png']
      .map((name) => `mipmap-${density}/${name}`)),
  'values/ic_launcher_background.xml',
]);

function excluded(relative) {
  const parts = relative.split('/');
  return !projectEntries.has(parts[0])
    || parts.some((part) => generatedDirectories.has(part))
    || parts.some((part) => part === 'local.properties' || part === 'key.properties')
    || /\.(so|jks|keystore|p12|pfx)$/.test(relative)
    || generatedFiles.has(relative)
    || relative === 'app/src/main/assets/bin';
}

export function prepareAndroid(root) {
  const source = path.join(root, 'android');
  const destination = path.join(root, 'app/gen/android');
  stageProject(source, destination, { exclude: excluded });
  stageProject(path.join(root, 'icons/android'), path.join(destination, 'app/src/main/res'), {
    exclude: (relative, entry) => entry.isDirectory()
      ? ![...launcherResources].some((name) => name.startsWith(`${relative}/`))
      : !launcherResources.has(relative),
  });
  // Older desktop resource configuration also populated mobile assets/bin.
  // Tauri does not remove resources that disappear from bundle.resources.
  rmSync(path.join(destination, 'app/src/main/assets/bin'), { recursive: true, force: true });

  // SDK paths are developer state. Signing material stays in android/ and is
  // read directly by Gradle, never copied into the disposable native project.
  const sdkConfig = path.join(source, 'local.properties');
  const generatedSdkConfig = path.join(destination, 'local.properties');
  if (existsSync(sdkConfig) && (!existsSync(generatedSdkConfig) || !readFileSync(sdkConfig).equals(readFileSync(generatedSdkConfig)))) {
    copyFileSync(sdkConfig, generatedSdkConfig);
  }
  return destination;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  prepareAndroid(path.resolve(fileURLToPath(new URL('../..', import.meta.url))));
}
