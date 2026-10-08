/// Split text into overlapping chunks of at most `max_chars` characters.
///
/// Chunk boundaries are snapped to the nearest whitespace inside the window so
/// words are not cut in half. The trailing `overlap` characters are repeated at
/// the start of the next chunk to preserve context across boundaries.
pub fn chunk_text(text: &str, max_chars: usize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.trim().chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }

    let max = max_chars.max(1);
    let overlap = overlap.min(max.saturating_sub(1));

    let mut chunks = Vec::new();
    let mut start = 0usize;

    while start < chars.len() {
        let hard_end = (start + max).min(chars.len());
        let end = if hard_end == chars.len() {
            hard_end
        } else {
            // Prefer a whitespace boundary within the last quarter of the window.
            let lower = start + (max * 3) / 4;
            let mut boundary = hard_end;
            for i in (lower..hard_end).rev() {
                if chars[i].is_whitespace() {
                    boundary = i;
                    break;
                }
            }
            boundary
        };

        let chunk: String = chars[start..end].iter().collect();
        let chunk = chunk.trim().to_string();
        if !chunk.is_empty() {
            chunks.push(chunk);
        }

        if end >= chars.len() {
            break;
        }
        start = end.saturating_sub(overlap);
        if start >= end {
            start = end;
        }
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::chunk_text;

    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(chunk_text("   ", 100, 10).is_empty());
    }

    #[test]
    fn short_input_is_one_chunk() {
        assert_eq!(chunk_text("hello world", 100, 10), vec!["hello world"]);
    }

    #[test]
    fn long_input_splits_with_overlap() {
        let text = "alpha bravo charlie delta echo foxtrot golf hotel india juliet";
        let chunks = chunk_text(text, 20, 5);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 20);
        }
    }
}
