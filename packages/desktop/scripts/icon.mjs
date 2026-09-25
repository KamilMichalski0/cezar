#!/usr/bin/env node
// Regenerate every app icon from ONE source SVG:
//
//   npm run icon                      # from packages/web/public/open-mercato.svg (the brand mark)
//   npm run icon -- path/to/new.svg   # from a new mark
//
// The SVG is wrapped full-bleed (its gradient covers the whole 1024px canvas — macOS masks the
// squircle itself, and a transparent margin would get a white plate behind it in the Dock),
// rendered to PNG, and fed to `tauri icon`, which writes the .icns/.ico/.png set under
// src-tauri/icons. The splash logo (ui/open-mercato.svg) is refreshed from the same source.
// Rendering uses rsvg-convert when present, else macOS's qlmanage.
import { execFileSync, spawnSync } from 'node:child_process'
import { copyFileSync, existsSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const desktop = resolve(here, '..')
const source = resolve(process.argv[2] ?? resolve(desktop, '../web/public/open-mercato.svg'))
if (!existsSync(source)) {
  console.error(`icon: source not found: ${source}`)
  process.exit(1)
}

const svg = readFileSync(source, 'utf8')
const size = /viewBox="0 0 (\d+(?:\.\d+)?) (\d+(?:\.\d+)?)"/.exec(svg)
if (!size) {
  console.error('icon: the source SVG needs a viewBox="0 0 W H"')
  process.exit(1)
}
const [w, h] = [Number(size[1]), Number(size[2])]
const inner = svg.replace(/^[\s\S]*?<svg[^>]*>/, '').replace(/<\/svg>\s*$/, '')
const gradient = /fill="url\(#([^)]+)\)"/.exec(svg)?.[1]
const scale = 1024 / Math.max(w, h)
// Full bleed: a rect in the mark's own gradient behind it, then the mark itself.
const wrapper = `<svg width="1024" height="1024" viewBox="0 0 1024 1024" fill="none" xmlns="http://www.w3.org/2000/svg">
<g transform="scale(${scale.toFixed(6)})">
${gradient ? `<rect width="${w}" height="${h}" fill="url(#${gradient})"/>` : ''}
${inner}
</g>
</svg>
`
const iconsDir = join(desktop, 'src-tauri', 'icons')
writeFileSync(join(iconsDir, 'source.svg'), wrapper)

const work = mkdtempSync(join(tmpdir(), 'cezar-icon-'))
const png = join(work, 'icon-1024.png')
const wrapperPath = join(work, 'source.svg')
writeFileSync(wrapperPath, wrapper)
if (spawnSync('rsvg-convert', ['--version'], { stdio: 'ignore' }).status === 0) {
  execFileSync('rsvg-convert', ['-w', '1024', '-h', '1024', '-o', png, wrapperPath])
} else if (process.platform === 'darwin') {
  execFileSync('qlmanage', ['-t', '-s', '1024', '-o', work, wrapperPath], { stdio: 'ignore' })
  renameSync(join(work, 'source.svg.png'), png)
} else {
  console.error('icon: need rsvg-convert (apt/brew install librsvg) to render the SVG')
  process.exit(1)
}

execFileSync('npx', ['tauri', 'icon', png, '-o', iconsDir], { cwd: desktop, stdio: 'inherit' })
for (const extra of ['android', 'ios']) rmSync(join(iconsDir, extra), { recursive: true, force: true })
copyFileSync(source, join(desktop, 'ui', 'open-mercato.svg'))
rmSync(work, { recursive: true, force: true })
console.log(`icon: regenerated ${iconsDir} and ui/open-mercato.svg from ${source}`)
