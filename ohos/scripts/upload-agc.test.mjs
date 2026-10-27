import assert from 'node:assert/strict';
import { constants, createHash, generateKeyPairSync, verify } from 'node:crypto';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';
import { uploadApp } from './upload-agc.mjs';

const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
const account = {
  project_id: '', key_id: 'test-key-id', sub_account: 'test-developer',
  private_key: privateKey.export({ type: 'pkcs8', format: 'pem' }),
};
const serviceAccountJson = JSON.stringify(account);
const appId = '123456789';
const now = () => 1_800_000_000_000;

async function fixture(t, { urlReply, uploadStatus = 200, apiStatus = 200, registrationReply, registrationStatus = 200, registrationBody } = {}) {
  const dir = mkdtempSync(path.join(tmpdir(), 'rssh agc upload '));
  const file = path.join(dir, 'RSSH release.app');
  // Binary fixture exercises byte preservation (NUL and non-UTF8 included).
  const bytes = Buffer.from([0x50, 0x4b, 0x03, 0x04, 0, 255, 128, 13, 10, 0x22]);
  writeFileSync(file, bytes);
  const requests = [];
  const server = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    requests.push({ method: req.method, url: req.url, headers: req.headers, body: Buffer.concat(chunks) });
    if (req.url.startsWith('/api/publish/v2/upload-url/for-obs?')) {
      res.writeHead(apiStatus, { 'content-type': 'application/json' });
      res.end(JSON.stringify(urlReply ?? {
        ret: { code: 0 },
        urlInfo: {
          objectId: 'CN/test/RSSH.app', url: 'https://upload.example.test/package?signature=do-not-log', method: 'PUT',
          headers: {
            Authorization: 'OBS scoped signature', Host: 'upload.example.test',
            'x-amz-date': '20261027T000000Z', 'x-amz-content-sha256': createHash('sha256').update(bytes).digest('hex'),
            'user-agent': 'agc-upload-test', 'Content-Type': 'application/octet-stream',
          },
        },
      }));
      return;
    }
    if (req.url.startsWith('/package?')) {
      res.writeHead(uploadStatus);
      res.end(uploadStatus === 200 ? '' : 'private diagnostic do-not-log');
      return;
    }
    if (req.url.startsWith('/api/publish/v3/app-package-info?')) {
      res.writeHead(registrationStatus, { 'content-type': 'application/json' });
      res.end(registrationBody ?? JSON.stringify(registrationReply ?? { ret: { code: 0 }, packageId: '157300000005952' }));
      return;
    }
    res.writeHead(500);
    res.end('Unexpected release/review endpoint');
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    rmSync(dir, { recursive: true, force: true });
  });
  const fetchImpl = (input, options) => {
    const url = new URL(input);
    assert.ok(['connect-api.cloud.huawei.com', 'upload.example.test'].includes(url.hostname));
    assert.equal(url.protocol, 'https:');
    assert.equal(options.redirect, 'error');
    assert.ok(options.signal instanceof AbortSignal);
    if (url.hostname === 'upload.example.test') assert.equal(options.headers.get('host'), 'upload.example.test');
    // Exercise real HTTP streams without allowing configurable production hosts.
    url.protocol = 'http:';
    url.host = `127.0.0.1:${server.address().port}`;
    return fetch(url, options);
  };
  const options = { file, appId, serviceAccountJson };
  return { file, bytes, requests, options, fetchImpl };
}

