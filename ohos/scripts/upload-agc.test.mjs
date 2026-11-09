import assert from 'node:assert/strict';
import { constants, createHash, generateKeyPairSync, verify } from 'node:crypto';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';
import { nextVersionCode, uploadApp } from './upload-agc.mjs';

const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
const account = {
  project_id: '', key_id: 'test-key-id', sub_account: 'test-developer',
  private_key: privateKey.export({ type: 'pkcs8', format: 'pem' }),
};
const serviceAccountJson = JSON.stringify(account);
const appId = '123456789';
const packageId = '157300000005952';
const now = () => 1_800_000_000_000;

async function fixture(t, { urlReply, uploadStatus = 200, apiStatus = 200, registrationReply, registrationStatus = 200, registrationBody, versionReply, versionStatus = 200, versionBody, compileReplies = [{ ret: { code: 0 }, pkgStateList: [{ pkgId: packageId, successStatus: 0 }] }], compileStatus = 200, compileBody, submitReply, submitStatus = 200, submitBody } = {}) {
  const dir = mkdtempSync(path.join(tmpdir(), 'rssh agc upload '));
  const file = path.join(dir, 'RSSH release.app');
  // Binary fixture exercises byte preservation (NUL and non-UTF8 included).
  const bytes = Buffer.from([0x50, 0x4b, 0x03, 0x04, 0, 255, 128, 13, 10, 0x22]);
  writeFileSync(file, bytes);
  const requests = [];
  const waits = [];
  let compileCalls = 0;
  const server = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    requests.push({ method: req.method, url: req.url, headers: req.headers, body: Buffer.concat(chunks) });
    if (req.url === '/api/publish/v3/version/brief-info/list') {
      res.writeHead(versionStatus, { 'content-type': 'application/json' });
      res.end(versionBody ?? JSON.stringify(versionReply ?? {
        ret: { code: 0 }, versionList: [{ packageList: [{ versionCode: 41 }] }],
      }));
      return;
    }
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
      res.end(registrationBody ?? JSON.stringify(registrationReply ?? { ret: { code: 0 }, packageId }));
      return;
    }
    if (req.url.startsWith('/api/publish/v3/package/compile/status?')) {
      res.writeHead(compileStatus, { 'content-type': 'application/json' });
      res.end(compileBody ?? JSON.stringify(compileReplies[Math.min(compileCalls++, compileReplies.length - 1)]));
      return;
    }
    if (req.url.startsWith('/api/publish/v3/app-submit?')) {
      res.writeHead(submitStatus, { 'content-type': 'application/json' });
      res.end(submitBody ?? JSON.stringify(submitReply ?? { ret: { code: 0 } }));
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
  const sleep = async (ms) => waits.push({ ms, requestCount: requests.length });
  return { file, bytes, requests, options, fetchImpl, sleep, waits };
}

test('next version code authenticates and queries every release type and state before incrementing the largest package code', async (t) => {
  const f = await fixture(t, { versionReply: { ret: { code: 0 }, versionList: [
    { versionName: 'newer draft' },
    { versionName: 'commercial', packageList: [{ versionCode: 12 }, { versionCode: 5000 }] },
    { versionName: 'empty draft', packageList: null },
    { versionName: 'beta', packageList: [{ versionCode: 700 }] },
    { versionName: 'unuploaded', packageList: [] },
  ] } });
  assert.equal(await nextVersionCode(f.options, { fetchImpl: f.fetchImpl, now }), 5001);
  assert.equal(f.requests.length, 1, 'Version lookup must not upload, register, submit review, or publish.');
  const [request] = f.requests;
  assert.equal(request.method, 'POST');
  assert.equal(request.url, '/api/publish/v3/version/brief-info/list');
  assert.equal(request.headers.appid, appId);
  assert.equal(request.headers['content-type'], 'application/json');
  assert.equal(request.headers.client_id, undefined);
  assert.deepEqual(JSON.parse(request.body), {}, 'Do not filter out test releases or unpublished versions.');
  const token = request.headers.authorization.replace(/^Bearer /, '');
  const [header, payload, signature] = token.split('.');
  assert.deepEqual(JSON.parse(Buffer.from(header, 'base64url')), { alg: 'PS256', kid: account.key_id, typ: 'JWT' });
  assert.equal(verify('sha256', Buffer.from(`${header}.${payload}`), {
    key: publicKey, padding: constants.RSA_PKCS1_PSS_PADDING, saltLength: 32,
  }, Buffer.from(signature, 'base64url')), true);
});

