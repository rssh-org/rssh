import { NativeAbility } from '@ohos-rs/ability';
import { window } from '@kit.ArkUI';
import { rsshPlugins } from './RsshPlugins';

export class RsshAbility extends NativeAbility {
  public moduleName: string = 'rssh_lib';
  public bridgePlugins = rsshPlugins();

  protected override async loadWindowStageContent(windowStage: window.WindowStage): Promise<void> {
    await windowStage.loadContent('pages/Index');
  }

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
