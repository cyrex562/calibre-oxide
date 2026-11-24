//! Trailing Byte Sequences (TBS): per-text-record metadata describing
//! which `NCX` index entries start, end, span, or are wholly contained
//! within that record, arranged into hierarchical "strands" and encoded
//! as a compact byte sequence appended to each text record's trailing
//! data.
//!
//! Port of `calibre.ebooks.mobi.writer8.tbs`. [`crate::mobi::utils::encode_tbs`]/
//! [`crate::mobi::utils::encode_trailing_data`] (ported for issue #33/#34)
//! are this module's low-level primitives; everything here -- strand
//! separation, sequence encoding, the two-pass negative-index retry --
//! is new work specific to this issue.
//!
//! See `DOC` in `tbs.py` for the conceptual overview this module doc
//! summarizes.

use std::collections::HashMap;

use anyhow::Result;

use crate::mobi::utils::{encode_tbs, encode_trailing_data};
use crate::mobi::writer8::index::NcxTableEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Starts,
    Ends,
    Spans,
    Completes,
}

/// Port of the `Entry` namedtuple in `tbs.py`, after `fill_entry` has
/// computed its per-record `action`. Python's namedtuple also carries
/// `start_offset`/`length_offset`/`title`/`text_record_length`, but
/// those are only ever read by `fill_entry` itself (to compute `action`)
/// and by the disabled `elif False and (...)` heuristic branch in
/// `encode_strands_as_sequences` (intentionally not ported -- see that
/// function's doc) -- so they aren't stored here.
#[derive(Debug, Clone)]
struct FilledEntry {
    index: u64,
    depth: u64,
    parent: Option<u64>,
    action: Action,
}

/// Port of `fill_entry`.
fn fill_entry(entry: &NcxTableEntry, start_offset: i64, text_record_length: i64) -> FilledEntry {
    let length_offset = start_offset + entry.length as i64;
    let action = if start_offset < 0 {
        if length_offset > text_record_length {
            Action::Spans
        } else {
            Action::Ends
        }
    } else if length_offset > text_record_length {
        Action::Starts
    } else {
        Action::Completes
    };
    FilledEntry {
        index: entry.index,
        depth: entry.depth,
        parent: entry.parent,
        action,
    }
}

/// Port of `populate_strand`: consumes matching entries out of `entries`
/// (mirroring Python's in-place `list.remove`), returning `parent`
/// followed by its single child chain (if any) or its contiguous
/// same-depth siblings.
fn populate_strand(parent: FilledEntry, entries: &mut Vec<FilledEntry>) -> Vec<FilledEntry> {
    let parent_index = parent.index;
    let parent_depth = parent.depth;
    let parent_parent = parent.parent;
    let mut ans = vec![parent];

    if let Some(pos) = entries.iter().position(|c| c.parent == Some(parent_index)) {
        let child = entries.remove(pos);
        ans.extend(populate_strand(child, entries));
        return ans;
    }

    let mut current_index = parent_index;
    let mut siblings = Vec::new();
    loop {
        let found = entries.iter().position(|e| {
            e.depth == parent_depth && e.parent == parent_parent && e.index == current_index + 1
        });
        let Some(pos) = found else { break };
        let entry = entries.remove(pos);
        current_index = entry.index;
        let has_children = entries.iter().any(|c| c.parent == Some(entry.index));
        if has_children {
            siblings.extend(populate_strand(entry, entries));
            break;
        }
        siblings.push(entry);
    }
    ans.extend(siblings);
    ans
}

/// One strand: an ordered list of `(depth, entries at that depth)`
/// layers. Port of the `OrderedDict` built in `separate_strands`.
type Layers = Vec<(u64, Vec<FilledEntry>)>;

/// Port of `separate_strands`.
fn separate_strands(mut entries: Vec<FilledEntry>) -> Vec<Layers> {
    let mut ans = Vec::new();
    while !entries.is_empty() {
        let top = entries.remove(0);
        let strand = populate_strand(top, &mut entries);
        let mut layers: Layers = Vec::new();
        for entry in strand {
            match layers.iter_mut().find(|(d, _)| *d == entry.depth) {
                Some((_, v)) => v.push(entry),
                None => layers.push((entry.depth, vec![entry])),
            }
        }
        ans.push(layers);
    }
    ans
}