test('the last available positive int32 version code can be allocated', async (t) => {
  const f = await fixture(t, { versionReply: { ret: { code: 0 }, versionList: [{ packageList: [{ versionCode: 2147483646 }] }] } });
  assert.equal(await nextVersionCode(f.options, { fetchImpl: f.fetchImpl, now }), 2147483647);
});

for (const [name, versionList] of [
  ['no version list', undefined],
  ['a null version list', null],
  ['an object instead of a version list', {}],
  ['no versions', []],
  ['only empty drafts', [{}, { packageList: null }, { packageList: [] }]],
  ['a null version', [null]],
  ['a malformed package list', [{ packageList: {} }]],
  ['a null package', [{ packageList: [null] }]],
  ['a package without its version code', [{ packageList: [{ versionCode: 41 }, {}] }]],
]) {
  test(`next version code refuses to guess from ${name}`, async (t) => {
    const f = await fixture(t, { versionReply: { ret: { code: 0 }, versionList } });
    await assert.rejects(nextVersionCode(f.options, { fetchImpl: f.fetchImpl, now }), /version|package/i);
    assert.equal(f.requests.length, 1);
  });
}

for (const code of [null, '42', 'SECRET', 0, -1, 1.5, 2147483648]) {
  test(`next version code rejects invalid package code ${JSON.stringify(code)}`, async (t) => {
    const f = await fixture(t, { versionReply: { ret: { code: 0 }, versionList: [{ packageList: [{ versionCode: 41 }, { versionCode: code }] }] } });
    await assert.rejects(nextVersionCode(f.options, { fetchImpl: f.fetchImpl, now }), (error) => {
      assert.match(error.message, /versionCode/);
      assert.doesNotMatch(error.message, /SECRET/);
      return true;
    });
  });
}

test('an exhausted version code range cannot wrap or return an invalid next code', async (t) => {
  const f = await fixture(t, { versionReply: { ret: { code: 0 }, versionList: [{ packageList: [{ versionCode: 2147483647 }] }] } });
  await assert.rejects(nextVersionCode(f.options, { fetchImpl: f.fetchImpl, now }), /versionCode/);
});

for (const [name, config, expected] of [
  ['HTTP failure', { versionStatus: 403 }, /HTTP 403/],
  ['business failure', { versionReply: { ret: { code: 204144666, msg: 'SECRET do-not-log' } } }, /code 204144666/],
  ['missing business status', { versionReply: { versionList: [] } }, /missing or invalid/],
  ['a string business status', { versionReply: { ret: { code: '0' } } }, /missing or invalid/],
  ['invalid JSON', { versionBody: 'SECRET do-not-log' }, /response was not valid JSON/],
]) {
  test(`version lookup ${name} fails without exposing the response`, async (t) => {
    const f = await fixture(t, config);
    await assert.rejects(nextVersionCode(f.options, { fetchImpl: f.fetchImpl, now }), (error) => {
      assert.match(error.message, expected);
      assert.doesNotMatch(error.message, /SECRET|do-not-log/);
      return true;
    });
    assert.equal(f.requests.length, 1);
  });
}

test('version lookup transport errors never expose the JWT', async () => {
  const fetchImpl = async (url, options) => {
    throw new Error(`${url} ${options.headers.Authorization} SECRET`);
  };
  await assert.rejects(nextVersionCode({ appId, serviceAccountJson }, { fetchImpl, now }), {
    message: 'AGC version lookup request failed (network, redirect, or timeout).',
  });
});

test('version lookup validates app and credentials before any network request', async () => {
  const fetchImpl = async () => assert.fail('Invalid input must not use the network');
  for (const change of [
    { appId: '' }, { appId: 'SECRET' }, { serviceAccountJson: 'SECRET invalid JSON' },
    { serviceAccountJson: JSON.stringify({ ...account, project_id: 'project-scope' }) },
  ]) {
    await assert.rejects(nextVersionCode({ appId, serviceAccountJson, ...change }, { fetchImpl, now }), (error) => {
      assert.doesNotMatch(error.message, /SECRET/);
      return true;
    });
  }
});

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
  });
});

test('the mainland flag can be omitted for a mainland-registered developer', async (t) => {
  const f = await fixture(t);
  await uploadApp(f.options, { fetchImpl: f.fetchImpl, now });
  assert.equal(new URL(f.requests[0].url, 'https://example.test').searchParams.has('chineseMainlandFlag'), false);
});

