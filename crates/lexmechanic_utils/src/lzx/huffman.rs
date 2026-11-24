//! Canonical Huffman tree construction for the LZX compressor.
//!
//! Port of `build_huffman_tree` in `src/calibre/utils/lzx/lzxc.c`,
//! Copyright (C) 2002 Matthew T. Russotto.
//!
//! Microsoft's second condition on its canonical codes is that, working
//! upwards from the deepest level, leaf nodes must start as far *left*
//! as possible — which yields codes where the longest code is all ones,
//! the opposite of the more familiar canonical form. The code
//! assignment below walks the sorted leaves backwards for that reason.

/// One symbol's entry in a built tree. `huff_entry` in `lzxc.c`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct HuffEntry {
    /// Code length in bits; 0 means the symbol is unused.
    pub codelength: u16,
    /// The code itself, right-aligned in `codelength` bits.
    pub code: u16,
}

/// A leaf during construction. `h_elem` in `lzxc.c`.
#[derive(Clone, Copy)]
struct Leaf {
    freq: i64,
    sym: i32,
    pathlength: i32,
    parent: Option<usize>,
    code: u16,
}

/// An internal node. `ih_elem` in `lzxc.c`.
#[derive(Clone, Copy)]
struct Inode {
    freq: i64,
    pathlength: i32,
    parent: Option<usize>,
    left: NodeRef,
    right: NodeRef,
}

/// The C casts `h_elem*` to `ih_elem*` and tells them apart by `sym`;
/// here the two arenas stay separate and the tag does the work.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeRef {
    Leaf(usize),
    Inode(usize),
}

fn freq_of(r: NodeRef, leaves: &[Leaf], inodes: &[Inode]) -> i64 {
    match r {
        NodeRef::Leaf(i) => leaves[i].freq,
        NodeRef::Inode(i) => inodes[i].freq,
    }
}

fn path_of(r: NodeRef, leaves: &[Leaf], inodes: &[Inode]) -> i32 {
    match r {
        NodeRef::Leaf(i) => leaves[i].pathlength,
        NodeRef::Inode(i) => inodes[i].pathlength,
    }
}

/// `cmp_leaves` — zero frequencies last, then ascending frequency,
/// ties broken by ascending symbol.
fn cmp_leaves(a: &Leaf, b: &Leaf) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a.freq == 0, b.freq == 0) {
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    if a.freq == b.freq {
        return a.sym.cmp(&b.sym);
    }
    a.freq.cmp(&b.freq)
}

/// `cmp_pathlengths` — descending path length, ties broken by
/// *descending* symbol (see the note on canonical path lengths in the C).
fn cmp_pathlengths(a: &Leaf, b: &Leaf) -> std::cmp::Ordering {
    if a.pathlength == b.pathlength {
        return b.sym.cmp(&a.sym);
    }
    b.pathlength.cmp(&a.pathlength)
}

