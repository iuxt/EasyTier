const fs = require('node:fs/promises');
const path = require('node:path');
const { packageAssets } = require('./nightly-release.js');

async function prepare({ context, source, directory, manifest }) {
  if (context.eventName !== 'push' || context.ref !== 'refs/heads/main') {
    throw new Error('Automatic releases require a push to main.');
  }
  if (!/^[a-f0-9]{40}$/.test(context.sha) ||
      !Number.isSafeInteger(context.runNumber) || context.runNumber < 1) {
    throw new Error('A release requires an exact commit and a valid build number.');
  }
  const version = manifest.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error('The build must declare a valid release version.');
  }
  const tag = `v${version}-build.${context.runNumber}`;
  const artifacts = [];
  for (const entry of await fs.readdir(source, { withFileTypes: true })) {
    if (!entry.isDirectory() || !/^easytier-[\w.-]+$/.test(entry.name)) {
      throw new Error(`Unexpected release artifact: ${entry.name}`);
    }
    const binary = entry.name.startsWith('easytier-windows-') ? 'easytier-core.exe' : 'easytier-core';
    for (const name of [binary, 'config.toml']) {
      const file = path.join(source, entry.name, name);
      const stat = await fs.stat(file);
      if (!stat.isFile() || !stat.size) throw new Error(`Missing or empty release file: ${file}`);
    }
    artifacts.push({ name: entry.name });
  }
  if (!artifacts.length) throw new Error('No release artifacts were downloaded.');
  const build = { sha: context.sha, tag, version: tag, artifacts: { core: artifacts } };
  await packageAssets({ build, source, directory });
  return build;
}

async function publish({ github, context, core, build, directory }) {
  const repository = context.repo;
  let release;
  try {
    ({ data: release } = await github.rest.repos.getReleaseByTag({ ...repository, tag: build.tag }));
  } catch (error) {
    if (error.status !== 404) throw error;
    // A draft may not have a tag ref yet, so tag lookup alone can miss it.
    const releases = await github.paginate(github.rest.repos.listReleases, {
      ...repository, per_page: 100,
    });
    release = releases.find(item => item.tag_name === build.tag);
  }
  if (release && release.target_commitish !== build.sha) {
    throw new Error(`${build.tag} belongs to another commit.`);
  }
  if (release && !release.draft) {
    core.info(`${build.tag} is already published: ${release.html_url}`);
    return release;
  }
  const url = `${context.serverUrl || 'https://github.com'}/${repository.owner}/${repository.repo}`;
  const body = [
    'Automated build from `main`.', '',
    `Commit: [${build.sha}](${url}/commit/${build.sha})`,
    `Build: [#${context.runNumber}](${url}/actions/runs/${context.runId})`, '',
    'Each ZIP includes easytier-core and config.toml. Windows ZIPs also include the required DLLs and drivers.',
    'Verify downloads against SHA256SUMS.',
  ].join('\n');
  if (!release) {
    ({ data: release } = await github.rest.repos.createRelease({
      ...repository, tag_name: build.tag, target_commitish: build.sha,
      name: build.tag, body, draft: true, prerelease: false, make_latest: 'false',
    }));
  }
  const previous = await github.paginate(github.rest.repos.listReleaseAssets, {
    ...repository, release_id: release.id, per_page: 100,
  });
  // A failed upload leaves a draft. Reruns replace its partial asset set.
  for (const asset of previous) {
    await github.rest.repos.deleteReleaseAsset({ ...repository, asset_id: asset.id });
  }
  for (const name of build.assets) {
    const data = await fs.readFile(path.join(directory, name));
    await github.rest.repos.uploadReleaseAsset({
      ...repository, release_id: release.id, name, data,
      headers: { 'content-type': name.endsWith('.zip') ? 'application/zip' : 'application/octet-stream' },
    });
  }
  const { data: branch } = await github.rest.repos.getBranch({ ...repository, branch: 'main' });
  // A slower, older build may have completed after a new push.
  const { data: published } = await github.rest.repos.updateRelease({
    ...repository, release_id: release.id, name: build.tag, body,
    draft: false, prerelease: false,
    make_latest: branch.commit.sha === build.sha ? 'true' : 'false',
  });
  core.info(`Published ${build.tag}: ${published.html_url}`);
  if (core.summary) {
    await core.summary.addHeading('Release').addLink(build.tag, published.html_url).write();
  }
  return published;
}

async function releasePush({ github, context, core, source = 'push-artifacts', directory = 'push-assets' }) {
  const manifest = await fs.readFile('easytier/Cargo.toml', 'utf8');
  const build = await prepare({ context, source, directory, manifest });
  return publish({ github, context, core, build, directory });
}

module.exports = { prepare, publish, releasePush };
