const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const { test } = require('node:test');
const { prepare, publish } = require('./push-release.js');

const context = {
  eventName: 'push', ref: 'refs/heads/main', sha: 'a'.repeat(40),
  runNumber: 42, runId: 123, repo: { owner: 'iuxt', repo: 'EasyTier' },
};

async function files(t) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'easytier-push-release-'));
  t.after(() => fs.rm(directory, { recursive: true, force: true }));
  const source = path.join(directory, 'artifacts');
  await fs.mkdir(source);
  for (const name of ['easytier-linux-x86_64', 'easytier-windows-x86_64']) {
    const target = path.join(source, name);
    await fs.mkdir(target);
    await fs.writeFile(path.join(target, name.includes('windows') ? 'easytier-core.exe' : 'easytier-core'), 'binary');
    await fs.writeFile(path.join(target, 'config.toml'), 'hostname = "test"');
  }
  return { context, source, directory: path.join(directory, 'assets'), manifest: 'version = "2.7.0"\n' };
}

test('main push packages the exact build with ZIPs and checksums without a release baseline', async t => {
  const options = await files(t);
  const build = await prepare(options);
  assert.equal(build.tag, 'v2.7.0-build.42');
  assert.equal(build.sha, context.sha);
  assert.deepEqual(build.assets, [
    'SHA256SUMS',
    'easytier-linux-x86_64-v2.7.0-build.42.zip',
    'easytier-windows-x86_64-v2.7.0-build.42.zip',
  ]);
  const sums = await fs.readFile(path.join(options.directory, 'SHA256SUMS'), 'utf8');
  assert.equal(sums.trim().split('\n').length, 2);
});

test('PRs, other branches and invalid identities cannot publish a push release', async t => {
  const options = await files(t);
  for (const change of [
    { eventName: 'pull_request' }, { ref: 'refs/heads/develop' },
    { sha: 'main' }, { runNumber: 0 },
  ]) {
    await assert.rejects(prepare({ ...options, context: { ...context, ...change } }));
  }
  await assert.rejects(prepare({ ...options, manifest: 'version = "../../invalid"' }), /valid release version/);
});

test('missing or empty binaries stop packaging before a release can be created', async t => {
  const options = await files(t);
  const binary = path.join(options.source, 'easytier-linux-x86_64', 'easytier-core');
  await fs.writeFile(binary, '');
  await assert.rejects(prepare(options), /empty release file/);
  await fs.rm(binary);
  await assert.rejects(prepare(options), /ENOENT/);
  await fs.rm(options.source, { recursive: true });
  await fs.mkdir(options.source);
  await assert.rejects(prepare(options), /No release artifacts/);
});

function fixture(build, directory) {
  const calls = [];
  const state = { release: null, head: context.sha, assets: [] };
  const repos = {
    async getReleaseByTag() {
      if (!state.release || (state.hideDraft && state.release.draft)) {
        throw Object.assign(new Error('Not found'), { status: 404 });
      }
      return { data: state.release };
    },
    async createRelease(request) {
      calls.push(['create', request]);
      state.release = { ...request, id: 1, html_url: 'https://github.com/iuxt/EasyTier/releases/tag/test' };
      return { data: state.release };
    },
    listReleaseAssets() {},
    listReleases() {},
    async deleteReleaseAsset(request) { calls.push(['delete', request.asset_id]); },
    async uploadReleaseAsset(request) {
      calls.push(['upload', request.name]);
      if (state.uploadFails) throw new Error('Upload failed');
    },
    async getBranch() { return { data: { commit: { sha: state.head } } }; },
    async updateRelease(request) {
      calls.push(['publish', request]);
      Object.assign(state.release, request);
      return { data: state.release };
    },
  };
  const github = { rest: { repos }, paginate: async method => (
    method === repos.listReleases ? (state.release ? [state.release] : []) : state.assets
  ) };
  return { state, calls, run: () => publish({ github, context, core: { info() {} }, build, directory }) };
}

test('a new release stays draft until all uploads finish and is then marked Latest', async t => {
  const options = await files(t);
  const build = await prepare(options);
  const f = fixture(build, options.directory);
  await f.run();
  assert.equal(f.calls[0][1].draft, true);
  assert.equal(f.calls[0][1].target_commitish, context.sha);
  assert.deepEqual(f.calls.filter(c => c[0] === 'upload').map(c => c[1]), build.assets);
  const published = f.calls.at(-1);
  assert.equal(published[0], 'publish');
  assert.equal(published[1].draft, false);
  assert.equal(published[1].prerelease, false);
  assert.equal(published[1].make_latest, 'true');
});

test('failed uploads leave a draft that can be resumed by rerunning', async t => {
  const options = await files(t);
  const build = await prepare(options);
  const f = fixture(build, options.directory);
  f.state.uploadFails = true;
  await assert.rejects(f.run(), /Upload failed/);
  assert.equal(f.state.release.draft, true);
  assert.ok(!f.calls.some(c => c[0] === 'publish'));
  f.state.uploadFails = false;
  f.state.hideDraft = true;
  f.state.assets = [{ id: 10, name: 'SHA256SUMS' }];
  await f.run();
  assert.equal(f.calls.filter(c => c[0] === 'create').length, 1);
  assert.ok(f.calls.some(c => c[0] === 'delete' && c[1] === 10));
  assert.equal(f.state.release.draft, false);
});

test('a superseded build cannot replace Latest and published reruns are idempotent', async t => {
  const options = await files(t);
  const build = await prepare(options);
  const f = fixture(build, options.directory);
  f.state.head = 'b'.repeat(40);
  await f.run();
  assert.equal(f.calls.at(-1)[1].make_latest, 'false');
  const count = f.calls.length;
  await f.run();
  assert.equal(f.calls.length, count);
  f.state.release.target_commitish = 'b'.repeat(40);
  await assert.rejects(f.run(), /another commit/);
});