/// Port of `collect_indexing_data`.
fn collect_indexing_data(
    entries: &[NcxTableEntry],
    text_record_lengths: &[usize],
) -> Vec<Vec<Layers>> {
    let mut sorted: Vec<&NcxTableEntry> = entries.iter().collect();
    sorted.sort_by_key(|e| e.offset);

    let mut data = Vec::with_capacity(text_record_lengths.len());
    let mut record_start: i64 = 0;
    for &rec_length in text_record_lengths {
        let next_record_start = record_start + rec_length as i64;
        let mut local_entries = Vec::new();
        for entry in &sorted {
            if entry.offset as i64 >= next_record_start {
                break;
            }
            if entry.offset as i64 + entry.length as i64 <= record_start {
                continue;
            }
            let start_offset = entry.offset as i64 - record_start;
            local_entries.push(fill_entry(entry, start_offset, rec_length as i64));
        }
        data.push(separate_strands(local_entries));
        record_start = next_record_start;
    }
    data
}

/// Raised (as `Err`) when a strand's index delta comes out negative
/// under `tbs_type == 8`; the caller retries the whole record with
/// `tbs_type == 5`, which flips the sign instead of failing. Port of
/// `NegativeStrandIndex`.
struct NegativeStrandIndex;

/// A sequence's `(index_delta, extra_flag_values)` pair, ready for
/// [`sequences_to_bytes`]/`encode_tbs`.
type Sequence = (i64, HashMap<u32, u64>);

/// Port of `encode_strands_as_sequences`.
fn encode_strands_as_sequences(
    strands: &[Layers],
    tbs_type: u32,
) -> std::result::Result<Vec<Sequence>, NegativeStrandIndex> {
    let mut first_entry_index: Option<u64> = None;
    for strand in strands {
        for (_, entries) in strand {
            for entry in entries {
                if first_entry_index.is_none() {
                    first_entry_index = Some(entry.index);
                }
            }
        }
    }

    let mut ans: Vec<Sequence> = Vec::new();
    let mut last_index: Option<u64> = None;

    for strand in strands {
        let mut strand_seqs: Vec<Sequence> = Vec::new();
        for (_, entries) in strand {
            let mut extra: HashMap<u32, u64> = HashMap::new();
            if let Some(last) = entries.last() {
                if last.action == Action::Spans {
                    extra.insert(0b1, 0);
                }
            }
            // Python's `elif False and (...)`: dead code (a heuristic
            // the original author could never pin down when kindlegen
            // applies), intentionally not ported.

            if let Some(first) = entries.first() {
                if Some(first.index) == first_entry_index {
                    extra.insert(0b10, tbs_type as u64);
                }
            }
            if entries.len() > 1 {
                extra.insert(0b100, entries.len() as u64);
            }

            let first = entries.first().expect("strand layer is never empty");
            let mut index: i64 = first.index as i64 - first.parent.unwrap_or(0) as i64;
            if !ans.is_empty() && strand_seqs.is_empty() {
                index = last_index.unwrap_or(0) as i64 - first.index as i64;
                if index < 0 {
                    if tbs_type == 5 {
                        index = -index;
                    } else {
                        return Err(NegativeStrandIndex);
                    }
                } else {
                    extra.insert(0b1000, 1);
                }
            }
            last_index = entries.last().map(|e| e.index);
            strand_seqs.push((index, extra));
        }

        // Consecutive `spans` entries: only the last one keeps the
        // `0b1 = 0` flag.
        for i in 0..strand_seqs.len().saturating_sub(1) {
            let (cur_has, next_has) = (
                strand_seqs[i].1.contains_key(&0b1),
                strand_seqs[i + 1].1.contains_key(&0b1),
            );
            if cur_has && next_has {
                strand_seqs[i].1.remove(&0b1);
            }
        }
        ans.extend(strand_seqs);
    }
    Ok(ans)
}