test('automatic review waits for this package to finish parsing and submits the full release once', async (t) => {
  const f = await fixture(t, { compileReplies: [
    { ret: { code: 0 }, pkgStateList: [{ pkgId: packageId, successStatus: 1 }] },
    { ret: { code: 0 }, pkgStateList: [{ pkgId: packageId, successStatus: 0 }] },
  ] });
  await uploadApp({ ...f.options, submitReview: true }, { fetchImpl: f.fetchImpl, now, sleep: f.sleep });
  assert.equal(f.requests.length, 6);
  assert.deepEqual(f.waits, [{ ms: 120_000, requestCount: 3 }, { ms: 30_000, requestCount: 4 }]);
  for (const status of f.requests.slice(3, 5)) {
    assert.equal(status.method, 'GET');
    assert.equal(status.url, `/api/publish/v3/package/compile/status?appId=${appId}&pkgIds=${packageId}`);
    assert.equal(status.headers.authorization, f.requests[0].headers.authorization);
  }
  const submit = f.requests[5];
  assert.equal(submit.method, 'POST');
  assert.equal(submit.url, `/api/publish/v3/app-submit?appId=${appId}`);
  assert.equal(submit.headers.authorization, f.requests[0].headers.authorization);
  assert.equal(submit.headers.client_id, undefined);
  assert.equal(submit.headers['content-type'], 'application/json');
  assert.deepEqual(JSON.parse(submit.body), { releaseType: 1, releasePhase: 0 });
});

for (const code of [204144743, 204144666]) {
  test(`automatic review stops immediately if registration fails with ${code}`, async (t) => {
    const f = await fixture(t, { registrationReply: { ret: { code } } });
    await assert.rejects(uploadApp({ ...f.options, submitReview: true }, { fetchImpl: f.fetchImpl, now, sleep: f.sleep }), /package registration request failed/);
    assert.equal(f.requests.length, 3);
    assert.deepEqual(f.waits, []);
  });
}

test('automatic review requires the package ID returned by registration', async (t) => {
  const f = await fixture(t, { registrationReply: { ret: { code: 0 } } });
  await assert.rejects(uploadApp({ ...f.options, submitReview: true }, { fetchImpl: f.fetchImpl, now, sleep: f.sleep }), /package ID/);
  assert.equal(f.requests.length, 3);
  assert.deepEqual(f.waits, []);
});

for (const [name, reply, expected] of [
  ['failed parsing', { pkgStateList: [{ pkgId: packageId, successStatus: 2, errorMsg: 'SECRET' }] }, /package parsing failed/],
  ['unknown status', { pkgStateList: [{ pkgId: packageId, successStatus: 99 }] }, /invalid package status/],
  ['string status', { pkgStateList: [{ pkgId: packageId, successStatus: '0' }] }, /invalid package status/],
  ['missing status list', {}, /package status/],
  ['empty status list', { pkgStateList: [] }, /package status/],
  ['another package', { pkgStateList: [{ pkgId: '999', successStatus: 0 }] }, /package status/],
  ['duplicate package', { pkgStateList: [{ pkgId: packageId, successStatus: 0 }, { pkgId: packageId, successStatus: 1 }] }, /package status/],
]) {
  test(`automatic review refuses ${name}`, async (t) => {
    const f = await fixture(t, { compileReplies: [{ ret: { code: 0 }, ...reply }] });
    await assert.rejects(uploadApp({ ...f.options, submitReview: true }, { fetchImpl: f.fetchImpl, now, sleep: f.sleep }), (error) => {
      assert.match(error.message, expected);
      assert.doesNotMatch(error.message, /SECRET/);
      return true;
    });
    assert.equal(f.requests.length, 4);
  });
}

test('automatic review stops polling a package that never finishes parsing', async (t) => {
  const f = await fixture(t, { compileReplies: [{ ret: { code: 0 }, pkgStateList: [{ pkgId: packageId, successStatus: 1 }] }] });
  await assert.rejects(uploadApp({ ...f.options, submitReview: true }, { fetchImpl: f.fetchImpl, now, sleep: f.sleep }), /still parsing.*no review submitted/i);
  assert.equal(f.requests.length, 20, 'Three upload requests followed by at most 17 status checks.');
  assert.equal(f.waits.reduce((sum, wait) => sum + wait.ms, 0), 600_000);
  assert.ok(f.requests.slice(3).every((request) => request.method === 'GET'));
});

