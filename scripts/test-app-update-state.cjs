const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');
const loaded = { exports: {} };
new Function('module', 'exports', buildSync({ entryPoints: ['apps/frontend/src/appUpdateState.ts'], bundle: true, platform: 'node', format: 'cjs', write: false }).outputFiles[0].text)(loaded, loaded.exports);
const { UpdateCheckGate, UPDATE_CHECK_INTERVAL, shouldOfferUpdate, mergeUpdateProgress, downloadPercent, performUpdateInstallation } = loaded.exports;
const update = { version: '2.0.0', currentVersion: '1.0.0' };

(async () => {
  const checks = new UpdateCheckGate();
  let calls = 0, complete;
  const remote = () => { calls++; return new Promise(resolve => { complete = resolve; }); };
  // StrictMode remount and a manual click share the same pending request.
  const first = checks.check(remote, false, 1000);
  const remount = checks.check(remote, false, 1001);
  const manual = checks.check(remote, true, 1002);
  await Promise.resolve();
  assert.equal(calls, 1);
  assert.equal(first, remount); assert.equal(first, manual);
  complete(update);
  const result = await first;
  assert.deepEqual(result, { update, checkedAt: 1000 });
  assert.equal(await checks.check(remote, false, 1000 + UPDATE_CHECK_INTERVAL - 1), result);
  assert.equal(calls, 1);
  assert.equal(checks.isDue(1000 + UPDATE_CHECK_INTERVAL), true);
  assert.equal(checks.isDue(999), true); // Wall-clock rollback must not postpone checks forever.
  const newer = await checks.check(async () => { calls++; return null; }, true, 2000);
  assert.equal(newer.update, null); assert.equal(calls, 2);

  const failing = new UpdateCheckGate();
  let attempts = 0;
  const unavailable = async () => { attempts++; throw new Error('offline'); };
  await assert.rejects(failing.check(unavailable, false, 0), /offline/);
  assert.equal(await failing.check(unavailable, false, 500), undefined);
  assert.equal(attempts, 1); // Automatic failures respect the six-hour backoff.
  await assert.rejects(failing.check(unavailable, true, 600), /offline/);
  assert.equal(attempts, 2); // A manual retry is never locked out by the backoff.
  const recovered = await failing.check(async () => update, false, 600 + UPDATE_CHECK_INTERVAL);
  assert.equal(recovered.update, update);

  // A failed replacement check invalidates the native offer, so a remount or
  // visibility refresh must not resurrect the previous download button.
  const invalidated = new UpdateCheckGate();
  assert.equal((await invalidated.check(async () => update, false, 100)).update, update);
  await assert.rejects(invalidated.check(async () => { throw new Error('replacement check failed'); }, true, 200), /replacement check failed/);
  assert.equal(await invalidated.check(async () => assert.fail('automatic backoff'), false, 201), undefined);
  assert.equal((await invalidated.check(async () => update, true, 202)).update, update);

  // An authoritative no-update result supersedes a previous successful offer,
  // including the cached result consumed by a later automatic caller.
  const withdrawn = new UpdateCheckGate();
  await withdrawn.check(async () => update, false, 300);
  const noUpdate = await withdrawn.check(async () => null, false, 300 + UPDATE_CHECK_INTERVAL);
  assert.equal(noUpdate.update, null);
  assert.equal(shouldOfferUpdate(noUpdate.update, null, false), false);
  assert.equal(await withdrawn.check(async () => assert.fail('should use the new null cache'), false, 301 + UPDATE_CHECK_INTERVAL), noUpdate);

  assert.equal(shouldOfferUpdate(update, '2.0.0', false), false);
  assert.equal(shouldOfferUpdate(update, '2.0.0', true), true);
  assert.equal(shouldOfferUpdate({ ...update, version: '2.0.1' }, '2.0.0', false), true);
  assert.equal(shouldOfferUpdate(null, null, true), false);
  assert.equal(shouldOfferUpdate({ ...update, currentVersion: '2.0.0' }, null, true), false);

  let progress = mergeUpdateProgress(null, { phase: 'downloading', downloaded: 0 });
  assert.equal(downloadPercent(progress), undefined);
  progress = mergeUpdateProgress(progress, { phase: 'downloading', downloaded: 25, total: 100 });
  assert.equal(downloadPercent(progress), 25);
  progress = mergeUpdateProgress(progress, { phase: 'downloading', downloaded: 10 });
  assert.equal(progress.downloaded, 25); assert.equal(progress.total, 100);
  for (const event of [null, {}, { phase: 'complete', downloaded: 100 }, { phase: 'downloading', downloaded: -1 }, { phase: 'downloading', downloaded: NaN }, { phase: 'downloading', downloaded: Infinity }]) {
    assert.equal(mergeUpdateProgress(progress, event), progress);
  }
  assert.equal(downloadPercent(mergeUpdateProgress(null, { phase: 'downloading', downloaded: 12, total: 0 })), undefined);
  assert.equal(downloadPercent(mergeUpdateProgress(progress, { phase: 'downloading', downloaded: 101, total: 100 })), undefined);
  progress = mergeUpdateProgress(progress, { phase: 'installing', downloaded: 0 });
  assert.equal(progress.phase, 'installing'); assert.equal(progress.downloaded, 25);
  assert.equal(mergeUpdateProgress(progress, { phase: 'downloading', downloaded: 100, total: 100 }), progress);
  progress = mergeUpdateProgress(progress, { phase: 'restarting', downloaded: 100, total: 100 });
  assert.equal(progress.phase, 'restarting'); assert.equal(downloadPercent(progress), 100);
  assert.equal(mergeUpdateProgress(progress, { phase: 'installing', downloaded: 100 }), progress);
  // Retry starts from a new attempt, rather than preserving the previous download's count.
  assert.deepEqual(mergeUpdateProgress(null, { phase: 'downloading', downloaded: 1 }), { phase: 'downloading', downloaded: 1 });

  const sequence = [];
  let finishDownload, latestDraft = 'before download', savedDraft;
  const installing = performUpdateInstallation({
    download: () => { sequence.push('download'); return new Promise(resolve => { finishDownload = resolve; }); },
    beforeInstall: async () => { sequence.push('flush'); savedDraft = latestDraft; },
    install: async () => { sequence.push('install'); },
    active: () => true,
  });
  latestDraft = 'edited during download';
  assert.deepEqual(sequence, ['download']);
  finishDownload();
  assert.equal(await installing, true);
  assert.equal(savedDraft, 'edited during download');
  assert.deepEqual(sequence, ['download', 'flush', 'install']);
  const stopped = [];
  await assert.rejects(performUpdateInstallation({ download: async () => { stopped.push('download'); }, beforeInstall: async () => { throw new Error('draft save failed'); }, install: async () => { stopped.push('install'); }, active: () => true }), /draft save failed/);
  assert.deepEqual(stopped, ['download']);
  await assert.rejects(performUpdateInstallation({ download: async () => { throw new Error('bad signature'); }, beforeInstall: async () => assert.fail('must not flush'), install: async () => assert.fail('must not install'), active: () => true }), /bad signature/);
  assert.equal(await performUpdateInstallation({ download: async () => {}, beforeInstall: async () => assert.fail('unmounted'), install: async () => assert.fail('unmounted'), active: () => false }), false);
  let active = true;
  assert.equal(await performUpdateInstallation({ download: async () => {}, beforeInstall: async () => { active = false; }, install: async () => assert.fail('unmounted during flush'), active: () => active }), false);
  console.log('App updater state: single-flight StrictMode/manual checks, six-hour failure backoff, skip/manual override, progress monotonicity and malformed totals/events passed.');
  console.log('App updater installation: download/verify precedes latest-draft flush; failed download/save and unmount prevent install.');
})().catch(error => { console.error(error); process.exitCode = 1; });
