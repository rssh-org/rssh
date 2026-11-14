import { createHash, createPrivateKey, constants, sign } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { readFile, stat } from 'node:fs/promises';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { normalizeReleaseNotes } from './release-notes.mjs';

const UPLOAD_URL = 'https://connect-api.cloud.huawei.com/api/publish/v2/upload-url/for-obs';
const PACKAGE_INFO_URL = 'https://connect-api.cloud.huawei.com/api/publish/v3/app-package-info';
const VERSION_LIST_URL = 'https://connect-api.cloud.huawei.com/api/publish/v3/version/brief-info/list';
const COMPILE_STATUS_URL = 'https://connect-api.cloud.huawei.com/api/publish/v3/package/compile/status';
const SUBMIT_URL = 'https://connect-api.cloud.huawei.com/api/publish/v3/app-submit';
const APP_INFO_URL = 'https://connect-api.cloud.huawei.com/api/publish/v3/app-info';
const LANGUAGE_INFO_URL = 'https://connect-api.cloud.huawei.com/api/publish/v3/app-language-info';
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

function reviewFailureDetail(message, sensitiveValues) {
  if (typeof message !== 'string' || message.length > 16_384) return '';
  // Only ret.msg is diagnostic text. Never serialize the response or request.
  // Redact before truncation/whitespace normalization so secrets cannot be cut
  // into fragments that no longer match their original values.
  const secrets = sensitiveValues.filter((value) => typeof value === 'string' && value.length > 0)
    .flatMap((value) => [value, JSON.stringify(value).slice(1, -1), encodeURIComponent(value)])
    .sort((a, b) => b.length - a.length);
  for (const secret of secrets) message = message.replaceAll(secret, '[REDACTED]');
  message = message
    .replace(/-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----/g, '[REDACTED]')
    .replace(/https?:\/\/[^\s"'<>]+/gi, '[REDACTED URL]')
    .replace(/\bBearer\s+[^\s"',;]+/gi, 'Bearer [REDACTED]')
    .replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '')
    .replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, ' ')
    .replace(/\s+/g, ' ').trim();
  // Removing terminal escapes can join fragments back into a sensitive value.
  for (const secret of secrets) message = message.replaceAll(secret, '[REDACTED]');
  if (!message) return '';
  return ` AGC detail: ${message.slice(0, 1000)}${message.length > 1000 ? '…' : ''}`;
}

async function requestJson(fetchImpl, url, options, stage, diagnosticSecrets) {
  const response = await request(fetchImpl, url, options, stage, 30_000);
  requireValue(response.status === 200, `${stage} request returned HTTP ${response.status}.`);
  let result;
  try {
    result = await response.json();
  } catch {
    throw new UploadError(`${stage} response was not valid JSON.`);
  }
  const code = result?.ret?.code;
  // Most API responses can contain private upload data. Only review failures
  // opt in to bounded, redacted ret.msg diagnostics.
  let hint = code === 204144743
    ? ' AGC has no editable release version. For a published app, open its published version in AppGallery Connect and choose Upgrade to create a draft. If a version is pending review or release, resolve its status before retrying. See CONTRIBUTING.md.'
    : '';
  if (diagnosticSecrets && code === 204144692) {
    hint = ' The package was uploaded, registered, and parsed, but AGC rejected review submission. Check the release draft\'s required information, including What\'s new (new version features), in AppGallery Connect. Rebuilding alone does not fill missing release information.';
  }
  const detail = diagnosticSecrets && Number.isSafeInteger(code) && code !== 0
    ? reviewFailureDetail(result.ret.msg, diagnosticSecrets) : '';
  requireValue(code === 0, `${stage} request failed (code ${Number.isSafeInteger(code) ? code : 'missing or invalid'}).${detail}${hint}`);
  return result;
}

// Include every release type and state: an unpublished package can already have
// a higher versionCode than the current store release. Empty drafts have no code.
// https://developer.huawei.com/consumer/cn/doc/doccenter-connect-api/agc-help-publish-api-query-brief-info-list-0000002236201254
export async function nextVersionCode({ appId, serviceAccountJson }, { fetchImpl = fetch, now = Date.now } = {}) {
  requireValue(typeof appId === 'string' && /^[0-9]{1,32}$/.test(appId), 'AGC_APP_ID must be the numeric APP ID from AppGallery Connect.');
  const token = serviceAccountToken(serviceAccountJson, now);
  const result = await requestJson(fetchImpl, VERSION_LIST_URL, {
    method: 'POST', headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', appId }, body: '{}',
  }, 'AGC version lookup');
  requireValue(Array.isArray(result.versionList), 'AGC returned no valid version list.');
  let maximum = 0;
  for (const version of result.versionList) {
    requireValue(version && typeof version === 'object' && !Array.isArray(version), 'AGC returned an invalid version entry.');
    if (version.packageList == null) continue;
    requireValue(Array.isArray(version.packageList), 'AGC returned an invalid package list.');
    for (const pkg of version.packageList) {
      const code = pkg?.versionCode;
      requireValue(Number.isInteger(code) && code >= 1 && code <= 2147483647, 'AGC returned an invalid package versionCode.');
      maximum = Math.max(maximum, code);
    }
  }
  requireValue(maximum > 0, 'AGC returned no package versionCode to increment.');
  requireValue(maximum < 2147483647, 'AGC versionCode has reached the positive int32 limit.');
  return maximum + 1;
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

// Huawei requires waiting two minutes after upload before submitting review.
// Only the registered package's successful parsing state permits submission.
// https://developer.huawei.com/consumer/cn/doc/app/agc-help-publish-api-query-compile-status-0000002236041434
// https://developer.huawei.com/consumer/cn/doc/app/agc-help-publish-api-app-submit-0000002271160585
async function waitForPackage(appId, packageId, headers, fetchImpl, sleep) {
  requireValue(typeof packageId === 'string' && packageId.length > 0 && packageId.length <= 32, 'AGC returned no valid package ID; no review submitted.');
  const endpoint = new URL(COMPILE_STATUS_URL);
  endpoint.search = new URLSearchParams({ appId, pkgIds: packageId }).toString();
  await sleep(120_000);
  for (let attempt = 0; attempt < 17; attempt++) {
    const result = await requestJson(fetchImpl, endpoint, { headers }, 'AGC package status');
    requireValue(Array.isArray(result.pkgStateList), 'AGC returned no valid package status list; no review submitted.');
    const matches = result.pkgStateList.filter((pkg) => pkg?.pkgId === packageId);
    requireValue(matches.length === 1, 'AGC returned no unique package status for the uploaded package; no review submitted.');
    const { successStatus } = matches[0];
    requireValue([0, 1, 2].includes(successStatus), 'AGC returned an invalid package status; no review submitted.');
    requireValue(successStatus !== 2, 'AGC package parsing failed; no review submitted. Check its diagnostics in AppGallery Connect.');
    if (successStatus === 0) return;
    if (attempt < 16) await sleep(30_000);
  }
  throw new UploadError('AGC package is still parsing after polling; no review submitted. Check its status in AppGallery Connect.');
}

// Only update the existing default locale. Other localized descriptions and
// release notes remain owned by the developer console.
// https://developer.huawei.com/consumer/cn/doc/app/agc-help-publish-api-language-info-update-0000002271160569/
async function updateReleaseNotes(appId, newFeatures, headers, fetchImpl) {
  const appUrl = new URL(APP_INFO_URL);
  appUrl.search = new URLSearchParams({ appId, releaseType: '1' }).toString();
  const before = await requestJson(fetchImpl, appUrl, { headers }, 'AGC release information');
  const lang = before.appInfo?.defaultLang;
  requireValue(typeof lang === 'string' && /^[A-Za-z0-9_-]{1,8}$/.test(lang), 'AGC returned no valid default language; no review submitted.');
  const defaultLanguage = (result) => {
    requireValue(Array.isArray(result.languages), 'AGC returned no valid language information; no review submitted.');
    const matches = result.languages.filter((language) => language?.lang === lang);
    requireValue(matches.length === 1, 'AGC returned no unique default language entry; no review submitted.');
    return matches[0];
  };
  const original = defaultLanguage(before);
  const updateUrl = new URL(LANGUAGE_INFO_URL);
  updateUrl.search = new URLSearchParams({ appId, releaseType: '1', releasePhase: '0' }).toString();
  await requestJson(fetchImpl, updateUrl, {
    method: 'PUT', headers, body: JSON.stringify({ lang, newFeatures }),
  }, 'AGC release notes update');
  const after = await requestJson(fetchImpl, appUrl, { headers }, 'AGC release notes verification');
  const updated = defaultLanguage(after);
  requireValue(after.appInfo?.defaultLang === lang && updated.newFeatures === newFeatures,
    'AGC did not confirm the release notes update; no review submitted. Check the release draft in AppGallery Connect.');
  requireValue(['appName', 'appDesc', 'briefInfo'].every((field) => updated[field] === original[field]),
    'AGC release descriptions changed during the release notes update; no review submitted. Check the release draft in AppGallery Connect.');
}

// HarmonyOS uses a Service Account PS256 JWT, an OBS-signed binary PUT, then
// app-package-info to register the uploaded object in the developer console.
// Review submission is optional for direct callers and enabled by release CI.
// These are the HarmonyOS APIs, not Android's multipart upload/update-file API:
// https://developer.huawei.com/consumer/en/doc/app/agc-help-connect-api-obtain-server-auth-0000002271134661
// https://developer.huawei.com/consumer/en/doc/app/agc-help-upload-api-upload-url-0000002236201294
// https://developer.huawei.com/consumer/en/doc/app/agc-help-upload-api-upload-file-0000002271160621
// https://developer.huawei.com/consumer/cn/doc/app/agc-help-publish-api-app-package-info-update-0000002236201250
// https://developer.huawei.com/consumer/cn/doc/app/agc-help-publish-api-guide-0000002271134665
export async function uploadApp({ file, appId, serviceAccountJson, chineseMainlandFlag, submitReview = false, releaseNotes }, { fetchImpl = fetch, now = Date.now, sleep = delay } = {}) {
  requireValue(typeof appId === 'string' && /^[0-9]{1,32}$/.test(appId), 'AGC_APP_ID must be the numeric APP ID from AppGallery Connect.');
  requireValue(typeof submitReview === 'boolean', 'submitReview must be a boolean.');
  if (releaseNotes !== undefined) {
    requireValue(submitReview, 'Release notes can only be updated with --submit-review.');
    try {
      releaseNotes = normalizeReleaseNotes(releaseNotes);
    } catch {
      throw new UploadError('Release notes must contain the corresponding GitHub Release description.');
    }
  }
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
  const registration = await requestJson(fetchImpl, registrationUrl, {
    method: 'PUT', headers, body: JSON.stringify({ fileName, objectId: info.objectId }),
  }, 'AGC package registration');
  if (submitReview) {
    await waitForPackage(appId, registration.packageId, headers, fetchImpl, sleep);
    if (releaseNotes !== undefined) await updateReleaseNotes(appId, releaseNotes, headers, fetchImpl);
    const submitUrl = new URL(SUBMIT_URL);
    submitUrl.searchParams.set('appId', appId);
    // Do not retry a submission: a timeout may occur after AGC accepts it.
    await requestJson(fetchImpl, submitUrl, {
      method: 'POST', headers, body: JSON.stringify({ releaseType: 1, releasePhase: 0 }),
    }, 'AGC review submission', [
      token, serviceAccountJson, ...Object.values(JSON.parse(serviceAccountJson)),
      appId, registration.packageId, info.objectId, info.url.href, info.headers.get('authorization'),
      info.headers.get('x-obs-security-token'), info.headers.get('x-amz-security-token'),
    ]);
  }
  return { fileName, size: metadata.size, sha256, objectId: info.objectId };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const submitReview = process.argv[3] === '--submit-review';
    requireValue(process.argv.length === 3 || (process.argv.length === 4 && submitReview), 'Usage: node ohos/scripts/upload-agc.mjs path/to/RSSH.app [--submit-review]');
    let releaseNotes;
    if (process.env.AGC_RELEASE_NOTES_FILE) {
      try {
        releaseNotes = await readFile(process.env.AGC_RELEASE_NOTES_FILE, 'utf8');
      } catch {
        throw new UploadError('AGC_RELEASE_NOTES_FILE could not be read. Prepare the matching GitHub Release notes before uploading.');
      }
    }
    const result = await uploadApp({
      file: process.argv[2], appId: process.env.AGC_APP_ID,
      serviceAccountJson: process.env.AGC_SERVICE_ACCOUNT_JSON,
      chineseMainlandFlag: process.env.AGC_CHINESE_MAINLAND_FLAG || undefined,
      submitReview, releaseNotes,
    });
    const reviewStatus = submitReview ? 'Review submitted; awaiting Huawei approval.' : 'No review submitted.';
    console.log(`Uploaded and registered ${result.fileName} in AppGallery Connect (${result.size} bytes, SHA-256 ${result.sha256}). ${reviewStatus}`);
  } catch (error) {
    console.error(error instanceof UploadError ? error.message : 'AGC upload failed.');
    process.exitCode = 1;
  }
}