for (const [name, config, expected, requestCount] of [
  ['status HTTP error', { compileStatus: 403 }, /HTTP 403/, 4],
  ['status invalid JSON', { compileBody: 'SECRET' }, /not valid JSON/, 4],
  ['status business error', { compileReplies: [{ ret: { code: 204144718, msg: 'SECRET' } }] }, /code 204144718/, 4],
  ['submission HTTP error', { submitStatus: 403 }, /HTTP 403/, 5],
  ['submission invalid JSON', { submitBody: 'SECRET' }, /not valid JSON/, 5],
  ['submission business error', { submitReply: { ret: { code: 204144664, msg: 'SECRET' } } }, /code 204144664/, 5],
  ['submission missing status', { submitReply: {} }, /missing or invalid/, 5],
]) {
  test(`automatic review fails on ${name} without retrying or exposing the response`, async (t) => {
    const f = await fixture(t, config);
    await assert.rejects(uploadApp({ ...f.options, submitReview: true }, { fetchImpl: f.fetchImpl, now, sleep: f.sleep }), (error) => {
      assert.match(error.message, expected);
      assert.doesNotMatch(error.message, /SECRET/);
      return true;
    });
    assert.equal(f.requests.length, requestCount);
  });
}

test('automatic review never retries an ambiguous submission transport failure or exposes credentials', async (t) => {
  const f = await fixture(t);
  let submissions = 0;
  const fetchImpl = (url, options) => {
    if (new URL(url).pathname === '/api/publish/v3/app-submit') {
      submissions++;
      throw new Error(`${options.headers.Authorization} SECRET`);
    }
    return f.fetchImpl(url, options);
  };
  await assert.rejects(uploadApp({ ...f.options, submitReview: true }, { fetchImpl, now, sleep: f.sleep }), {
    message: 'AGC review submission request failed (network, redirect, or timeout).',
  });
  assert.equal(submissions, 1);
  assert.equal(f.requests.length, 4);
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

test('registration without an editable version explains how to create a draft and remains a failure', async (t) => {
  const f = await fixture(t, { registrationReply: { ret: { code: 204144743, msg: 'SECRET do-not-log' } } });
  await assert.rejects(uploadApp(f.options, { fetchImpl: f.fetchImpl, now }), (error) => {
    assert.match(error.message, /package registration request failed \(code 204144743\)/);
    assert.match(error.message, /no editable release version/i);
    assert.match(error.message, /published version.*Upgrade.*draft/);
    assert.match(error.message, /pending review or release/);
    assert.match(error.message, /CONTRIBUTING\.md/);
    assert.doesNotMatch(error.message, /SECRET|do-not-log/);
    return true;
  });
  assert.equal(f.requests.length, 3, 'Do not retry registration or change the review/release state.');
  assert.deepEqual(f.requests[1].body, f.bytes);
});

for (const [name, config, expected] of [
  ['HTTP failure', { registrationStatus: 403 }, /package registration request returned HTTP 403/],
  ['business failure', { registrationReply: { ret: { code: 204144666, msg: 'SECRET do-not-log' } } }, /package registration request failed \(code 204144666\)/],
  ['missing business status', { registrationReply: { packageId: '123' } }, /missing or invalid/],
  ['a string business status', { registrationReply: { ret: { code: '0' } } }, /missing or invalid/],
  ['invalid JSON', { registrationBody: 'SECRET do-not-log' }, /package registration response was not valid JSON/],
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
    { submitReview: 'false' },
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

test('the CLI keeps upload-only behavior unless --submit-review is passed', async (t) => {
  const f = await fixture(t);
  const preload = path.join(path.dirname(f.file), 'fetch-fixture.mjs');
  writeFileSync(preload, `
    const replies = [
      { ret: { code: 0 }, urlInfo: { method: 'PUT', url: 'https://upload.example.test/package', objectId: 'test-object' } },
      null,
      { ret: { code: 0 } },
    ];
    globalThis.fetch = async (url, options) => {
      if (options.body && options.duplex) for await (const chunk of options.body) {}
      if (!replies.length) throw new Error('Unexpected request');
      return new Response(JSON.stringify(replies.shift()), { status: 200 });
    };
  `);
  const options = {
    encoding: 'utf8', timeout: 5000,
    env: { ...process.env, AGC_APP_ID: appId, AGC_SERVICE_ACCOUNT_JSON: serviceAccountJson, AGC_CHINESE_MAINLAND_FLAG: '' },
  };
  const args = ['--import', preload, 'ohos/scripts/upload-agc.mjs', f.file];
  const uploadOnly = spawnSync(process.execPath, args, options);
  assert.equal(uploadOnly.status, 0, uploadOnly.stderr);
  assert.match(uploadOnly.stdout, /No review submitted/);
  const submit = spawnSync(process.execPath, [...args, '--submit-review'], options);
  assert.equal(submit.status, 1, submit.stderr);
  assert.match(submit.stderr, /package ID/, 'The review flag must reach the registration result check.');
  assert.doesNotMatch(submit.stdout, /Review submitted/);
  const invalid = spawnSync(process.execPath, [...args, '--submit-reveiw'], options);
  assert.equal(invalid.status, 1);
  assert.match(invalid.stderr, /Usage:/);
});
