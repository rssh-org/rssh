import { LazyPlugin, BridgePluginFactory } from '@ohos-rs/ability';
import { AppControlPlugin } from '@ohos-rs/ability-plugin-app-control';
import { FilesPlugin } from '@ohos-rs/ability-plugin-files';
import { PermissionPlugin } from '@ohos-rs/ability-plugin-permission';
import { UrlPlugin } from '@ohos-rs/ability-plugin-url';
import { WebviewPlugin } from '@ohos-rs/ability-plugin-webview';
import { WindowPlugin } from '@ohos-rs/ability-plugin-window';
import { ClipboardPlugin } from '../plugins/ClipboardPlugin';
import { FilesAccessPlugin } from '../plugins/FilesAccessPlugin';
import { RuntimePlugin } from '../plugins/RuntimePlugin';
import { SerialPlugin } from '../plugins/SerialPlugin';

export function rsshPlugins(): BridgePluginFactory[] {
  return [
    new LazyPlugin(() => new AppControlPlugin()),
    new LazyPlugin(() => new FilesPlugin()),
    new LazyPlugin(() => new PermissionPlugin()),
    new LazyPlugin(() => new UrlPlugin()),
    new LazyPlugin(() => new WebviewPlugin()),
    new LazyPlugin(() => new WindowPlugin()),
    new LazyPlugin(() => new ClipboardPlugin()),
    new LazyPlugin(() => new FilesAccessPlugin()),
    new LazyPlugin(() => new RuntimePlugin()),
    new LazyPlugin(() => new SerialPlugin()),
  ];
}
