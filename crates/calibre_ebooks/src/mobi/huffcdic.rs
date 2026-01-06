use anyhow::{bail, Result};
use byteorder::{BigEndian, ByteOrder, ReadBytesExt};
use std::io::Cursor;

pub struct HuffReader {
    dict1: Vec<(u32, bool, u32)>, // (codelen, term, maxcode)
    mincode: Vec<u32>,
    maxcode: Vec<u32>,
    dictionary: Vec<Option<(Vec<u8>, bool)>>, // Option<(slice, flag)>
}

impl HuffReader {
    pub fn new(huffs: &[Vec<u8>]) -> Result<Self> {
        let mut reader = HuffReader {
            dict1: Vec::new(),
            mincode: Vec::new(),
            maxcode: Vec::new(),
            dictionary: Vec::new(),
        };

        if huffs.is_empty() {
            bail!("No HUFF records provided");
        }

        reader.load_huff(&huffs[0])?;
        for cdic in &huffs[1..] {
            reader.load_cdic(cdic)?;
        }

        Ok(reader)
    }

    fn load_huff(&mut self, huff: &[u8]) -> Result<()> {
        if huff.len() < 24 || &huff[0..4] != b"HUFF" {
            bail!("Invalid HUFF header");
        }

        let off1 = BigEndian::read_u32(&huff[8..12]) as usize;
        let off2 = BigEndian::read_u32(&huff[12..16]) as usize;

        // Parse Dict1
        // dict1 is 256 u32s at off1
        let mut dict1_vals = Vec::new();
        let mut curs = Cursor::new(&huff[off1..]);
        for _ in 0..256 {
            if curs.position() as usize + 4 > huff.len() {
                break;
            }
            dict1_vals.push(curs.read_u32::<BigEndian>()?);
        }

        self.dict1 = dict1_vals
            .into_iter()
            .map(|v| {
                let codelen = v & 0x1f;
                let term = (v & 0x80) != 0;
                let maxcode_shifted = v >> 8;
                // assert codelen != 0
                // maxcode = ((maxcode + 1) << (32 - codelen)) - 1
                let maxcode = if codelen > 0 {
                    ((maxcode_shifted + 1) << (32 - codelen)).wrapping_sub(1)
                } else {
                    0
                };
                (codelen, term, maxcode)
            })
            .collect();

        // Parse Dict2
        // dict2 is 64 u32s at off2
        let mut dict2_vals = Vec::new();
        let mut curs = Cursor::new(&huff[off2..]);
        for _ in 0..64 {
            if curs.position() as usize + 4 > huff.len() {
                break;
            }
            dict2_vals.push(curs.read_u32::<BigEndian>()?);
        }

        self.mincode = Vec::new();
        self.maxcode = Vec::new();

        // Python:
        // for codelen, mincode in enumerate((0,) + dict2[0::2]):
        //    self.mincode += (mincode << (32 - codelen), )

        // Tuple insert 0 at start.
        // Elements at 0, 2, 4... (Even indices of dict2, which acts as the 'mincode' part)
        // Note: dict2 in python is [min1, max1, min2, max2 ...]

        // Element 0 (codelen 0) -> 0
        self.mincode.push(0); // For codelen 0
        for (i, &val) in dict2_vals.iter().step_by(2).enumerate() {
            let codelen = (i + 1) as u32; // starts at 1
            self.mincode.push(val << (32 - codelen));
        }

        // for codelen, maxcode in enumerate((0,) + dict2[1::2]):
        //     self.maxcode += (((maxcode + 1) << (32 - codelen)) - 1, )

        self.maxcode.push(0); // For codelen 0
        for (i, &val) in dict2_vals.iter().skip(1).step_by(2).enumerate() {
            let codelen = (i + 1) as u32;
            let m = ((val + 1) << (32 - codelen)).wrapping_sub(1);
            self.maxcode.push(m);
        }

        self.dictionary = Vec::new();
        Ok(())
    }

    fn load_cdic(&mut self, cdic: &[u8]) -> Result<()> {
        if cdic.len() < 16 || &cdic[0..4] != b"CDIC" {
            bail!("Invalid CDIC header");
        }

        let phrases = BigEndian::read_u32(&cdic[8..12]) as usize;
        let bits = BigEndian::read_u32(&cdic[12..16]) as u32;

        let n = std::cmp::min(1 << bits, phrases - self.dictionary.len());

        let mut curs = Cursor::new(&cdic[16..]);
        for _ in 0..n {
            if curs.position() as usize + 2 > cdic.len() {
                break;
            }
            let offset = curs.read_u16::<BigEndian>()? as usize;

            // getslice
            let slice_start = 16 + offset;
            if slice_start + 2 > cdic.len() {
                // Out of bounds
                self.dictionary.push(None); // Placeholder?
                continue;
            }

            let blen = BigEndian::read_u16(&cdic[slice_start..slice_start + 2]);
            let real_len = (blen & 0x7fff) as usize;
            let flag = (blen & 0x8000) != 0;

            let data_start = slice_start + 2; // 16 + off + 2
            if data_start + real_len > cdic.len() {
                // Out of bounds
                self.dictionary.push(None);
                continue;
            }

            let slice = cdic[data_start..data_start + real_len].to_vec();
            self.dictionary.push(Some((slice, flag)));
        }

        Ok(())
    }

    pub fn unpack(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        let mut bitsleft = (data.len() * 8) as isize;
        let mut padded = data.to_vec();
        padded.extend_from_slice(&[0u8; 8]);

        let mut pos = 0;
        let mut x = BigEndian::read_u64(&padded[0..8]);
        let mut n = 32;

        let mut output = Vec::new();

        loop {
            if n <= 0 {
                pos += 4;
                if pos + 8 > padded.len() {
                    break;
                }
                x = BigEndian::read_u64(&padded[pos..pos + 8]);
                n += 32;
            }

            let code = (x >> n) & ((1u64 << 32) - 1);
            let code_u32 = code as u32;

            let idx = (code_u32 >> 24) as usize;
            if idx >= self.dict1.len() {
                break;
            }

            let (mut codelen, term, mut maxcode) = self.dict1[idx];

            if !term {
                while (codelen as usize) < self.mincode.len()
                    && code_u32 < self.mincode[codelen as usize]
                {
                    codelen += 1;
                }
                if (codelen as usize) < self.maxcode.len() {
                    maxcode = self.maxcode[codelen as usize];
                }
            }

            n -= codelen as i32;
            bitsleft -= codelen as isize;

            if bitsleft < 0 {
                break;
            }

            let r = (maxcode.wrapping_sub(code_u32)) >> (32 - codelen);

            if (r as usize) < self.dictionary.len() {
                let maybe_entry = self.dictionary[r as usize].clone();
                if let Some((slice, flag)) = maybe_entry {
                    if !flag {
                        // Recursive unpack
                        let unpacked_slice = self.unpack(&slice)?;
                        // Update cache
                        self.dictionary[r as usize] = Some((unpacked_slice.clone(), true));
                        output.extend_from_slice(&unpacked_slice);
                    } else {
                        output.extend_from_slice(&slice);
                    }
                }
            }
        }

        Ok(output)
    }
}
