import { NativeApplicationAbility } from '@ohos-rs/ability';
import { rsshPlugins } from './RsshPlugins';

/** Desktop UIAbility instances are peer windows sharing one native application runtime. */
export class RsshApplicationAbility extends NativeApplicationAbility {
  public moduleName: string = 'rssh_lib';
  public peerAbilityName: string = 'WindowAbility';
  public bridgePlugins = rsshPlugins();

  protected override createInitContext(moduleName: string) {
    const context = super.createInitContext(moduleName);
    const filesDir = this.context.getApplicationContext().filesDir;
    context.basePath = filesDir;
    context.prefPath = filesDir;
    return context;
  }
}
