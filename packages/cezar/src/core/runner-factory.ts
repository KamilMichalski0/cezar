import type { AgentBackend, AgentRunner, RunnerId } from './agent-runner.ts';
import { ClaudeCliRunner } from './claude-cli-runner.ts';
import { CodexAppServerRunner } from './codex-app-server-runner.ts';
import { OpencodeServerRunner } from './opencode-server-runner.ts';
import { PiRunner } from './pi-runner.ts';

/**
 * The single place that maps a backend id onto a concrete runner. Everything
 * that used to `new ClaudeCliRunner()` (the planner and the workflow engine)
 * goes through here so switching the agent backend is one function call.
 * `claude-cli` is the legacy id for `claude`.
 */
export function createRunner(backend: AgentBackend | RunnerId | undefined): AgentRunner {
  switch (backend) {
    case 'codex':
      return new CodexAppServerRunner();
    case 'opencode':
      return new OpencodeServerRunner();
    case 'pi':
      return new PiRunner();
    case 'copilot':
      // Replaced by `new CopilotAcpRunner()` when the runner class lands (#582 Step 3.1). Until
      // then this case exists so a `copilot` run FAILS instead of falling through to the `default`
      // and silently running Claude under Copilot's name — the landmine §9 of AGENT_PROTOCOL.md
      // and the #582 dossier both call out.
      throw new Error('The copilot runner is not wired up yet.');
    case 'claude':
    case 'claude-cli':
    default:
      return new ClaudeCliRunner();
  }
}
