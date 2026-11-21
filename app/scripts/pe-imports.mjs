// A minimal PE import-table reader: enough to list the DLLs an .exe needs
// at load time, so the installer can bundle the ones Windows will not supply.
//
// Reads the regular import directory (data directory 1: hard imports that
// stop the process starting if missing) and the delay-load directory
// (data directory 13: loaded on first call, so only one feature breaks).

/**
 * @param {Buffer} buf
 * @returns {{imports: string[], delayImports: string[]}} DLL names as written in the file
 */
export function readPeImports(buf) {
  if (buf.length < 0x40 || buf.readUInt16LE(0) !== 0x5a4d) throw new Error("not a PE file (no MZ header)");
  const peOff = buf.readUInt32LE(0x3c);
  if (buf.toString("latin1", peOff, peOff + 4) !== "PE\0\0") throw new Error("not a PE file (no PE signature)");

  const numSections = buf.readUInt16LE(peOff + 6);
  const optSize = buf.readUInt16LE(peOff + 20);
  const opt = peOff + 24;
  const magic = buf.readUInt16LE(opt);
  const pe32plus = magic === 0x20b;
  if (!pe32plus && magic !== 0x10b) throw new Error(`unknown PE optional header magic 0x${magic.toString(16)}`);
  const dirBase = opt + (pe32plus ? 112 : 96);
  const numDirs = buf.readUInt32LE(dirBase - 4);

  const sectionTable = opt + optSize;
  const sections = [];
  for (let i = 0; i < numSections; i++) {
    const s = sectionTable + i * 40;
    sections.push({
      va: buf.readUInt32LE(s + 12),
      vsize: buf.readUInt32LE(s + 8),
      rawSize: buf.readUInt32LE(s + 16),
      raw: buf.readUInt32LE(s + 20),
    });
  }
  const rvaToOff = (rva) => {
    for (const s of sections) {
      if (rva >= s.va && rva < s.va + Math.max(s.vsize, s.rawSize)) return rva - s.va + s.raw;
    }
    throw new Error(`RVA 0x${rva.toString(16)} is outside every section`);
  };
  const cstr = (off) => buf.toString("latin1", off, buf.indexOf(0, off));
  const dir = (n) => (n < numDirs ? buf.readUInt32LE(dirBase + n * 8) : 0);

  const imports = [];
  const impRva = dir(1);
  if (impRva) {
    // IMAGE_IMPORT_DESCRIPTOR is 20 bytes, Name RVA at +12; zero-terminated.
    for (let off = rvaToOff(impRva); ; off += 20) {
      const nameRva = buf.readUInt32LE(off + 12);
      if (nameRva === 0) break;
      imports.push(cstr(rvaToOff(nameRva)));
    }
  }
  const delayImports = [];
  const delRva = dir(13);
  if (delRva) {
    // IMAGE_DELAYLOAD_DESCRIPTOR is 32 bytes, DllNameRVA at +4; zero-terminated.
    for (let off = rvaToOff(delRva); ; off += 32) {
      const nameRva = buf.readUInt32LE(off + 4);
      if (nameRva === 0) break;
      delayImports.push(cstr(rvaToOff(nameRva)));
    }
  }
  return { imports, delayImports };
}