/// Port of `sequences_to_bytes`.
fn sequences_to_bytes(sequences: &[Sequence]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut flag_size = 3u32;
    for (val, extra) in sequences {
        // `val` is only negative if `encode_strands_as_sequences` failed
        // to normalize it (a bug, not adversarial input, since this
        // function is only ever called with its own output); clamp
        // rather than panic either way.
        let v = (*val).max(0) as u64;
        out.extend(encode_tbs(v, extra, flag_size));
        flag_size = 4; // only the first sequence has flag size 3
    }
    out
}

/// Port of `calculate_all_tbs`.
fn calculate_all_tbs(
    indexing_data: &[Vec<Layers>],
    tbs_type: u32,
) -> std::result::Result<HashMap<usize, Vec<u8>>, NegativeStrandIndex> {
    let mut rmap = HashMap::new();
    for (i, strands) in indexing_data.iter().enumerate() {
        let sequences = encode_strands_as_sequences(strands, tbs_type)?;
        rmap.insert(i + 1, sequences_to_bytes(&sequences));
    }
    Ok(rmap)
}

/// Compute and append trailing byte sequences to every text record.
/// Port of `apply_trailing_byte_sequences`. `records[i]` (`i` 1-based)
/// must be the already-compressed text record `i`; its TBS trailer is
/// appended in place via [`crate::mobi::utils::encode_trailing_data`].
/// Always returns `Ok(true)` on success (matching Python's `return
/// True`; kept as a `Result`/`bool` pair rather than simplified to `()`
/// so `KF8Writer::create_indices` can set `self.has_tbs` exactly like
/// `main.py` does).
pub fn apply_trailing_byte_sequences(
    index_table: &[NcxTableEntry],
    records: &mut [Vec<u8>],
    text_record_lengths: &[usize],
) -> Result<bool> {
    let indexing_data = collect_indexing_data(index_table, text_record_lengths);
    let rmap = match calculate_all_tbs(&indexing_data, 8) {
        Ok(m) => m,
        Err(NegativeStrandIndex) => calculate_all_tbs(&indexing_data, 5)
            .unwrap_or_else(|NegativeStrandIndex| HashMap::new()),
    };

    for (i, tbs_bytes) in rmap {
        if let Some(rec) = records.get_mut(i) {
            rec.extend(encode_trailing_data(&tbs_bytes));
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        index: u64,
        offset: u64,
        length: u64,
        depth: u64,
        parent: Option<u64>,
    ) -> NcxTableEntry {
        NcxTableEntry {
            index,
            offset,
            length,
            depth,
            parent,
            ..Default::default()
        }
    }

    #[test]
    fn a_single_completed_entry_produces_a_nonempty_tbs() {
        let entries = vec![entry(0, 0, 100, 0, None)];
        let mut records = vec![Vec::new(), vec![0u8; 10]];
        let ok = apply_trailing_byte_sequences(&entries, &mut records, &[0x1000]).unwrap();
        assert!(ok);
        assert!(
            records[1].len() > 10,
            "TBS trailer should have been appended"
        );
    }

    #[test]
    fn a_hierarchical_toc_produces_tbs_for_every_touched_record() {
        let entries = vec![
            entry(0, 0, 0x2000, 0, None),
            entry(1, 0, 0x1000, 1, Some(0)),
            entry(2, 0x1000, 0x1000, 1, Some(0)),
        ];
        let mut records = vec![Vec::new(), vec![1u8; 4], vec![2u8; 4]];
        let ok = apply_trailing_byte_sequences(&entries, &mut records, &[0x1000, 0x1000]).unwrap();
        assert!(ok);
        assert!(records[1].len() > 4);
        assert!(records[2].len() > 4);
    }

    #[test]
    fn empty_index_table_still_appends_an_empty_trailing_marker() {
        // Matches Python: `apply_trailing_byte_sequences` always writes
        // one `encode_trailing_data` marker per record it's given,
        // even a "no TBS data at all" one -- callers with no ToC at
        // all skip calling this function in the first place (see
        // `main.rs::KF8Writer::write`'s `oeb.toc.count() >= 1` guard).
        let mut records = vec![Vec::new(), vec![9u8; 3]];
        apply_trailing_byte_sequences(&[], &mut records, &[0x1000]).unwrap();
        assert_eq!(records[1], vec![9u8, 9, 9, 0x81]);
    }
}
