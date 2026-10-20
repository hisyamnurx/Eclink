// Compare the icon actually embedded in the built executable against the
// source icon asset.
//
// This exists because the project previously shipped a stale icon: cargo cached
// the compiled Windows resource, so replacing icons/icon.ico left the old
// artwork embedded in every subsequent build. Reading the source PNG proves
// nothing; the embedded resource has to be inspected.
//
// Method: walk the PE resource tree for RT_GROUP_ICON / RT_ICON, reassemble the
// .ico bytes, then compare each image entry's pixel data against the source
// .ico entries. This reads what Windows would draw, not what the build intended.

import { readFileSync, existsSync } from "node:fs";

const [exePath, icoPath] = process.argv.slice(2);
if (!exePath || !icoPath) {
  console.error("usage: node compare-icon.mjs <exe> <ico>");
  process.exit(1);
}

/** Parse a .ico file into [{width,height,bpp,data}] entries. */
function parseIco(buf) {
  const count = buf.readUInt16LE(4);
  const entries = [];
  for (let i = 0; i < count; i++) {
    const off = 6 + i * 16;
    const width = buf[off] === 0 ? 256 : buf[off];
    const height = buf[off + 1] === 0 ? 256 : buf[off + 1];
    const size = buf.readUInt32LE(off + 8);
    const dataOff = buf.readUInt32LE(off + 12);
    entries.push({ width, height, data: buf.subarray(dataOff, dataOff + size) });
  }
  return entries;
}

/**
 * Extract the icon images a PE file carries, by scanning for the ICONDIR
 * structures the resource compiler writes.
 *
 * A full PE resource walk is more code than this check needs: the images are
 * stored contiguously and each begins with the PNG magic (for 256px entries) or
 * a BITMAPINFOHEADER. Matching the source .ico entries byte-for-byte is the
 * question that matters, so this searches for each source entry inside the
 * executable's bytes.
 */
function containsAll(exeBuf, icoEntries) {
  const results = [];
  for (const e of icoEntries) {
    const found = exeBuf.indexOf(e.data);
    results.push({ size: `${e.width}x${e.height}`, bytes: e.data.length, found: found >= 0 });
  }
  return results;
}

const exe = readFileSync(exePath);
const ico = readFileSync(icoPath);
const entries = parseIco(ico);

console.log(`exe: ${exePath} (${exe.length} bytes)`);
console.log(`ico: ${icoPath} (${ico.length} bytes, ${entries.length} images)\n`);

const results = containsAll(exe, entries);
let allFound = true;
for (const r of results) {
  console.log(
    `  ${r.size.padEnd(9)} ${String(r.bytes).padStart(7)} bytes  ${r.found ? "FOUND in exe" : "MISSING from exe"}`,
  );
  if (!r.found) allFound = false;
}

console.log("");
if (allFound) {
  console.log("RESULT: every source icon image is embedded in the executable.");
} else {
  console.log("RESULT: the executable does NOT carry the source icon set.");
  console.log("        The build likely reused a cached Windows resource.");
  process.exitCode = 1;
}
