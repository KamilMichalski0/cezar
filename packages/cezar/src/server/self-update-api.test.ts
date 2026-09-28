import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { Hono } from 'hono';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { RunStore } from '../runs/store.ts';
import type { RunManager } from '../workflows/run.ts';
import { apiRequest } from './loopback-request.testkit.ts';
import { createApp } from './server.ts';

/**
 * `/api/v1/workspace/self-update` (self-update PoC). The load-bearing security property: a
 * HOSTED cockpit (`CEZ_REMOTE`) may only apply a version NEWER than the one it runs. Installing
 * a published package cannot inject code, but installing an older one can — every hosted guard
 * (the `/api/*` request-origin check #426, the `localHandoff` 409 on agent-config writes that
 * closes the hooks RCE path) lives in the running version, so a downgrade to a release that
 * predates them re-opens exactly what they close. A local cockpit keeps the whole picker.
 */
describe('the self-update API', () => {
  let repoRoot: string;
  let home: string;
  let store: RunStore;
  let app: Hono;
  const prevRemote = process.env.CEZ_REMOTE;
  const prevHome = process.env.CEZ_HOME;

  beforeEach(() => {
    delete process.env.CEZ_REMOTE;
    repoRoot = mkdtempSync(join(tmpdir(), 'cez-selfupdate-'));
    home = mkdtempSync(join(tmpdir(), 'cez-selfupdate-home-'));
    process.env.CEZ_HOME = home;
    mkdirSync(join(repoRoot, '.ai/cezar'), { recursive: true });
    store = RunStore.open(join(repoRoot, '.ai/cezar'));
    app = createApp({ repoRoot, store, manager: {} as RunManager, version: '0.12.0' });
  });
  afterEach(() => {
    store.flush();
    rmSync(repoRoot, { recursive: true, force: true });
    rmSync(home, { recursive: true, force: true });
    if (prevRemote === undefined) delete process.env.CEZ_REMOTE;
    else process.env.CEZ_REMOTE = prevRemote;
    if (prevHome === undefined) delete process.env.CEZ_HOME;
    else process.env.CEZ_HOME = prevHome;
  });

  const apply = (version: string) =>
    apiRequest(app, '/api/v1/workspace/self-update/apply', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ version }),
    });

  it('answers the status with the running version and an install kind', async () => {
    const res = await apiRequest(app, '/api/v1/workspace/self-update');
    expect(res.status).toBe(200);
    const body = (await res.json()) as { version: string; installKind: string; job: unknown };
    expect(body.version).toBe('0.12.0');
    expect(['managed', 'global-npm', 'npx', 'checkout', 'unknown']).toContain(body.installKind);
    expect(body.job).toBeNull();
  });

  it('rejects a body that is not a plain version string', async () => {
    for (const version of ['', 'https://evil.example/x.tgz', '../../etc', 'a'.repeat(65)]) {
      const res = await apply(version);
      expect(res.status).toBe(400);
    }
  });

  it('refuses a downgrade in hosted mode, naming the forward-only rule', async () => {
    process.env.CEZ_REMOTE = '1';
    for (const older of ['0.11.1', '0.12.0', '0.12.0-nightly.20260927.60', '0.11.1+local']) {
      const res = await apply(older);
      expect(res.status).toBe(409);
      expect((await res.json()) as { error: string }).toMatchObject({
        error: expect.stringContaining('can only update forward'),
      });
    }
  });

  it('lets a newer version past the hosted guard (the capability check answers instead)', async () => {
    process.env.CEZ_REMOTE = '1';
    const res = await apply('0.13.0');
    expect(res.status).toBe(409);
    // Past the forward-only guard: whatever refuses now is the install-kind capability, not it.
    expect(((await res.json()) as { error: string }).error).not.toContain('can only update forward');
  });

  it('never applies the forward-only rule to a local cockpit', async () => {
    for (const older of ['0.11.1', '0.12.0']) {
      const res = await apply(older);
      expect(res.status).toBe(409);
      expect(((await res.json()) as { error: string }).error).not.toContain('can only update forward');
    }
  });
});
