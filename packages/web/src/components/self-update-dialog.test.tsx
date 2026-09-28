import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { SelfUpdateStatus } from '@open-mercato/cezar-api-client'
import { workspaceQueryKeys } from '@/api/queries'
import { SelfUpdateDialog } from '@/components/self-update-dialog'

const status: SelfUpdateStatus = {
  version: '0.12.0',
  installKind: 'managed',
  entry: '/home/u/.cezar/versions/current/node_modules/@open-mercato/cezar/dist/index.js',
  canSelfUpdate: true,
  channel: 'stable',
  restartMode: 'reexec',
  latest: { stable: '0.12.0', nightly: '0.12.0-nightly.20260927.60' },
  updateAvailable: null,
  checkedAt: '2026-09-28T08:04:09.000Z',
  installed: [
    { id: '0.12.0+local', version: '0.12.0', source: 'local', installedAt: '2026-09-28T08:00:00.000Z', active: true },
    { id: '0.11.1', version: '0.11.1', source: 'registry', installedAt: '2026-09-25T14:03:47.000Z', active: false },
  ],
  available: [
    { version: '0.12.0', channel: 'stable', publishedAt: '2026-09-27T10:00:00.000Z', installed: false },
    { version: '0.11.1', channel: 'stable', publishedAt: '2026-09-21T11:33:05.050Z', installed: true },
    { version: '0.12.0-nightly.20260927.60', channel: 'nightly', publishedAt: '2026-09-27T03:17:00.000Z', installed: false },
  ],
  job: null,
  activeRuns: 0,
}

function renderDialog() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } })
  queryClient.setQueryData(workspaceQueryKeys.selfUpdate, status)
  return render(
    <QueryClientProvider client={queryClient}>
      <SelfUpdateDialog open onOpenChange={() => {}} />
    </QueryClientProvider>,
  )
}

describe('SelfUpdateDialog', () => {
  afterEach(() => vi.restoreAllMocks())

  // The dialog is opened to READ a version. Radix's default — focus the first focusable element —
  // lands on the close button and rings it; the panel takes the initial focus instead, which
  // keeps the focus trap (and Escape) working without highlighting any control.
  it('does not put the initial focus on the close button', async () => {
    renderDialog()
    const dialog = await screen.findByRole('dialog')
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true))
    expect(document.activeElement).toBe(dialog)
    expect(document.activeElement?.tagName).not.toBe('BUTTON')
    // The close button is still there and reachable — only not pre-focused.
    expect(screen.getByRole('button', { name: /close/i })).toBeTruthy()
  })

  it('heads the dialog with the version and marks a local build', async () => {
    renderDialog()
    const dialog = await screen.findByRole('dialog')
    expect(dialog.textContent).toContain('cezar v0.12.0')
    expect(dialog.textContent).toContain('local build')
    expect(screen.getByRole('radio', { name: 'Stable' }).getAttribute('aria-checked')).toBe('true')
  })
})
