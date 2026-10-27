import { createHash, createPrivateKey, constants, sign } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { stat } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const UPLOAD_URL = 'https://connect-api.cloud.huawei.com/api/publish/v2/upload-url/for-obs';
const PACKAGE_INFO_URL = 'https://connect-api.cloud.huawei.com/api/publish/v3/app-package-info';
const TOKEN_AUDIENCE = 'https://oauth-login.cloud.huawei.com/oauth2/v3/token';
const MAX_APP_BYTES = 4 * 1024 ** 3;

class UploadError extends Error {}

function requireValue(condition, message) {
  if (!condition) throw new UploadError(message);
}

function serviceAccountToken(json, now) {
  let account;
  try {
    account = JSON.parse(json);
  } catch {
    throw new UploadError('AGC_SERVICE_ACCOUNT_JSON must contain the downloaded Service Account JSON.');
  }
  requireValue(account && typeof account === 'object', 'Invalid AGC Service Account.');
  for (const name of ['key_id', 'sub_account', 'private_key']) {
    requireValue(typeof account[name] === 'string' && account[name].trim() !== '', `AGC Service Account is missing ${name}.`);
  }
  requireValue(!account.project_id, 'AGC upload requires a developer-level Service Account, not a project-level account.');
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString('base64url');
  const issued = Math.floor(now() / 1000);
  const header = encode({ alg: 'PS256', kid: account.key_id, typ: 'JWT' });
  const payload = encode({ aud: TOKEN_AUDIENCE, iss: account.sub_account, iat: issued, exp: issued + 3600 });
  const input = `${header}.${payload}`;
  try {
    const key = createPrivateKey(account.private_key);
    requireValue(['rsa', 'rsa-pss'].includes(key.asymmetricKeyType), 'AGC Service Account must contain an RSA private key.');
    const signature = sign('sha256', Buffer.from(input), {
      key, padding: constants.RSA_PKCS1_PSS_PADDING, saltLength: 32,
    });
    return `${input}.${signature.toString('base64url')}`;
  } catch {
    // Crypto errors can contain input details; never print credential contents.
    throw new UploadError('Could not sign the AGC Service Account JWT with its RSA private key.');
  }
}

async function request(fetchImpl, url, options, stage, timeout) {
  try {
    return await fetchImpl(url, { ...options, redirect: 'error', signal: AbortSignal.timeout(timeout) });
  } catch {
    // Fetch errors can include the signed upload URL or authorization headers.
    throw new UploadError(`${stage} request failed (network, redirect, or timeout).`);
  }
}

async function requestJson(fetchImpl, url, options, stage) {
  const response = await request(fetchImpl, url, options, stage, 30_000);
  requireValue(response.status === 200, `${stage} request returned HTTP ${response.status}.`);
  let result;
  try {
    result = await response.json();
  } catch {
    throw new UploadError(`${stage} response was not valid JSON.`);
  }
  const code = result?.ret?.code;
  requireValue(code === 0, `${stage} request failed (code ${Number.isSafeInteger(code) ? code : 'missing or invalid'}).`);
  return result;
}

function uploadInfo(value, size) {
  requireValue(value && typeof value === 'object', 'AGC returned no upload information.');
  requireValue(value.method === 'PUT', 'AGC returned an unexpected upload method.');
  requireValue(typeof value.objectId === 'string' && value.objectId !== '', 'AGC returned no upload object ID.');
  let url;
  let headers;
  try {
    url = new URL(value.url);
    headers = new Headers(value.headers ?? {});
  } catch {
    throw new UploadError('AGC returned invalid upload URL or headers.');
  }
  requireValue(url.protocol === 'https:' && !url.username && !url.password && !url.hash, 'AGC returned an unsafe upload URL.');
  // Preserve Huawei's signed headers, adding binary-body defaults when absent.
  if (!headers.has('content-length')) headers.set('content-length', String(size));
  if (!headers.has('content-type')) headers.set('content-type', 'application/octet-stream');
  return { url, headers, objectId: value.objectId };
}

