// Generates the app source icon (1024x1024 RGBA PNG) with zero dependencies.
// Run:  node scripts/gen-icon.mjs   ->  assets/icon.png
//       npx tauri icon assets/icon.png   (regenerates src-tauri/icons/*)
import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT = resolve(__dirname, "../assets/icon.png");

const S = 1024;
const buf = new Uint8Array(S * S * 4);

// ---------- helpers ----------
const put = (x, y, r, g, b, a) => {
  if (x < 0 || y < 0 || x >= S || y >= S || a <= 0) return;
  const i = (y * S + x) * 4;
  const sa = Math.min(1, a / 255);
  const da = buf[i + 3] / 255;
  const oa = sa + da * (1 - sa);
  if (oa <= 0) return;
  buf[i] = Math.round((r * sa + buf[i] * da * (1 - sa)) / oa);
  buf[i + 1] = Math.round((g * sa + buf[i + 1] * da * (1 - sa)) / oa);
  buf[i + 2] = Math.round((b * sa + buf[i + 2] * da * (1 - sa)) / oa);
  buf[i + 3] = Math.round(oa * 255);
};

const sdRoundRect = (x, y, cx, cy, w, h, r) => {
  const dx = Math.abs(x - cx) - (w / 2 - r);
  const dy = Math.abs(y - cy) - (h / 2 - r);
  const ox = Math.max(dx, 0);
  const oy = Math.max(dy, 0);
  return Math.min(Math.max(dx, dy), 0) + Math.hypot(ox, oy) - r;
};

/** coverage from a signed distance; `soft` widens the falloff (blur-ish). */
const cov = (d, soft = 1.4) => Math.min(1, Math.max(0, 0.5 - d / soft));

const roundRect = (cx, cy, w, h, r, [cr, cg, cb], alpha = 255, soft = 1.4) => {
  const x0 = Math.max(0, Math.floor(cx - w / 2 - soft));
  const x1 = Math.min(S, Math.ceil(cx + w / 2 + soft));
  const y0 = Math.max(0, Math.floor(cy - h / 2 - soft));
  const y1 = Math.min(S, Math.ceil(cy + h / 2 + soft));
  for (let y = y0; y < y1; y++) {
    for (let x = x0; x < x1; x++) {
      const c = cov(sdRoundRect(x + 0.5, y + 0.5, cx, cy, w, h, r), soft);
      if (c > 0) put(x, y, cr, cg, cb, alpha * c);
    }
  }
};

const lerp = (a, b, t) => a + (b - a) * t;

// ---------- 1. squircle: indigo -> violet gradient + soft top-left highlight ----------
const R = 226;
const G_TOP = [91, 91, 245]; // #5B5BF5
const G_BOT = [139, 92, 246]; // #8B5CF6
for (let y = 0; y < S; y++) {
  for (let x = 0; x < S; x++) {
    const d = sdRoundRect(x + 0.5, y + 0.5, S / 2, S / 2, S, S, R);
    const c = cov(d);
    if (c <= 0) continue;
    const t = y / (S - 1);
    let r = lerp(G_TOP[0], G_BOT[0], t);
    let g = lerp(G_TOP[1], G_BOT[1], t);
    let b = lerp(G_TOP[2], G_BOT[2], t);
    // radial highlight from the upper-left, gives the flat square some depth
    const hx = (x - S * 0.3) / (S * 0.85);
    const hy = (y - S * 0.18) / (S * 0.85);
    const hl = Math.max(0, 1 - Math.hypot(hx, hy));
    const k = hl * hl * 46;
    r += k; g += k; b += k * 0.7;
    put(x, y, Math.min(255, Math.round(r)), Math.min(255, Math.round(g)), Math.min(255, Math.round(b)), Math.round(255 * c));
  }
}

// ---------- 2. soft drop shadow under the card ----------
for (let i = 0; i < 26; i++) {
  roundRect(512, 566 + i * 1.5, 436, 552, 78, [49, 46, 129], 5, 46);
}

// ---------- 3. two cards behind (history stack) ----------
roundRect(512 + 30, 560 + 30, 420, 540, 74, [255, 255, 255], 78);
roundRect(512 + 15, 560 + 15, 424, 544, 76, [255, 255, 255], 132);

// ---------- 4. the front card ----------
roundRect(512, 560, 430, 548, 76, [255, 255, 255], 255);

// ---------- 5. clip on top of the card ----------
roundRect(512, 300, 252, 116, 52, [79, 70, 229], 255); // indigo-600
roundRect(512, 288, 236, 74, 40, [99, 102, 241], 255); // lighter top half

// ---------- 6. content bars: two neutral + one accent ----------
roundRect(512 - 20, 560, 300, 48, 24, [148, 163, 184], 255);
roundRect(512 - 20, 680, 300, 48, 24, [148, 163, 184], 255);
roundRect(512 - 72, 800, 196, 48, 24, [99, 102, 241], 255);

// ---------- PNG encode ----------
const raw = Buffer.alloc((S * 4 + 1) * S);
for (let y = 0; y < S; y++) {
  raw[y * (S * 4 + 1)] = 0;
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
ihdr[8] = 8;
ihdr[9] = 6;
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);
mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, png);
console.log(`wrote ${OUT} (${S}x${S}, ${(png.length / 1024).toFixed(1)} KiB)`);
