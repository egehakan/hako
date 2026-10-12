//! Word-level changes between the removed and the added lines of one diff hunk.
//!
//! Each removed line is paired with the added line it most plausibly became, and the two are
//! compared token by token. Lines that have no such counterpart, or too little in common with
//! it, get no word-level changes: marking nearly all of a line says nothing its line color does
//! not already say.

use std::cmp::Ordering;
use std::ops::Range;

use similar::{Algorithm, DiffOp, capture_diff_slices};
use unicode_segmentation::UnicodeSegmentation;

/// Longest line, in bytes, that is compared word by word. Longer ones are typically minified or
/// generated, slow to compare and unreadable when marked.
pub(super) const MAX_LINE_LEN: usize = 2_000;

/// Largest hunk, as removed lines times added lines, whose lines are paired.
pub(super) const MAX_PAIRING_CELLS: usize = 40_000;

/// Least share of two lines' visible text that has to be unchanged for one to count as an edit
/// of the other.
pub(super) const MIN_LINE_SIMILARITY: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenKind {
    /// A run of letters, digits and underscores.
    Word,
    /// A run of whitespace.
    Blank,
    /// A single character that is a word of its own: punctuation, a symbol, an ideograph.
    Single,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Token<'a> {
    pub text: &'a str,
    /// Position in the line, in characters.
    pub chars: Range<usize>,
    kind: TokenKind,
}

impl Token<'_> {
    fn is_blank(&self) -> bool {
        self.kind == TokenKind::Blank
    }

    /// How much the token counts towards the similarity of two lines.
    fn weight(&self) -> usize {
        match self.kind {
            TokenKind::Word | TokenKind::Single => self.chars.len(),
            TokenKind::Blank => 0,
        }
    }
}

/// Scripts written without spaces between words, where each character is compared on its own.
fn is_ideograph(character: char) -> bool {
    matches!(
        u32::from(character),
        0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3FFFF
    )
}

fn token_kind(grapheme: &str) -> TokenKind {
    match grapheme.chars().next() {
        Some(first) if first.is_whitespace() => TokenKind::Blank,
        Some(first) if is_ideograph(first) => TokenKind::Single,
        Some(first) if first.is_alphanumeric() || first == '_' => TokenKind::Word,
        Some(_) | None => TokenKind::Single,
    }
}

/// Splits a line into words, runs of whitespace and single punctuation characters. A character
/// and the marks combined with it are never split.
pub(super) fn tokenize(line: &str) -> Vec<Token<'_>> {
    let mut tokens: Vec<Token<'_>> = Vec::new();
    let mut token_start = 0;
    let mut char_offset = 0;
    for (byte_offset, grapheme) in line.grapheme_indices(true) {
        let kind = token_kind(grapheme);
        let char_count = grapheme.chars().count();
        let byte_end = byte_offset + grapheme.len();
        match tokens.last_mut() {
            Some(last) if last.kind == kind && kind != TokenKind::Single => {
                last.text = &line[token_start..byte_end];
                last.chars.end += char_count;
            }
            Some(_) | None => {
                token_start = byte_offset;
                tokens.push(Token {
                    text: grapheme,
                    chars: char_offset..char_offset + char_count,
                    kind,
                });
            }
        }
        char_offset += char_count;
    }
    tokens
}

/// A line prepared for comparison with the lines on the other side of its hunk.
struct LineTokens<'a> {
    tokens: Vec<Token<'a>>,
    /// The line's visible tokens with how much each counts, sorted by text.
    bag: Vec<(&'a str, usize)>,
    weight: usize,
}

impl<'a> LineTokens<'a> {
    fn new(line: &'a str) -> Self {
        let line = line.trim_end_matches(['\n', '\r']);
        if line.len() > MAX_LINE_LEN {
            return Self {
                tokens: Vec::new(),
                bag: Vec::new(),
                weight: 0,
            };
        }
        let tokens = tokenize(line);
        let mut bag: Vec<(&str, usize)> = tokens
            .iter()
            .filter(|token| !token.is_blank())
            .map(|token| (token.text, token.weight()))
            .collect();
        bag.sort_unstable();
        bag.dedup_by(|later, earlier| {
            if later.0 == earlier.0 {
                earlier.1 += later.1;
                true
            } else {
                false
            }
        });
        let weight = bag.iter().map(|(_, weight)| weight).sum();
        Self {
            tokens,
            bag,
            weight,
        }
    }

    /// Share of the two lines' visible text made of tokens they have in common, wherever those
    /// are in the line. Never less than the share that is unchanged between the two.
    fn common_share(&self, other: &Self) -> f32 {
        let total = self.weight + other.weight;
        if total == 0 {
            return 0.;
        }
        let mut common = 0;
        let (mut mine, mut theirs) = (0, 0);
        while mine < self.bag.len() && theirs < other.bag.len() {
            let (my_text, my_weight) = self.bag[mine];
            let (their_text, their_weight) = other.bag[theirs];
            match my_text.cmp(their_text) {
                Ordering::Less => mine += 1,
                Ordering::Greater => theirs += 1,
                Ordering::Equal => {
                    common += my_weight.min(their_weight);
                    mine += 1;
                    theirs += 1;
                }
            }
        }
        (2 * common) as f32 / total as f32
    }
}

