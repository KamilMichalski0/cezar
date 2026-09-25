import { useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { getSelfUpdate } from '@/api/client'
import {
  useApplySelfUpdate,
  useRefreshSelfUpdate,
  useSelfUpdate,
  useSetSelfUpdateChannel,
  workspaceQueryKeys,
} from '@/api/queries'
import type { SelfUpdateStatus, UpdateChannel } from '@open-mercato/cezar-api-client'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { cn } from '@/lib/utils'

/**
 * The dialog behind the footer's version chip (self-update PoC): which channel cezar follows,
 * whether something newer is out, and a picker over every stable release and nightly so a
 * downgrade is one click too. Applying = the server installs into `~/.cezar/versions`, flips
 * `current`, and restarts; the dialog then waits for the new process to answer and reloads.
 *
 * Only a managed install (`cezar install`) can do that to itself. Any other install kind gets
 * the reason and the one command that gets it there — never a download it could not use.
 */
export function SelfUpdateDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const status = useSelfUpdate(open)
  const refresh = useRefreshSelfUpdate()
  const setChannel = useSetSelfUpdateChannel()
  const apply = useApplySelfUpdate()
  const [picked, setPicked] = useState<string>('')
  const data = status.data

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="self-update-dialog" className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>cezar {data ? `v${data.version}` : ''}</DialogTitle>
          <DialogDescription>{data ? installKindSentence(data) : 'Reading update state…'}</DialogDescription>
        </DialogHeader>

        {data ? (
          <div className="flex flex-col gap-4">
            <ChannelRow
              data={data}
              busy={setChannel.isPending}
              onChange={(channel) => setChannel.mutate(channel)}
            />

            <LatestCard
              data={data}
              checking={refresh.isPending}
              onCheck={() => refresh.mutate()}
              onApply={(version) => apply.mutate(version)}
              applying={apply.isPending}
            />

            <VersionPicker
              data={data}
              picked={picked}
              onPick={setPicked}
              onApply={(version) => apply.mutate(version)}
              applying={apply.isPending}
            />

            {!data.canSelfUpdate ? <InstallHint data={data} /> : null}

            {data.activeRuns > 0 && data.canSelfUpdate ? (
              <p className="rounded-md border border-pending/50 bg-pending/10 px-3 py-2 text-[12.5px] text-foreground">
                {data.activeRuns} task{data.activeRuns === 1 ? ' is' : 's are'} running. A restart interrupts
                them; cezar re-queues or resumes them on the way back up.
              </p>
            ) : null}

            {apply.error ? <p className="text-[12.5px] text-danger">{apply.error.message}</p> : null}

            {data.job ? <JobPanel data={data} /> : null}
          </div>
        ) : status.error ? (
          <p className="text-[13px] text-danger">{status.error.message}</p>
        ) : null}

        <DialogFooter>
          <Button variant="outline" size="sm" onClick={() => onOpenChange(false)}>
            Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function installKindSentence(data: SelfUpdateStatus): string {
  switch (data.installKind) {
    case 'managed':
      return `Managed install — ${data.installed.find((entry) => entry.active)?.id ?? 'current'} under ~/.cezar/versions. Updates apply from here and restart cezar.`
    case 'npx':
      return 'Running from the npx cache.'
    case 'global-npm':
      return 'Installed globally with npm.'
    case 'checkout':
      return 'Running from a git checkout.'
    default:
      return 'Install location unknown.'
  }
}

function ChannelRow({
  data,
  busy,
  onChange,
}: {
  data: SelfUpdateStatus
  busy: boolean
  onChange: (channel: UpdateChannel) => void
}) {
  return (
    <div className="flex items-center justify-between gap-3">
      <div className="min-w-0">
        <div className="text-[13px] font-semibold">Release channel</div>
        <div className="text-[12px] text-muted-foreground">
          stable {data.latest.stable ? `v${data.latest.stable}` : '—'} · nightly{' '}
          {data.latest.nightly ? `v${data.latest.nightly}` : '—'}
        </div>
      </div>
      <Select value={data.channel} onValueChange={(value) => onChange(value as UpdateChannel)} disabled={busy}>
        <SelectTrigger size="sm" aria-label="Release channel" className="w-[130px] text-[13px]">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="stable">Stable</SelectItem>
          <SelectItem value="nightly">Nightly</SelectItem>
        </SelectContent>
      </Select>
    </div>
  )
}

function LatestCard({
  data,
  checking,
  onCheck,
  onApply,
  applying,
}: {
  data: SelfUpdateStatus
  checking: boolean
  onCheck: () => void
  onApply: (version: string) => void
  applying: boolean
}) {
  const jobBusy = data.job?.status === 'running' || data.job?.status === 'restarting'
  const target = data.updateAvailable
  return (
    <div
      data-slot="self-update-latest"
      className={cn(
        'flex items-center justify-between gap-3 rounded-md border px-3 py-2.5',
        target ? 'border-primary/40 bg-primary/5' : 'border-border bg-card',
      )}
    >
      <div className="min-w-0 text-[13px]">
        {!data.checkedAt ? (
          <span className="text-muted-foreground">The npm registry has not answered yet.</span>
        ) : target ? (
          <>
            <span className="font-semibold">v{target}</span> is available on {data.channel}.
          </>
        ) : (
          <>You are on the newest {data.channel} version.</>
        )}
        {data.checkedAt ? (
          <div className="text-[11.5px] text-muted-foreground">checked {new Date(data.checkedAt).toLocaleTimeString()}</div>
        ) : null}
      </div>
      <div className="flex shrink-0 items-center gap-2">
        <Button variant="ghost" size="sm" onClick={onCheck} disabled={checking || jobBusy}>
          {checking ? 'Checking…' : 'Check again'}
        </Button>
        {target ? (
          <Button size="sm" onClick={() => onApply(target)} disabled={!data.canSelfUpdate || applying || jobBusy}>
            Update &amp; restart
          </Button>
        ) : null}
      </div>
    </div>
  )
}

function VersionPicker({
  data,
  picked,
  onPick,
  onApply,
  applying,
}: {
  data: SelfUpdateStatus
  picked: string
  onPick: (version: string) => void
  onApply: (version: string) => void
  applying: boolean
}) {
  const jobBusy = data.job?.status === 'running' || data.job?.status === 'restarting'
  // Local builds only exist in `installed`; registry versions come from `available`, with the
  // `installed` flag telling the two apart in the label.
  const options = useMemo(() => {
    const locals = data.installed
      .filter((entry) => entry.source === 'local')
      .map((entry) => ({ value: entry.id, label: `v${entry.version} · local build`, installed: true, active: entry.active }))
    const remote = data.available.map((entry) => ({
      value: entry.version,
      label: `v${entry.version}${entry.channel === 'nightly' ? ' · nightly' : ''}${entry.publishedAt ? ` · ${entry.publishedAt.slice(0, 10)}` : ''}`,
      installed: entry.installed,
      active: data.installed.some((row) => row.active && row.id === entry.version),
    }))
    return [...locals, ...remote]
  }, [data])
  const selected = options.find((option) => option.value === picked)
  return (
    <div data-slot="self-update-picker" className="flex flex-col gap-2">
      <div className="text-[13px] font-semibold">Pick a version</div>
      <div className="flex items-center gap-2">
        <Select value={picked} onValueChange={onPick} disabled={options.length === 0 || jobBusy}>
          <SelectTrigger size="sm" aria-label="Version" className="min-w-0 flex-1 text-[13px]">
            <SelectValue placeholder={options.length === 0 ? 'No versions known' : 'Choose a version to install'} />
          </SelectTrigger>
          <SelectContent className="max-h-72">
            {options.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                <span className="flex items-center gap-2">
                  {option.label}
                  {option.active ? (
                    <Badge variant="secondary">current</Badge>
                  ) : option.installed ? (
                    <Badge variant="outline">installed</Badge>
                  ) : null}
                </span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button
          size="sm"
          variant="contrast"
          onClick={() => selected && onApply(selected.value)}
          disabled={!selected || selected.active || !data.canSelfUpdate || applying || jobBusy}
        >
          {selected?.installed ? 'Switch & restart' : 'Install & restart'}
        </Button>
      </div>
      <p className="text-[11.5px] text-muted-foreground">
        Older versions work too — that is the downgrade path. Installed versions stay under
        ~/.cezar/versions, so switching back is instant.
      </p>
    </div>
  )
}

function InstallHint({ data }: { data: SelfUpdateStatus }) {
  const command =
    data.installKind === 'checkout'
      ? 'node packages/cezar/dist/index.js install'
      : data.installKind === 'global-npm'
        ? 'cezar install'
        : 'npx cezar-cli install'
  return (
    <div data-slot="self-update-install-hint" className="rounded-md border border-border bg-muted/40 px-3 py-2.5 text-[12.5px]">
      <p>{data.reason}</p>
      <p className="mt-1.5 text-muted-foreground">Run this once, then start cezar with the plain command:</p>
      <pre className="mt-1.5 overflow-x-auto rounded bg-background px-2 py-1.5 font-mono text-[12px]">{command}</pre>
    </div>
  )
}

/** The install log while npm runs, then the restart wait: once the process is gone, poll until a
 *  fresh one (no job on its status) answers, and reload into it. */
function JobPanel({ data }: { data: SelfUpdateStatus }) {
  const job = data.job!
  const queryClient = useQueryClient()
  const logRef = useRef<HTMLPreElement>(null)
  const [comeback, setComeback] = useState<'waiting' | 'back' | 'timeout'>('waiting')

  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight })
  }, [job.log.length])

  useEffect(() => {
    if (job.status !== 'restarting') return
    let cancelled = false
    const startedAt = Date.now()
    const tick = async () => {
      if (cancelled) return
      try {
        const fresh = await getSelfUpdate()
        // The old process still answering carries this very job; the new one starts clean.
        if (!fresh.job) {
          setComeback('back')
          queryClient.removeQueries({ queryKey: workspaceQueryKeys.selfUpdate })
          window.setTimeout(() => window.location.reload(), 400)
          return
        }
      } catch {
        // Down between the two processes — expected.
      }
      if (Date.now() - startedAt > 90_000) {
        setComeback('timeout')
        return
      }
      window.setTimeout(tick, 1_000)
    }
    window.setTimeout(tick, 1_500)
    return () => {
      cancelled = true
    }
  }, [job.status, queryClient])

  return (
    <div data-slot="self-update-job" className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between text-[12.5px]">
        <span className="font-semibold">
          {job.status === 'running'
            ? `Installing ${job.target}…`
            : job.status === 'failed'
              ? `Install of ${job.target} failed`
              : comeback === 'back'
                ? 'Back — reloading'
                : comeback === 'timeout'
                  ? 'Restart took too long — reload the page by hand'
                  : `Restarting into ${job.target}…`}
        </span>
        <span className="text-muted-foreground">{job.log.length} lines</span>
      </div>
      <pre
        ref={logRef}
        className="max-h-40 overflow-auto rounded-md border border-border bg-background px-2 py-1.5 font-mono text-[11px] leading-[1.5] text-muted-foreground"
      >
        {job.log.join('\n')}
      </pre>
      {job.error ? <p className="text-[12.5px] text-danger">{job.error}</p> : null}
    </div>
  )
}