// Huawei's Publishing API guide requires registering the uploaded OBS object:
// https://developer.huawei.com/consumer/cn/doc/app/agc-help-publish-api-app-package-info-update-0000002236201250
test('HarmonyOS upload authenticates, PUTs exact bytes, and registers the package without submitting review', async (t) => {
  const f = await fixture(t);
  const result = await uploadApp({ ...f.options, chineseMainlandFlag: '1' }, { fetchImpl: f.fetchImpl, now });
  assert.equal(f.requests.length, 3, 'Only upload URL, OBS PUT and HarmonyOS package registration are called; no token exchange, Android update-file or review/publish request.');
  const [api, upload, registration] = f.requests;
  assert.equal(api.method, 'GET');
  const query = new URL(api.url, 'https://connect-api.cloud.huawei.com').searchParams;
  assert.deepEqual(Object.fromEntries(query), {
    appId, fileName: 'RSSH release.app', sha256: createHash('sha256').update(f.bytes).digest('hex'),
    contentLength: String(f.bytes.length), releaseType: '1', chineseMainlandFlag: '1',
  });
  assert.equal(api.headers.client_id, undefined, 'Service Accounts do not send API client credentials.');
  const token = api.headers.authorization.replace(/^Bearer /, '');
  const [header, payload, signature] = token.split('.');
  assert.deepEqual(JSON.parse(Buffer.from(header, 'base64url')), { alg: 'PS256', kid: account.key_id, typ: 'JWT' });
  assert.deepEqual(JSON.parse(Buffer.from(payload, 'base64url')), {
    aud: 'https://oauth-login.cloud.huawei.com/oauth2/v3/token', iss: account.sub_account,
    iat: now() / 1000, exp: now() / 1000 + 3600,
  });
  assert.equal(verify('sha256', Buffer.from(`${header}.${payload}`), {
    key: publicKey, padding: constants.RSA_PKCS1_PSS_PADDING, saltLength: 32,
  }, Buffer.from(signature, 'base64url')), true);
  assert.equal(upload.method, 'PUT');
  assert.deepEqual(upload.body, f.bytes);
  assert.equal(upload.headers['content-type'], 'application/octet-stream', 'HarmonyOS upload is raw binary, not Android multipart/form-data.');
  assert.equal(upload.headers['content-length'], String(f.bytes.length));
  assert.equal(upload.headers.authorization, 'OBS scoped signature', 'Never forward the AGC JWT to the file server.');
  assert.equal(upload.headers['x-amz-date'], '20261027T000000Z');
  assert.equal(upload.headers['x-amz-content-sha256'], query.get('sha256'));
  assert.equal(upload.headers['user-agent'], 'agc-upload-test');
  assert.equal(registration.method, 'PUT');
  assert.equal(registration.url, `/api/publish/v3/app-package-info?appId=${appId}`);
  assert.equal(registration.headers.authorization, api.headers.authorization, 'Package registration uses the AGC JWT, not the OBS signature.');
  assert.equal(registration.headers.client_id, undefined);
  assert.equal(registration.headers['content-type'], 'application/json');
  assert.deepEqual(JSON.parse(registration.body), { fileName: 'RSSH release.app', objectId: 'CN/test/RSSH.app' });
  assert.deepEqual(result, {
    fileName: 'RSSH release.app', size: f.bytes.length, sha256: query.get('sha256'), objectId: 'CN/test/RSSH.app',
    packageId: '157300000005952',
  });
});

test('the mainland flag can be omitted for a mainland-registered developer', async (t) => {
  const f = await fixture(t);
  await uploadApp(f.options, { fetchImpl: f.fetchImpl, now });
  assert.equal(new URL(f.requests[0].url, 'https://example.test').searchParams.has('chineseMainlandFlag'), false);
});

for (const [name, config, expected] of [
  ['HTTP failure', { apiStatus: 403 }, /HTTP 403/],
  ['business failure', { urlReply: { ret: { code: 204144666, msg: 'SECRET do-not-log' } } }, /code 204144666/],
  ['missing business status', { urlReply: { urlInfo: {} } }, /missing or invalid/],
  ['a string business status', { urlReply: { ret: { code: '0' } } }, /missing or invalid/],
]) {
  test(`upload URL ${name} stops before sending package bytes`, async (t) => {
    const f = await fixture(t, config);
    await assert.rejects(uploadApp(f.options, { fetchImpl: f.fetchImpl, now }), (error) => {
      assert.match(error.message, expected);
      assert.doesNotMatch(error.message, /SECRET|do-not-log/);
      return true;
    });
    assert.equal(f.requests.length, 1);
  });
}

test('a failed binary PUT is not reported as a successful upload', async (t) => {
  const f = await fixture(t, { uploadStatus: 403 });
  await assert.rejects(uploadApp(f.options, { fetchImpl: f.fetchImpl, now }), /package upload returned HTTP 403/);
  assert.equal(f.requests.length, 2);
});

