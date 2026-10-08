// Generates the white-background, black-X 1024x1024 app icon without any image
// library: raw RGBA pixels -> PNG (zlib). Re-run `pnpm tauri icon app-icon.png`
// afterwards to refresh src-tauri/icons/.
import { deflateSync } from 'node:zlib'
import { writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const SIZE = 1024
const RADIUS = 200
const STROKE = 112
const MARGIN = 300
const BACKGROUND = [255, 255, 255]
const ACCENT = [0, 0, 0]

const CRC_TABLE = new Int32Array(256).map((_, n) => {
  let c = n
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
  return c
})

function crc32(buf) {
  let crc = -1
  for (let i = 0; i < buf.length; i++) crc = (crc >>> 8) ^ CRC_TABLE[(crc ^ buf[i]) & 0xff]
  return (crc ^ -1) >>> 0
}

function chunk(type, data) {
  const out = Buffer.alloc(8 + data.length + 4)
  out.writeUInt32BE(data.length, 0)
  out.write(type, 4, 'ascii')
  data.copy(out, 8)
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length)
  return out
}

function distToSegment(px, py, ax, ay, bx, by) {
  const dx = bx - ax
  const dy = by - ay
  const t = Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
  return Math.hypot(px - (ax + t * dx), py - (ay + t * dy))
}

const clamp01 = (v) => Math.min(Math.max(v, 0), 1)
const raw = Buffer.alloc(SIZE * (SIZE * 4 + 1))

for (let y = 0; y < SIZE; y++) {
  const row = y * (SIZE * 4 + 1)
  raw[row] = 0
  for (let x = 0; x < SIZE; x++) {
    const cx = Math.min(Math.max(x, RADIUS), SIZE - RADIUS)
    const cy = Math.min(Math.max(y, RADIUS), SIZE - RADIUS)
    const shape = clamp01(RADIUS + 0.5 - Math.hypot(x - cx, y - cy))
    const cross = Math.min(
      distToSegment(x, y, MARGIN, MARGIN, SIZE - MARGIN, SIZE - MARGIN),
      distToSegment(x, y, SIZE - MARGIN, MARGIN, MARGIN, SIZE - MARGIN),
    )
    const line = clamp01(STROKE / 2 + 0.5 - cross)
    const o = row + 1 + x * 4
    for (let i = 0; i < 3; i++) {
      const bg = BACKGROUND[i]
      raw[o + i] = Math.round(bg + (ACCENT[i] - bg) * line)
    }
    raw[o + 3] = Math.round(255 * shape)
  }
}

const ihdr = Buffer.alloc(13)
ihdr.writeUInt32BE(SIZE, 0)
ihdr.writeUInt32BE(SIZE, 4)
ihdr[8] = 8 // bit depth
ihdr[9] = 6 // color type: RGBA

const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk('IHDR', ihdr),
  chunk('IDAT', deflateSync(raw, { level: 9 })),
  chunk('IEND', Buffer.alloc(0)),
])

const outPath = fileURLToPath(new URL('../app-icon.png', import.meta.url))
writeFileSync(outPath, png)
console.log(`Wrote ${outPath} (${png.length} bytes)`)
