import { describe, expect, it } from 'vitest';
import { runRunsCommand, type RunsCliIo } from './runs-cli.ts';

/** `cez runs` is a thin read-only client: what is pinned is the URL it builds, the filtering and
 *  the printed rows — never the route, which has its own tests. */
describe('cez runs', () => {
  const env = { CEZ_API_URL: 'http://127.0.0.1:4321/', CEZ_PROJECT_ID: 'proj' };

  const run = (id: string, overrides: Record<string, unknown> = {}) => ({
    id,
    title: `task ${id}`,
    workflow: 'quick-task',
    task: 'do it',
    status: 'done',
    createdAt: '2026-09-01T10:00:00.000Z',
    tokensUsed: 0,
    archived: false,
    steps: [],
    ...overrides,
  });

  const harness = (reply: { status: number; body: unknown }) => {
    const calls: string[] = [];
    const out: string[] = [];
    const err: string[] = [];
    const io: RunsCliIo = {
      fetch: (async (url: string | URL | Request) => {
        calls.push(String(url));
        return new Response(JSON.stringify(reply.body), { status: reply.status, headers: { 'content-type': 'application/json' } });
      }) as typeof fetch,
      log: (line) => out.push(line),
      error: (line) => err.push(line),
    };
    return { calls, out, err, io };
  };

  const runs = [
    run('aaaaaaaa-1', { createdAt: '2026-09-01T10:00:00.000Z', costUsd: 1.5, branch: 'cez/aaaaaaaa' }),
    run('bbbbbbbb-2', { createdAt: '2026-09-03T10:00:00.000Z', status: 'running', activity: 'monitoring' }),
    run('cccccccc-3', { createdAt: '2026-09-02T10:00:00.000Z', archived: true }),
  ];

  it('list reads the project-scoped runs and prints the unarchived ones newest first', async () => {
    const h = harness({ status: 200, body: runs });
    expect(await runRunsCommand(['list'], env, h.io)).toBe(0);
    expect(h.calls).toEqual(['http://127.0.0.1:4321/api/v1/p/proj/runs']);
    expect(h.out).toEqual([
      'bbbbbbbb  running/monitoring  task bbbbbbbb-2',
      'aaaaaaaa  done  $1.50  task aaaaaaaa-1  [cez/aaaaaaaa]',
    ]);
  });

  it('--all includes archived tasks, --status filters, and the unscoped API is used without a project', async () => {
    const h = harness({ status: 200, body: runs });
    expect(await runRunsCommand(['list', '--all', '--status', 'done'], { CEZ_API_URL: 'http://127.0.0.1:1' }, h.io)).toBe(0);
    expect(h.calls).toEqual(['http://127.0.0.1:1/api/v1/runs']);
    expect(h.out).toEqual([
      'cccccccc  done  task cccccccc-3  (archived)',
      'aaaaaaaa  done  $1.50  task aaaaaaaa-1  [cez/aaaaaaaa]',
    ]);
  });

  it('--json prints the matching records untouched', async () => {
    const h = harness({ status: 200, body: runs });
    expect(await runRunsCommand(['list', '--json', '--status', 'running'], env, h.io)).toBe(0);
    expect(JSON.parse(h.out[0]!)).toEqual([runs[1]]);
  });

  it('refuses an unknown status before calling the cockpit', async () => {
    const h = harness({ status: 200, body: runs });
    expect(await runRunsCommand(['list', '--status', 'finished'], env, h.io)).toBe(1);
    expect(h.err[0]).toContain('--status wants queued, running, waiting, review, done, failed, cancelled, not "finished"');
    expect(h.calls).toHaveLength(0);
  });

  it('relays a server error with a non-zero exit', async () => {
    const h = harness({ status: 404, body: { error: 'unknown project: proj' } });
    expect(await runRunsCommand(['list'], env, h.io)).toBe(1);
    expect(h.err[0]).toBe('cez runs: could not list runs — unknown project: proj');
  });

  it('says so when nothing matches', async () => {
    const empty = harness({ status: 200, body: [] });
    await runRunsCommand(['list'], env, empty.io);
    expect(empty.out).toEqual(['no tasks']);
    const none = harness({ status: 200, body: runs });
    await runRunsCommand(['list', '--status', 'failed'], env, none.io);
    expect(none.out).toEqual(['no tasks match']);
  });

  it('without an address it explains how to set one and never guesses a cockpit', async () => {
    const h = harness({ status: 200, body: runs });
    expect(await runRunsCommand(['list'], {}, h.io)).toBe(2);
    expect(h.err[0]).toContain('CEZ_API_URL=http://127.0.0.1:4321 cez runs list');
    expect(h.calls).toHaveLength(0);
  });

  it('prints usage for help and refuses an unknown command', async () => {
    const help = harness({ status: 200, body: [] });
    expect(await runRunsCommand(['--help'], env, help.io)).toBe(0);
    expect(help.out[0]).toContain('cez runs list');
    const unknown = harness({ status: 200, body: [] });
    expect(await runRunsCommand(['cancel', 'x'], env, unknown.io)).toBe(2);
    expect(unknown.err[0]).toContain('unknown command "cancel"');
    expect(unknown.calls).toHaveLength(0);
  });
});
