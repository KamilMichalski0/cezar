import { existsSync, mkdirSync, mkdtempSync, readlinkSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { activate, activeId, detectInstallKind, installId, listInstalled, versionEntry, versionsDir, writeManifest } from './layout.ts';
import { fetchPackageDocument } from './registry.ts';
import { restartArgs } from './restart.ts';
import { classifyVersion, compareVersions, isNewer } from './semver.ts';

describe('semver', () => {
  it('orders releases numerically, not lexically', () => {
    expect(compareVersions('1.2.10', '1.2.9')).toBe(1);
    expect(compareVersions('0.11.1', '0.9.2')).toBe(1);
    expect(compareVersions('0.11.1', '0.11.1')).toBe(0);
  });

  it('ranks a release above every prerelease of the same core, and nightlies by date', () => {
    expect(isNewer('0.11.1', '0.11.1-nightly.20260924.49')).toBe(true);
    expect(isNewer('0.11.1-nightly.20260924.49', '0.11.1')).toBe(false);
    expect(isNewer('0.11.1-nightly.20260925.50', '0.11.1-nightly.20260924.49')).toBe(true);
    expect(isNewer('0.11.2-nightly.20260901.1', '0.11.1')).toBe(true);
  });

  it('classifies dist-tag families from the prerelease identifier', () => {
    expect(classifyVersion('0.11.1')).toBe('stable');
    expect(classifyVersion('0.11.1-nightly.20260924.49')).toBe('nightly');
    expect(classifyVersion('0.9.2-pr743.1156.2')).toBe('preview');
    expect(classifyVersion('0.1.5-develop.124')).toBe('preview');
  });
});

describe('managed layout', () => {
  let home: string;
  let env: NodeJS.ProcessEnv;

  beforeEach(() => {
    home = mkdtempSync(join(tmpdir(), 'cez-self-update-'));
    env = { ...process.env, CEZ_HOME: home };
  });
  afterEach(() => rmSync(home, { recursive: true, force: true }));

  const fakeInstall = (id: string, version: string, source: 'registry' | 'local') => {
    const entry = versionEntry(id, env);
    mkdirSync(join(entry, '..'), { recursive: true });
    writeFileSync(entry, '// entry\n');
    writeManifest(id, { version, source, installedAt: new Date(Date.parse('2026-09-25T10:00:00Z') + (source === 'local' ? 1 : 0)).toISOString() }, env);
  };

  it('lists complete installs only and flips `current` atomically', () => {
    fakeInstall('0.11.0', '0.11.0', 'registry');
    fakeInstall('0.11.1+local', '0.11.1', 'local');
    // A manifest without an entry file is a torn install and must not be offered.
    writeManifest('0.10.0', { version: '0.10.0', source: 'registry', installedAt: '2026-09-01T00:00:00Z' }, env);

    expect(listInstalled(env).map((entry) => entry.id)).toEqual(['0.11.1+local', '0.11.0']);
    expect(activeId(env)).toBeNull();

    activate('0.11.0', env);
    expect(activeId(env)).toBe('0.11.0');
    expect(readlinkSync(join(versionsDir(env), 'current'))).toBe('0.11.0');

    activate('0.11.1+local', env);
    expect(activeId(env)).toBe('0.11.1+local');
    expect(listInstalled(env).find((entry) => entry.active)?.id).toBe('0.11.1+local');
    expect(existsSync(join(versionsDir(env), `.current.${process.pid}.tmp`))).toBe(false);
    expect(() => activate('9.9.9', env)).toThrow(/not installed/);
  });

  it('derives the install id from the source', () => {
    expect(installId('0.11.1', 'registry')).toBe('0.11.1');
    expect(installId('0.11.1', 'local')).toBe('0.11.1+local');
  });

  it('tells install kinds apart by where the entry file lives', () => {
    expect(detectInstallKind(versionEntry('0.11.1', env), env)).toBe('managed');
    expect(detectInstallKind('/Users/x/.npm/_npx/abc123/node_modules/@open-mercato/cezar/dist/index.js', env)).toBe('npx');
    expect(detectInstallKind('/opt/homebrew/lib/node_modules/@open-mercato/cezar/dist/index.js', env)).toBe('global-npm');
    const checkout = join(home, 'repo', 'packages', 'cezar');
    mkdirSync(join(checkout, 'src'), { recursive: true });
    mkdirSync(join(checkout, 'dist'), { recursive: true });
    writeFileSync(join(checkout, 'package.json'), '{}');
    expect(detectInstallKind(join(checkout, 'dist', 'index.js'), env)).toBe('checkout');
    expect(detectInstallKind(join(home, 'elsewhere', 'index.js'), env)).toBe('unknown');
  });
});

describe('restart args', () => {
  it('keeps the command and flags, pins the port and never opens a second tab', () => {
    expect(restartArgs(['serve', '--repo', '/x', '--port', '4000'], 4321)).toEqual(['serve', '--repo', '/x', '--port', '4321', '--no-open']);
    expect(restartArgs(['-p', '4000', '--no-open'], 4321)).toEqual(['--port', '4321', '--no-open']);
    expect(restartArgs(['--port=4000'], 4321)).toEqual(['--port', '4321', '--no-open']);
  });
});

describe('registry document', () => {
  it('sorts versions newest first, stamps channels and publish dates, and survives a bad payload', async () => {
    const doc = await fetchPackageDocument('@open-mercato/cezar', (async () =>
      new Response(
        JSON.stringify({
          'dist-tags': { latest: '0.11.1', nightly: '0.11.1-nightly.20260924.49' },
          versions: { '0.11.0': {}, '0.11.1': {}, '0.11.1-nightly.20260924.49': {}, '0.9.2-pr743.1156.2': {} },
          time: { '0.11.1': '2026-09-20T00:00:00.000Z' },
        }),
        { status: 200 },
      )) as unknown as typeof fetch);
    expect(doc?.versions.map((entry) => entry.version)).toEqual(['0.11.1', '0.11.1-nightly.20260924.49', '0.11.0', '0.9.2-pr743.1156.2']);
    expect(doc?.versions[0]).toMatchObject({ channel: 'stable', publishedAt: '2026-09-20T00:00:00.000Z' });
    expect(doc?.versions[1]?.channel).toBe('nightly');
    expect(doc?.distTags.nightly).toBe('0.11.1-nightly.20260924.49');

    expect(await fetchPackageDocument('x', (async () => new Response('nope', { status: 500 })) as unknown as typeof fetch)).toBeNull();
    expect(await fetchPackageDocument('x', (async () => { throw new Error('offline'); }) as unknown as typeof fetch)).toBeNull();
  });
});
