//! Display-ready form of a loaded [`FileDiff`], computed once per load.
//!
//! Rendering only slices these rows; it never re-parses, re-aligns, or
//! re-sanitizes content. Alignment comes unchanged from the Phase 2 engine:
//! each [`ViewRow::Line`] is exactly one `AlignedRow`.

use crate::{
    helpers::display::{sanitize_content, Segment},
    types::{
        diff::{
            BinarySummary, ConflictInfo, DiffContent, DiffFile, DiffLine, DiffRowKind, FileDiff,
            LineEnding, SubmoduleChange,
        },
        repository::ObjectId,
    },
};

pub const TAB_WIDTH: usize = 4;
/// Appended to a line without a final newline.
pub const NO_NEWLINE_MARK: &str = " \\ no newline";
/// Appended to a line that ended with CRLF.
pub const CRLF_MARK: &str = "\u{240d}";

/// One side of a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub number: usize,
    pub segments: Vec<Segment>,
    pub ending: LineEnding,
    /// Display width of the content plus any ending marker.
    pub width: usize,
}

impl Cell {
    fn new(line: DiffLine) -> Self {
        let segments = sanitize_content(&line.content, TAB_WIDTH);
        let mut width: usize = segments
            .iter()
            .map(|s| crate::helpers::display::width(&s.text))
            .sum();
        width += match line.ending {
            LineEnding::None => NO_NEWLINE_MARK.len(),
            LineEnding::CrLf => 1,
            LineEnding::Lf => 0,
        };
        Self {
            number: line.line_number,
            segments,
            ending: line.ending,
            width,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewRow {
    /// Start of a hunk: `@@ -a,b +c,d @@ heading`.
    Hunk { index: usize, header: String },
    Line {
        kind: DiffRowKind,
        old: Option<Cell>,
        new: Option<Cell>,
    },
}

/// Why there are no rows to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Binary(BinarySummary),
    Submodule(SubmoduleChange),
    Unmerged(ConflictInfo),
    TooLarge {
        limit: u64,
        size: Option<u64>,
    },
    Unreadable(String),
    NestedRepository,
    /// Text change without hunks: empty file, pure rename or copy, mode change.
    NoTextChanges,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Rows {
        rows: Vec<ViewRow>,
        /// Row index of each hunk header.
        hunk_starts: Vec<usize>,
        /// Digits needed for the largest line number.
        number_width: usize,
        /// Widest cell, for horizontal scrolling.
        max_width: usize,
        bounded_alignment: bool,
    },
    Notice(Notice),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffView {
    pub file: DiffFile,
    pub body: Body,
    pub old_oid: Option<ObjectId>,
    pub new_oid: Option<ObjectId>,
    pub added: usize,
    pub removed: usize,
    /// Approximate heap and inline size, for the cache budget.
    approx_bytes: usize,
}

impl DiffView {
    pub fn new(diff: FileDiff) -> Self {
        let stats = diff.line_stats();
        let body = match diff.content {
            DiffContent::Text { hunks } if hunks.is_empty() => Body::Notice(Notice::NoTextChanges),
            DiffContent::Text { hunks } => {
                let mut rows = Vec::new();
                let mut hunk_starts = Vec::with_capacity(hunks.len());
                let mut max_line = 1;
                let mut max_width = 0;
                let mut bounded_alignment = false;
                for (index, hunk) in hunks.into_iter().enumerate() {
                    hunk_starts.push(rows.len());
                    bounded_alignment |=
                        hunk.alignment == crate::types::diff::AlignmentQuality::Bounded;
                    max_line = max_line
                        .max(hunk.old_start + hunk.old_count)
                        .max(hunk.new_start + hunk.new_count);
                    let header = sanitize_content(&hunk.header, TAB_WIDTH)
                        .into_iter()
                        .map(|s| s.text)
                        .collect();
                    rows.push(ViewRow::Hunk { index, header });
                    for row in hunk.rows {
                        let old = row.old_line.map(Cell::new);
                        let new = row.new_line.map(Cell::new);
                        for c in old.iter().chain(&new) {
                            max_width = max_width.max(c.width);
                        }
                        rows.push(ViewRow::Line {
                            kind: row.kind,
                            old,
                            new,
                        });
                    }
                }
                Body::Rows {
                    rows,
                    hunk_starts,
                    number_width: max_line.to_string().len().max(3),
                    max_width,
                    bounded_alignment,
                }
            }
            DiffContent::Binary(b) => Body::Notice(Notice::Binary(b)),
            DiffContent::Submodule(s) => Body::Notice(Notice::Submodule(s)),
            DiffContent::Unmerged(c) => Body::Notice(Notice::Unmerged(c)),
            DiffContent::TooLarge { limit, size } => Body::Notice(Notice::TooLarge { limit, size }),
            DiffContent::Unreadable { reason } => Body::Notice(Notice::Unreadable(reason)),
            DiffContent::NestedRepository => Body::Notice(Notice::NestedRepository),
        };
        let approx_bytes = std::mem::size_of::<Self>() + body_bytes(&body);
        Self {
            file: diff.file,
            body,
            old_oid: diff.old_oid,
            new_oid: diff.new_oid,
            added: stats.added as usize,
            removed: stats.removed as usize,
            approx_bytes,
        }
    }

    /// Approximate memory held by this view: row structs plus text.
    pub fn approx_bytes(&self) -> usize {
        self.approx_bytes
    }

    pub fn row_count(&self) -> usize {
        match &self.body {
            Body::Rows { rows, .. } => rows.len(),
            Body::Notice(_) => 0,
        }
    }

    pub fn hunk_starts(&self) -> &[usize] {
        match &self.body {
            Body::Rows { hunk_starts, .. } => hunk_starts,
            Body::Notice(_) => &[],
        }
    }

    pub fn number_width(&self) -> usize {
        match &self.body {
            Body::Rows { number_width, .. } => *number_width,
            Body::Notice(_) => 3,
        }
    }

    pub fn max_width(&self) -> usize {
        match &self.body {
            Body::Rows { max_width, .. } => *max_width,
            Body::Notice(_) => 0,
        }
    }

    /// Index of the hunk containing view row `row`.
    pub fn hunk_at(&self, row: usize) -> Option<usize> {
        let starts = self.hunk_starts();
        match starts.binary_search(&row) {
            Ok(i) => Some(i),
            Err(0) => None,
            Err(i) => Some(i - 1),
        }
    }
}

fn body_bytes(body: &Body) -> usize {
    let Body::Rows {
        rows, hunk_starts, ..
    } = body
    else {
        return 0;
    };
    let cell = |c: &Option<Cell>| {
        c.as_ref().map_or(0, |c| {
            c.segments.capacity() * std::mem::size_of::<Segment>()
                + c.segments.iter().map(|s| s.text.capacity()).sum::<usize>()
        })
    };
    rows.capacity() * std::mem::size_of::<ViewRow>()
        + hunk_starts.capacity() * std::mem::size_of::<usize>()
        + rows
            .iter()
            .map(|r| match r {
                ViewRow::Hunk { header, .. } => header.capacity(),
                ViewRow::Line { old, new, .. } => cell(old) + cell(new),
            })
            .sum::<usize>()
}