// HarmonyOS uses a Service Account PS256 JWT, an OBS-signed binary PUT, then
// app-package-info to register the uploaded object in the developer console.
// Registration is separate from review submission; never call app-submit here.
// These are the HarmonyOS APIs, not Android's multipart upload/update-file API:
// https://developer.huawei.com/consumer/en/doc/app/agc-help-connect-api-obtain-server-auth-0000002271134661
// https://developer.huawei.com/consumer/en/doc/app/agc-help-upload-api-upload-url-0000002236201294
// https://developer.huawei.com/consumer/en/doc/app/agc-help-upload-api-upload-file-0000002271160621
// https://developer.huawei.com/consumer/cn/doc/app/agc-help-publish-api-app-package-info-update-0000002236201250
// https://developer.huawei.com/consumer/cn/doc/app/agc-help-publish-api-guide-0000002271134665
export async function uploadApp({ file, appId, serviceAccountJson, chineseMainlandFlag }, { fetchImpl = fetch, now = Date.now } = {}) {
  requireValue(typeof appId === 'string' && /^[0-9]{1,32}$/.test(appId), 'AGC_APP_ID must be the numeric APP ID from AppGallery Connect.');
  requireValue(typeof file === 'string' && path.extname(file) === '.app', 'Provide the signed HarmonyOS .app package.');
  requireValue(chineseMainlandFlag === undefined || /^[01]$/.test(String(chineseMainlandFlag)), 'AGC_CHINESE_MAINLAND_FLAG must be 0 or 1 when configured.');
  let metadata;
  try {
    metadata = await stat(file);
  } catch {
    throw new UploadError('The HarmonyOS .app package could not be read.');
  }
  requireValue(metadata.isFile() && metadata.size > 0 && metadata.size <= MAX_APP_BYTES, 'The HarmonyOS .app package must be a nonempty file no larger than 4 GiB.');
  const fileName = path.basename(file);
  requireValue(fileName.length <= 255, 'The HarmonyOS .app filename must not exceed 255 characters.');
  const token = serviceAccountToken(serviceAccountJson, now);
  const hash = createHash('sha256');
  try {
    for await (const chunk of createReadStream(file)) hash.update(chunk);
  } catch {
    throw new UploadError('The HarmonyOS .app package could not be hashed.');
  }
  const sha256 = hash.digest('hex');
  const endpoint = new URL(UPLOAD_URL);
  endpoint.search = new URLSearchParams({ appId, fileName, sha256, contentLength: String(metadata.size), releaseType: '1' }).toString();
  if (chineseMainlandFlag !== undefined) endpoint.searchParams.set('chineseMainlandFlag', String(chineseMainlandFlag));
  const headers = { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' };
  const result = await requestJson(fetchImpl, endpoint, { headers }, 'AGC upload URL');
  const info = uploadInfo(result.urlInfo, metadata.size);
  const stream = createReadStream(file);
  try {
    const uploaded = await request(fetchImpl, info.url, {
      method: 'PUT', headers: info.headers, body: stream, duplex: 'half',
    }, 'AGC package upload', 240_000);
    requireValue(uploaded.status === 200, `AGC package upload returned HTTP ${uploaded.status}.`);
    // Success is HTTP 200 per the upload API. There is no JSON ret on this PUT.
    await uploaded.body?.cancel();
  } finally {
    stream.destroy();
  }
  // OBS only stores the bytes. Register the package before reporting success.
  const registrationUrl = new URL(PACKAGE_INFO_URL);
  registrationUrl.searchParams.set('appId', appId);
  await requestJson(fetchImpl, registrationUrl, {
    method: 'PUT', headers, body: JSON.stringify({ fileName, objectId: info.objectId }),
  }, 'AGC package registration');
  return { fileName, size: metadata.size, sha256, objectId: info.objectId };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    requireValue(process.argv.length === 3, 'Usage: node ohos/scripts/upload-agc.mjs path/to/RSSH.app');
    const result = await uploadApp({
      file: process.argv[2], appId: process.env.AGC_APP_ID,
      serviceAccountJson: process.env.AGC_SERVICE_ACCOUNT_JSON,
      chineseMainlandFlag: process.env.AGC_CHINESE_MAINLAND_FLAG || undefined,
    });
    console.log(`Uploaded and registered ${result.fileName} in AppGallery Connect (${result.size} bytes, SHA-256 ${result.sha256}). No review submitted.`);
  } catch (error) {
    console.error(error instanceof UploadError ? error.message : 'AGC upload failed.');
    process.exitCode = 1;
  }
}
