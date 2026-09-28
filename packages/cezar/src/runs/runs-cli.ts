/**
 * `cez runs …` — read a running cockpit's tasks from a shell (#1080).
 *
 * A thin, read-only HTTP client over `GET /api/v1/runs`, addressed like `cez task` and
 * `cez automation`: `CEZ_API_URL` (the cockpit) and optionally `CEZ_PROJECT_ID` (which project;
 * without it the cockpit answers for the project it was started in). No server: the command says
 * so and exits 2 — the address is never guessed, see `cockpit-address.ts`.
 *
 * Read-only on purpose. Cancelling or relaunching a task from a shell is a separate surface with
 * its own questions (which verbs, what an agent may do with them); listing is what a person needs
 * first, instead of reading `.ai/cezar/runs.json` by hand.
 */
import { parseArgs } from 'node:util';
import { z } from 'zod';
import { runRecordSchema, runStatusSchema, type RunStatus } from '@open-mercato/cezar-contract';
import { missingCockpitMessage } from '../cockpit-address.ts';

export interface RunsCliEnv {
  CEZ_API_URL?: string;
  CEZ_PROJECT_ID?: string;
  CEZ_TASK_ID?: string;
}

export interface RunsCliIo {
  fetch: typeof fetch;
  log: (line: string) => void;
  error: (line: string) => void;
}

const USAGE = `cez runs — read the tasks of a running cockpit from a shell (CEZ_API_URL, optional CEZ_PROJECT_ID)

  cez runs list [--status <status>[,<status>]] [--all] [--json]
                    this project's tasks, newest first: id, status, cost, title and branch;
                    --status keeps only ${runStatusSchema.options.join('|')};
                    --all includes archived tasks; --json prints the matching records as the API sends them`;

/** The keys `list` prints. The rest of each record travels untouched to `--json`. */
const listedRunSchema = runRecordSchema.pick({
  id: true,
  title: true,
  status: true,
  activity: true,
  createdAt: true,
  costUsd: true,
  branch: true,
  archived: true,
});

function base(env: RunsCliEnv): string | null {
  const url = env.CEZ_API_URL?.replace(/\/+$/, '');
  if (!url) return null;
  return env.CEZ_PROJECT_ID ? `${url}/api/v1/p/${encodeURIComponent(env.CEZ_PROJECT_ID)}` : `${url}/api/v1`;
}

async function readError(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { error?: unknown };
    if (typeof body.error === 'string') return body.error;
  } catch {
    // not JSON
  }
  return `${response.status} ${response.statusText}`;
}

function parseStatuses(value: string | undefined): Set<RunStatus> | null {
  if (value === undefined) return null;
  const statuses = new Set<RunStatus>();
  for (const part of value.split(',').map((status) => status.trim()).filter(Boolean)) {
    const parsed = runStatusSchema.safeParse(part);
    if (!parsed.success) throw new Error(`--status wants ${runStatusSchema.options.join(', ')}, not "${part}"`);
    statuses.add(parsed.data);
  }
  if (statuses.size === 0) throw new Error('--status needs at least one status');
  return statuses;
}

export async function runRunsCommand(
  args: string[],
  env: RunsCliEnv = process.env,
  io: RunsCliIo = { fetch, log: console.log, error: console.error },
): Promise<number> {
  const [command, ...rest] = args;
  if (!command || command === 'help' || command === '--help') {
    io.log(USAGE);
    return command ? 0 : 2;
  }
  if (rest.includes('--help') || rest.includes('-h')) {
    io.log(USAGE);
    return 0;
  }
  if (command !== 'list') {
    io.error(`cez runs: unknown command "${command}"\n\n${USAGE}`);
    return 2;
  }
  const scope = base(env);
  if (!scope) {
    io.error(missingCockpitMessage({
      command: 'cez runs',
      example: 'cez runs list',
      insideTask: 'this cockpit did not give the task an address. Read your own task tree with `cez task list` instead, or stop and report that the cockpit is unreachable.',
    }, env));
    return 2;
  }

  try {
    const { values } = parseArgs({
      args: rest,
      allowPositionals: false,
      options: {
        status: { type: 'string' },
        all: { type: 'boolean', default: false },
        json: { type: 'boolean', default: false },
      },
    });
    const statuses = parseStatuses(values.status);
    const response = await io.fetch(`${scope}/runs`);
    if (!response.ok) throw new Error(`could not list runs — ${await readError(response)}`);
    const raw: unknown = await response.json();
    const parsed = z.array(z.unknown()).safeParse(raw);
    if (!parsed.success) throw new Error('the cockpit answered GET /runs with something other than a list');
    const runs = parsed.data.flatMap((record) => {
      const run = listedRunSchema.safeParse(record);
      return run.success ? [{ run: run.data, record }] : [];
    });
    const kept = runs
      .filter(({ run }) => (values.all || !run.archived) && (!statuses || statuses.has(run.status)))
      .sort((a, b) => b.run.createdAt.localeCompare(a.run.createdAt));
    if (values.json) {
      io.log(JSON.stringify(kept.map(({ record }) => record), null, 2));
      return 0;
    }
    if (kept.length === 0) {
      io.log(runs.length === 0 ? 'no tasks' : 'no tasks match');
      return 0;
    }
    for (const { run } of kept) {
      const status = run.activity ? `${run.status}/${run.activity}` : run.status;
      const cost = run.costUsd !== undefined ? `  $${run.costUsd.toFixed(2)}` : '';
      const archived = run.archived ? '  (archived)' : '';
      io.log(`${run.id.slice(0, 8)}  ${status}${cost}  ${run.title}${run.branch ? `  [${run.branch}]` : ''}${archived}`);
    }
    return 0;
  } catch (error) {
    io.error(`cez runs: ${error instanceof Error ? error.message : String(error)}`);
    return 1;
  }
}
