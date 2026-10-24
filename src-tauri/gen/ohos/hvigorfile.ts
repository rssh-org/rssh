import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { hvigor } from '@ohos/hvigor';
import { appTasks, OhosAppContext, OhosPluginId } from '@ohos/hvigor-ohos-plugin';

// Apply signing before the HAP modules initialize their tasks. The public
// build-profile.json5 stays unchanged; Hvigor updates only its in-memory model.
hvigor.getRootNode().afterNodeEvaluate((node) => {
    const context = node.getContext(OhosPluginId.OHOS_APP_PLUGIN) as OhosAppContext;
    const profile = context.getBuildProfileOpt();
    if (profile.app.signingConfigs?.length) {
        throw new Error('Move signingConfigs from build-profile.json5 to signing.local.json.');
    }

    const enableSignTask = hvigor.getParameter().getExtParam('enableSignTask');
    if (enableSignTask === 'false') return;
    const localPath = join(node.getNodePath(), 'signing.local.json');
    if (!existsSync(localPath)) {
        if (enableSignTask === 'true') {
            throw new Error('Signing requires signing.local.json; copy signing.example.json and configure your signing materials.');
        }
        return;
    }

    let local;
    try {
        local = JSON.parse(readFileSync(localPath, 'utf8'));
    } catch {
        // JSON parser errors can contain fragments of passwords.
        throw new Error('Cannot read signing.local.json as JSON. Check its syntax and file permissions.');
    }
    if (!Array.isArray(local?.signingConfigs) || local.signingConfigs.length === 0) {
        throw new Error('signing.local.json must contain a non-empty signingConfigs array.');
    }
    profile.app.signingConfigs = local.signingConfigs;
    context.setBuildProfileOpt(profile);
});

export default {
    system: appTasks,  /* Built-in plugin of Hvigor. It cannot be modified. */
    plugins:[]         /* Custom plugin to extend the functionality of Hvigor. */
}