/// `build_huffman_tree` — build a canonical Huffman code from symbol
/// frequencies, with no code longer than `max_code_length` bits.
///
/// When the natural code would be too long, the C halves every
/// frequency greater than one and starts over; this does the same.
pub fn build_huffman_tree(nelem: usize, max_code_length: i32, freq: &[i32]) -> Vec<HuffEntry> {
    let mut leaves: Vec<Leaf> = (0..nelem)
        .map(|i| Leaf {
            freq: i64::from(freq[i]),
            sym: i as i32,
            pathlength: 0,
            parent: None,
            code: 0,
        })
        .collect();
    leaves.sort_by(cmp_leaves);

    let mut nleaves = leaves.iter().position(|l| l.freq == 0).unwrap_or(nelem);

    let mut tree = vec![HuffEntry::default(); nelem];

    if nleaves >= 2 {
        let mut inodes: Vec<Inode> = Vec::with_capacity(nelem.saturating_sub(1));
        let mut codes_too_long = false;
        loop {
            if codes_too_long {
                // Halve every frequency above one and try again.
                for leaf in leaves.iter_mut() {
                    if leaf.freq == 0 {
                        break;
                    }
                    if leaf.freq != 1 {
                        leaf.freq >>= 1;
                        codes_too_long = false;
                    }
                }
                debug_assert!(!codes_too_long, "no frequency could be reduced further");
            }

            inodes.clear();
            for leaf in leaves.iter_mut() {
                leaf.parent = None;
            }
            let mut leaves_left = nleaves;
            let mut cur_leaf = 0usize;
            // `cur_inode` trails `next_inode`: the queue of internal
            // nodes already built but not yet consumed.
            let mut cur_inode = 0usize;

            loop {
                let mut picked: [Option<NodeRef>; 2] = [None, None];
                for slot in picked.iter_mut() {
                    let leaf_first = leaves_left > 0
                        && (cur_inode == inodes.len()
                            || leaves[cur_leaf].freq <= inodes[cur_inode].freq);
                    if leaf_first {
                        *slot = Some(NodeRef::Leaf(cur_leaf));
                        cur_leaf += 1;
                        leaves_left -= 1;
                    } else if cur_inode != inodes.len() {
                        *slot = Some(NodeRef::Inode(cur_inode));
                        cur_inode += 1;
                    }
                }

                let (f1, f2) = match (picked[0], picked[1]) {
                    (Some(a), Some(b)) => (a, b),
                    _ => break,
                };

                let new_index = inodes.len();
                let pathlength =
                    path_of(f1, &leaves, &inodes).max(path_of(f2, &leaves, &inodes)) + 1;
                let node = Inode {
                    freq: freq_of(f1, &leaves, &inodes) + freq_of(f2, &leaves, &inodes),
                    pathlength,
                    parent: None,
                    left: f1,
                    right: f2,
                };
                for child in [f1, f2] {
                    match child {
                        NodeRef::Leaf(i) => leaves[i].parent = Some(new_index),
                        NodeRef::Inode(i) => inodes[i].parent = Some(new_index),
                    }
                }
                inodes.push(node);
                if pathlength > max_code_length {
                    codes_too_long = true;
                    break;
                }
            }

            if !codes_too_long {
                break;
            }
        }

        assign_pathlengths(&mut leaves, &mut inodes);

        // The path lengths are already in order, so this sorts by symbol.
        leaves.sort_by(cmp_pathlengths);

        // Longest code is all ones: walk from the shallowest leaf back.
        let mut pathlength = leaves[nleaves - 1].pathlength;
        let mut cur_code: u16 = 0;
        for i in (0..nleaves).rev() {
            while leaves[i].pathlength > pathlength {
                cur_code <<= 1;
                pathlength += 1;
            }
            leaves[i].code = cur_code;
            cur_code = cur_code.wrapping_add(1);
        }
    } else if nleaves == 1 {
        // Zero symbols is fine, but a lone symbol still needs two codes.
        nleaves = 2;
        leaves[0].pathlength = 1;
        leaves[1].pathlength = 1;
        if leaves[1].sym > leaves[0].sym {
            leaves[1].code = 1;
            leaves[0].code = 0;
        } else {
            leaves[0].code = 1;
            leaves[1].code = 0;
        }
    }

    for leaf in leaves.iter().take(nleaves) {
        tree[leaf.sym as usize] = HuffEntry {
            codelength: leaf.pathlength as u16,
            code: leaf.code,
        };
    }
    tree
}

