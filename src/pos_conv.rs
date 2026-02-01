use tower_lsp_server::lsp_types::Position;

/// Pre-built index of line start positions for O(1) position conversion.
/// Avoids O(N) scanning of the entire document on every position lookup.
#[derive(Debug, Clone, Default)]
pub struct LineIndex {
    /// Indices of the first character of each line.
    /// line_starts[0] is always 0 (first line starts at index 0).
    /// line_starts[1] is the index after the first '\n', etc.
    line_starts: Vec<usize>,
}

impl LineIndex {
    /// Build a new LineIndex from source characters.
    /// Scans the document once in O(N) to find all line boundaries.
    pub fn new(source: &[char]) -> Self {
        let mut line_starts = vec![0]; // First line always starts at 0

        for (idx, &ch) in source.iter().enumerate() {
            if ch == '\n' {
                // Next line starts after this newline
                line_starts.push(idx + 1);
            }
        }

        Self { line_starts }
    }

    /// Convert a character index to an LSP Position.
    /// O(log N) complexity using binary search on line starts.
    pub fn index_to_position(&self, source: &[char], index: usize) -> Position {
        // Binary search to find which line contains this index
        let line = match self.line_starts.binary_search(&index) {
            Ok(line) => line, // Exact match - index is at start of line
            Err(line) => line.saturating_sub(1), // Index is within previous line
        };

        let line_start = self.line_starts.get(line).copied().unwrap_or(0);

        // Calculate column as UTF-16 code units from line start to index
        let cols: usize = source[line_start..index.min(source.len())]
            .iter()
            .map(|c| c.len_utf16())
            .sum();

        Position {
            line: line as u32,
            character: cols as u32,
        }
    }

    /// Convert an LSP Position to a character index.
    /// O(1) complexity using direct array access.
    pub fn position_to_index(&self, source: &[char], position: Position) -> usize {
        let line_idx = position.line as usize;

        // Get the slice for the target line
        let Some(&line_start) = self.line_starts.get(line_idx) else {
            // Requested line doesn't exist - return last character
            return source.len().saturating_sub(1);
        };

        // Find where this line ends (at next line start or end of source)
        let line_end = self
            .line_starts
            .get(line_idx + 1)
            .copied()
            .unwrap_or(source.len());

        let target_line = &source[line_start..line_end];

        // Find character at requested column
        let target_char_idx = position.character as usize;

        if target_char_idx > target_line.len() {
            // Column is past end of line - clamp to end
            return line_end;
        }

        line_start + target_char_idx
    }

    /// Check if a position is beyond the current document bounds.
    pub fn is_position_out_of_bounds(&self, source: &[char], position: Position) -> bool {
        let line_idx = position.line as usize;

        if let Some(&line_start) = self.line_starts.get(line_idx) {
            let line_end = self
                .line_starts
                .get(line_idx + 1)
                .copied()
                .unwrap_or(source.len());

            let raw_line_len = line_end - line_start;
            let line_len = if raw_line_len > 0 && source.get(line_end - 1) == Some(&'\n') {
                raw_line_len - 1
            } else {
                raw_line_len
            };

            position.character as usize > line_len
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_index_single_line() {
        let source: Vec<char> = "hello world".chars().collect();
        let index = LineIndex::new(&source);

        let pos = Position {
            line: 0,
            character: 6,
        };
        assert_eq!(index.position_to_index(&source, pos), 6);
    }

    #[test]
    fn test_line_index_multiple_lines() {
        let source: Vec<char> = "hello\nworld\ntest".chars().collect();
        let index = LineIndex::new(&source);

        // Line 1, char 2 -> index 8 (after "hello\nwo")
        let pos = Position {
            line: 1,
            character: 2,
        };
        assert_eq!(index.position_to_index(&source, pos), 8);
    }

    #[test]
    fn test_index_to_position() {
        let source: Vec<char> = "hello\nworld".chars().collect();
        let index = LineIndex::new(&source);

        let pos = index.index_to_position(&source, 8);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 2);
    }
}