/// Pairs removed lines with added lines, keeping both in order, so that the paired lines have as
/// much in common as possible. Lines with too little in common with any candidate stay unpaired.
fn pair_lines(old: &[LineTokens<'_>], new: &[LineTokens<'_>]) -> Vec<(usize, usize)> {
    let (old_len, new_len) = (old.len(), new.len());
    let cell = |old_index: usize, new_index: usize| old_index * (new_len + 1) + new_index;

    let mut shares = vec![0f32; old_len * new_len];
    // Best total share of the pairs among `old[old_index..]` and `new[new_index..]`.
    let mut best = vec![0f32; (old_len + 1) * (new_len + 1)];
    for old_index in (0..old_len).rev() {
        for new_index in (0..new_len).rev() {
            let share = old[old_index].common_share(&new[new_index]);
            shares[old_index * new_len + new_index] = share;
            let mut total = best[cell(old_index + 1, new_index)].max(best[cell(old_index, new_index + 1)]);
            if share >= MIN_LINE_SIMILARITY {
                total = total.max(best[cell(old_index + 1, new_index + 1)] + share);
            }
            best[cell(old_index, new_index)] = total;
        }
    }

    let mut pairs = Vec::new();
    let (mut old_index, mut new_index) = (0, 0);
    while old_index < old_len && new_index < new_len {
        let share = shares[old_index * new_len + new_index];
        let total = best[cell(old_index, new_index)];
        if share >= MIN_LINE_SIMILARITY && total == best[cell(old_index + 1, new_index + 1)] + share
        {
            pairs.push((old_index, new_index));
            old_index += 1;
            new_index += 1;
        } else if total == best[cell(old_index + 1, new_index)] {
            old_index += 1;
        } else {
            new_index += 1;
        }
    }
    pairs
}

/// Character ranges covering the changed tokens of a line. Whitespace between two changed tokens
/// is covered as well, so that a reworded phrase reads as one change.
fn changed_ranges(tokens: &[Token<'_>], mut is_changed: Vec<bool>) -> Vec<Range<usize>> {
    for index in 1..tokens.len().saturating_sub(1) {
        if tokens[index].is_blank() && is_changed[index - 1] && is_changed[index + 1] {
            is_changed[index] = true;
        }
    }

    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (token, is_changed) in tokens.iter().zip(is_changed) {
        if !is_changed {
            continue;
        }
        match ranges.last_mut() {
            Some(last) if last.end == token.chars.start => last.end = token.chars.end,
            Some(_) | None => ranges.push(token.chars.clone()),
        }
    }
    ranges
}

/// The changed character ranges of a removed line and of the added line it is paired with.
/// `None` when the two have too little in common.
fn paired_line_changes(
    old: &LineTokens<'_>,
    new: &LineTokens<'_>,
) -> Option<(Vec<Range<usize>>, Vec<Range<usize>>)> {
    let total = old.weight + new.weight;
    if total == 0 {
        return None;
    }

    let old_texts: Vec<&str> = old.tokens.iter().map(|token| token.text).collect();
    let new_texts: Vec<&str> = new.tokens.iter().map(|token| token.text).collect();
    let mut old_changed = vec![false; old_texts.len()];
    let mut new_changed = vec![false; new_texts.len()];
    let mut unchanged = 0;
    for op in capture_diff_slices(Algorithm::Myers, &old_texts, &new_texts) {
        match op {
            DiffOp::Equal { old_index, len, .. } => {
                unchanged += old.tokens[old_index..old_index + len]
                    .iter()
                    .map(Token::weight)
                    .sum::<usize>();
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => old_changed[old_index..old_index + old_len].fill(true),
            DiffOp::Insert {
                new_index, new_len, ..
            } => new_changed[new_index..new_index + new_len].fill(true),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                old_changed[old_index..old_index + old_len].fill(true);
                new_changed[new_index..new_index + new_len].fill(true);
            }
        }
    }

    if ((2 * unchanged) as f32 / total as f32) < MIN_LINE_SIMILARITY {
        return None;
    }
    Some((
        changed_ranges(&old.tokens, old_changed),
        changed_ranges(&new.tokens, new_changed),
    ))
}

/// The changed character ranges of two lines, when one is an edit of the other.
#[cfg(test)]
pub(super) fn line_changes(old: &str, new: &str) -> Option<(Vec<Range<usize>>, Vec<Range<usize>>)> {
    paired_line_changes(&LineTokens::new(old), &LineTokens::new(new))
}

/// The pairs of removed and added lines of a hunk that are edits of one another, as indices into
/// `old` and `new`.
#[cfg(test)]
pub(super) fn paired_lines(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    let old: Vec<LineTokens<'_>> = old.iter().map(|line| LineTokens::new(line)).collect();
    let new: Vec<LineTokens<'_>> = new.iter().map(|line| LineTokens::new(line)).collect();
    pair_lines(&old, &new)
}

/// Word-level changes of a hunk: for each of its lines, the character ranges that changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct HunkWordChanges {
    pub removed: Vec<Vec<Range<usize>>>,
    pub added: Vec<Vec<Range<usize>>>,
}

/// Computes the word-level changes between the removed lines `old` and the added lines `new` of
/// one hunk. Line terminators are ignored.
pub(super) fn hunk_word_changes(old: &[&str], new: &[&str]) -> HunkWordChanges {
    let mut changes = HunkWordChanges {
        removed: vec![Vec::new(); old.len()],
        added: vec![Vec::new(); new.len()],
    };
    if old.len().saturating_mul(new.len()) > MAX_PAIRING_CELLS {
        return changes;
    }

    let old: Vec<LineTokens<'_>> = old.iter().map(|line| LineTokens::new(line)).collect();
    let new: Vec<LineTokens<'_>> = new.iter().map(|line| LineTokens::new(line)).collect();
    for (old_index, new_index) in pair_lines(&old, &new) {
        if let Some((removed, added)) = paired_line_changes(&old[old_index], &new[new_index]) {
            changes.removed[old_index] = removed;
            changes.added[new_index] = added;
        }
    }
    changes
}

#[cfg(test)]
#[path = "word_diff_tests.rs"]
mod tests;
