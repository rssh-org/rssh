import { LazyPlugin, NativeAbility } from '@ohos-rs/ability';
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

export default class EntryAbility extends NativeAbility {
  public moduleName: string = 'rssh_lib';
  public bridgePlugins = [
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

  protected override createInitContext(moduleName: string) {
    const context = super.createInitContext(moduleName);
    // Keep existing databases and keys in ApplicationContext's /base/files.
    // https://github.com/openharmony/docs/blob/master/en/application-dev/application-models/application-context-stage.md#obtaining-application-file-paths
    const filesDir = this.context.getApplicationContext().filesDir;
    context.basePath = filesDir;
    context.prefPath = filesDir;
    return context;
  }
}