for (const [name, config, expected] of [
  ['HTTP failure', { registrationStatus: 403 }, /package registration request returned HTTP 403/],
  ['business failure', { registrationReply: { ret: { code: 204144666, msg: 'SECRET do-not-log' } } }, /package registration request failed \(code 204144666\)/],
  ['missing business status', { registrationReply: { packageId: '123' } }, /missing or invalid/],
  ['a string business status', { registrationReply: { ret: { code: '0' } } }, /missing or invalid/],
  ['invalid JSON', { registrationBody: 'SECRET do-not-log' }, /package registration response was not valid JSON/],
  ['invalid package ID', { registrationReply: { ret: { code: 0 }, packageId: 123 } }, /invalid package ID/],
]) {
  test(`package registration ${name} fails the upload after OBS accepts the bytes`, async (t) => {
    const f = await fixture(t, config);
    await assert.rejects(uploadApp(f.options, { fetchImpl: f.fetchImpl, now }), (error) => {
      assert.match(error.message, expected);
      assert.doesNotMatch(error.message, /SECRET|do-not-log/);
      return true;
    });
    assert.equal(f.requests.length, 3, 'A failed registration never proceeds to review or publish.');
    assert.deepEqual(f.requests[1].body, f.bytes);
  });
}

test('a successful registration may omit the optional packageId', async (t) => {
  const f = await fixture(t, { registrationReply: { ret: { code: 0 } } });
  const result = await uploadApp(f.options, { fetchImpl: f.fetchImpl, now });
  assert.equal(f.requests.length, 3);
  assert.equal(result.packageId, undefined);
});

test('package registration transport failures are redacted and do not report success', async (t) => {
  const f = await fixture(t);
  const fetchImpl = async (url, options) => {
    if (new URL(url).pathname === '/api/publish/v3/app-package-info') {
      throw new Error(`${url} ${options.headers.Authorization} SECRET`);
    }
    return f.fetchImpl(url, options);
  };
  await assert.rejects(uploadApp(f.options, { fetchImpl, now }), {
    message: 'AGC package registration request failed (network, redirect, or timeout).',
  });
  assert.equal(f.requests.length, 2);
});

for (const [name, info] of [
  ['HTTP upload URL', { url: 'http://upload.example.test/package' }],
  ['credentials in URL', { url: 'https://secret@upload.example.test/package' }],
  ['unexpected method', { method: 'POST' }],
  ['wrong signed Host', { headers: { Host: 'other.example.test' } }],
  ['wrong content length', { headers: { 'Content-Length': '999' } }],
  ['unexpected content type', { headers: { 'Content-Type': 'multipart/form-data' } }],
]) {
  test(`rejects ${name} before uploading`, async (t) => {
    const f = await fixture(t, { urlReply: { ret: { code: 0 }, urlInfo: {
      objectId: 'test-object', method: 'PUT', url: 'https://upload.example.test/package', headers: {}, ...info,
    } } });
    await assert.rejects(uploadApp(f.options, { fetchImpl: f.fetchImpl, now }));
    assert.equal(f.requests.length, 1);
  });
}

test('transport errors never expose credentials or signed URLs', async (t) => {
  const f = await fixture(t);
  const fetchImpl = async (url, options) => {
    throw new Error(`${url} ${options.headers.Authorization} SECRET`);
  };
  await assert.rejects(uploadApp(f.options, { fetchImpl, now }), (error) => {
    assert.equal(error.message, 'AGC upload URL request failed (network, redirect, or timeout).');
    return true;
  });
});

test('invalid credentials and packages fail before any network access', async (t) => {
  const f = await fixture(t);
  const fetchImpl = async () => assert.fail('Invalid input must not use the network');
  for (const change of [
    { appId: '' }, { file: f.file.replace(/app$/, 'hap') }, { chineseMainlandFlag: '2' },
    { serviceAccountJson: '{}' }, { serviceAccountJson: 'SECRET invalid JSON' },
    { serviceAccountJson: JSON.stringify({ ...account, project_id: 'project-scope' }) },
    { serviceAccountJson: JSON.stringify({ ...account, private_key: 'SECRET invalid private key' }) },
  ]) {
    await assert.rejects(uploadApp({ ...f.options, ...change }, { fetchImpl, now }), (error) => {
      assert.doesNotMatch(error.message, /SECRET/);
      return true;
    });
  }
  writeFileSync(f.file, '');
  await assert.rejects(uploadApp(f.options, { fetchImpl, now }), /nonempty/);
});

test('the CLI fails clearly when required configuration is absent, without printing it', () => {
  const run = spawnSync(process.execPath, ['ohos/scripts/upload-agc.mjs', 'RSSH.app'], {
    encoding: 'utf8', env: { ...process.env, AGC_APP_ID: '', AGC_SERVICE_ACCOUNT_JSON: 'SECRET' },
  });
  assert.equal(run.status, 1);
  assert.match(run.stderr, /AGC_APP_ID/);
  assert.doesNotMatch(run.stderr, /SECRET/);
});
