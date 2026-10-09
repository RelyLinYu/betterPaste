// Generates the source app icon (1024x1024 RGBA PNG) with zero dependencies.
// Run:  node scripts/gen-icon.mjs   ->  assets/icon.png
import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT = resolve(__dirname, "../assets/icon.png");

const S = 1024;

// ---------- tiny raster helpers ----------
const buf = new Uint8Array(S * S * 4); // RGBA, transparent by default

const put = (x, y, r, g, b, a) => {
  if (x < 0 || y < 0 || x >= S || y >= S || a <= 0) return;
  const i = (y * S + x) * 4;
  const sa = a / 255;
  const da = buf[i + 3] / 255;
  const oa = sa + da * (1 - sa);
  if (oa <= 0) return;
  buf[i] = Math.round((r * sa + buf[i] * da * (1 - sa)) / oa);
  buf[i + 1] = Math.round((g * sa + buf[i + 1] * da * (1 - sa)) / oa);
  buf[i + 2] = Math.round((b * sa + buf[i + 2] * da * (1 - sa)) / oa);
  buf[i + 3] = Math.round(oa * 255);
};

// signed distance to a rounded rectangle centred at (cx,cy)
const sdRoundRect = (x, y, cx, cy, w, h, r) => {
  const dx = Math.abs(x - cx) - (w / 2 - r);
  const dy = Math.abs(y - cy) - (h / 2 - r);
  const ox = Math.max(dx, 0);
  const oy = Math.max(dy, 0);
  return Math.min(Math.max(dx, dy), 0) + Math.hypot(ox, oy) - r;
};

const coverage = (d, aa = 1.2) => Math.min(1, Math.max(0, 0.5 - d / aa));

// ---------- 1. rounded square, vertical gradient (#6366F1 -> #8B5CF6) ----------
const R = 232;
for (let y = 0; y < S; y++) {
  for (let x = 0; x < S; x++) {
    const d = sdRoundRect(x + 0.5, y + 0.5, S / 2, S / 2, S, S, R);
    const c = coverage(d);
    if (c <= 0) continue;
    const t = y / (S - 1);
    put(x, y, Math.round(99 + (139 - 99) * t), Math.round(102 + (92 - 102) * t), Math.round(241 + (246 - 241) * t), Math.round(255 * c));
  }
}

// ---------- 2. clipboard body ----------
const BW = 452; // body width
const BH = 560; // body height
const BX = S / 2;
const BY = 556;
const BR = 64;

for (let y = 0; y < S; y++) {
  for (let x = 0; x < S; x++) {
    const d = sdRoundRect(x + 0.5, y + 0.5, BX, BY, BW, BH, BR);
    const c = coverage(d);
    if (c > 0) put(x, y, 255, 255, 255, Math.round(255 * c));
  }
}

// ---------- 3. clip at the top (wrapper over the board) ----------
const CW = 236;
const CH = 112;
const CX = S / 2;
const CY = 292;
for (let y = 0; y < S; y++) {
  for (let x = 0; x < S; x++) {
    const d = sdRoundRect(x + 0.5, y + 0.5, CX, CY, CW, CH, 46);
    const c = coverage(d);
    if (c > 0) put(x, y, 79, 70, 229, Math.round(255 * c)); // indigo-700
  }
}

// ---------- 4. three content lines (punched out of the white board) ----------
const lines = [
  { y: 520, w: 300 },
  { y: 648, w: 300 },
  { y: 776, w: 190 },
];
for (const ln of lines) {
  const x0 = S / 2 - ln.w / 2;
  const x1 = S / 2 + ln.w / 2;
  for (let y = ln.y - 22; y < ln.y + 22; y++) {
    for (let x = Math.floor(x0); x < Math.ceil(x1); x++) {
      const d = sdRoundRect(x + 0.5, y + 0.5, (x0 + x1) / 2, ln.y, ln.w, 44, 22);
      const c = coverage(d);
      if (c <= 0) continue;
      const i = (y * S + x) * 4;
      const t = c; // blend towards slate-400
      buf[i] = Math.round(buf[i] * (1 - t) + 148 * t);
      buf[i + 1] = Math.round(buf[i + 1] * (1 - t) + 163 * t);
      buf[i + 2] = Math.round(buf[i + 2] * (1 - t) + 184 * t);
    }
  }
}

// ---------- PNG encode ----------
const raw = Buffer.alloc((S * 4 + 1) * S);
for (let y = 0; y < S; y++) {
  raw[y * (S * 4 + 1)] = 0; // filter: none
  Buffer.from(buf.buffer, y * S * 4, S * 4).copy(raw, y * (S * 4 + 1) + 1);
}

const crcTable = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
  }
  return t;
})();
const crc32 = (b) => {
  let c = -1;
  for (let i = 0; i < b.length; i++) c = crcTable[(c ^ b[i]) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
};

const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
};

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0);
ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // colour type RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, png);
console.log(`wrote ${OUT} (${S}x${S}, ${(png.length / 1024).toFixed(1)} KiB)`);