/// Depth-first traversal that records each leaf's depth, mirroring the
/// pointer-chasing walk in `build_huffman_tree`.
fn assign_pathlengths(leaves: &mut [Leaf], inodes: &mut [Inode]) {
    // The last node built is the root.
    let root = inodes.len() - 1;
    let mut cur = Some(NodeRef::Inode(root));
    let mut pathlength = 0i32;
    inodes[root].pathlength = -1;

    while let Some(node) = cur {
        match node {
            NodeRef::Inode(i) => {
                // An unmarked internal node: descend left.
                let left = inodes[i].left;
                match left {
                    NodeRef::Leaf(j) => leaves[j].pathlength = -1,
                    NodeRef::Inode(j) => inodes[j].pathlength = -1,
                }
                cur = Some(left);
                pathlength += 1;
            }
            NodeRef::Leaf(i) => {
                leaves[i].pathlength = pathlength;
                // Climb until an unmarked node is reached, or the tree
                // is exhausted.
                let mut up = leaves[i].parent;
                loop {
                    pathlength -= 1;
                    match up {
                        None => break,
                        Some(p) => {
                            if inodes[p].pathlength == -1 {
                                break;
                            }
                            up = inodes[p].parent;
                        }
                    }
                }
                match up {
                    None => cur = None,
                    Some(p) => {
                        inodes[p].pathlength = pathlength;
                        let right = inodes[p].right;
                        match right {
                            NodeRef::Leaf(j) => leaves[j].pathlength = -1,
                            NodeRef::Inode(j) => inodes[j].pathlength = -1,
                        }
                        cur = Some(right);
                        pathlength += 1;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kraft's equality: a complete prefix code satisfies
    /// `sum(2^-len) == 1`.
    fn kraft_sum(tree: &[HuffEntry]) -> f64 {
        tree.iter()
            .filter(|e| e.codelength > 0)
            .map(|e| 2f64.powi(-i32::from(e.codelength)))
            .sum()
    }

    fn is_prefix_free(tree: &[HuffEntry]) -> bool {
        let used: Vec<_> = tree.iter().filter(|e| e.codelength > 0).collect();
        for (i, a) in used.iter().enumerate() {
            for b in used.iter().skip(i + 1) {
                let (short, long) = if a.codelength <= b.codelength {
                    (a, b)
                } else {
                    (b, a)
                };
                let shift = long.codelength - short.codelength;
                if long.code >> shift == short.code {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn no_symbols_yields_an_empty_tree() {
        let tree = build_huffman_tree(8, 16, &[0; 8]);
        assert!(tree.iter().all(|e| e.codelength == 0));
    }

    #[test]
    fn a_lone_symbol_still_gets_two_one_bit_codes() {
        let mut freq = [0i32; 8];
        freq[5] = 42;
        let tree = build_huffman_tree(8, 16, &freq);
        let used: Vec<_> = tree
            .iter()
            .enumerate()
            .filter(|(_, e)| e.codelength > 0)
            .collect();
        assert_eq!(used.len(), 2, "a single symbol requires a partner");
        assert!(used.iter().all(|(_, e)| e.codelength == 1));
        assert!(used.iter().any(|(i, _)| *i == 5));
    }

    #[test]
    fn balanced_frequencies_give_a_complete_code() {
        let freq = [1i32; 8];
        let tree = build_huffman_tree(8, 16, &freq);
        assert!(tree.iter().all(|e| e.codelength == 3));
        assert!((kraft_sum(&tree) - 1.0).abs() < 1e-9);
        assert!(is_prefix_free(&tree));
    }

    #[test]
    fn skewed_frequencies_give_shorter_codes_to_common_symbols() {
        let freq = [100i32, 50, 25, 12, 6, 3, 2, 1];
        let tree = build_huffman_tree(8, 16, &freq);
        assert!(tree[0].codelength < tree[7].codelength);
        assert!((kraft_sum(&tree) - 1.0).abs() < 1e-9);
        assert!(is_prefix_free(&tree));
    }

    #[test]
    fn code_lengths_are_capped_by_halving_frequencies() {
        // Fibonacci frequencies are the worst case for code depth.
        let mut freq = vec![0i32; 40];
        freq[0] = 1;
        freq[1] = 1;
        for i in 2..40 {
            freq[i] = freq[i - 1].saturating_add(freq[i - 2]);
        }
        for cap in [16i32, 12, 8] {
            let tree = build_huffman_tree(40, cap, &freq);
            let longest = tree.iter().map(|e| e.codelength).max().unwrap_or(0);
            assert!(i32::from(longest) <= cap, "cap {cap} exceeded by {longest}");
            assert!(is_prefix_free(&tree));
        }
    }

    #[test]
    fn the_longest_code_is_all_ones() {
        // Microsoft's canonical form, as opposed to the usual one where
        // the longest code is all zeros.
        let freq = [1i32, 1, 2, 4, 8, 16, 32, 64];
        let tree = build_huffman_tree(8, 16, &freq);
        let longest = tree
            .iter()
            .filter(|e| e.codelength > 0)
            .max_by_key(|e| e.codelength)
            .copied()
            .unwrap();
        let all_ones = (1u16 << longest.codelength) - 1;
        assert_eq!(longest.code, all_ones);
    }

    #[test]
    fn a_realistic_alphabet_stays_prefix_free() {
        let mut freq = vec![0i32; 256];
        for (i, f) in freq.iter_mut().enumerate() {
            *f = ((i * 7919) % 97) as i32;
        }
        let tree = build_huffman_tree(256, 16, &freq);
        assert!(is_prefix_free(&tree));
        assert!((kraft_sum(&tree) - 1.0).abs() < 1e-9);
    }
}
