// ------------------------------------------------------------------
//  Text field editing
// ------------------------------------------------------------------
//
// UTF-8-safe caret editing shared by every in-terminal text field: the
// settings popup's single- and multi-line fields and the debate modal's
// subject field.

/// Insert `c` into `text` at char index `idx` (clamped to the string's
/// length).
pub fn insert_char_at(text: &mut String, idx: usize, c: char) {
  let idx = idx.min(text.chars().count());
  let byte = text
    .char_indices()
    .nth(idx)
    .map(|(i, _)| i)
    .unwrap_or(text.len());
  text.insert(byte, c);
}

/// Remove the char at char index `idx`, if any.
pub fn remove_char_at(text: &mut String, idx: usize) {
  if let Some((byte, _)) = text.char_indices().nth(idx) {
    text.remove(byte);
  }
}

/// Where the caret lands moving one line up or down within `text`, keeping
/// its column where the destination line is at least that long and clamping
/// to that line's end otherwise. `None` at the field's first (going up) or
/// last (going down) line, so the caller can fall back to cycling focus
/// instead.
pub fn move_caret_vertical(text: &str, caret: usize, down: bool) -> Option<usize> {
  let chars: Vec<char> = text.chars().collect();
  let caret = caret.min(chars.len());
  let line_start = chars[..caret]
    .iter()
    .rposition(|c| *c == '\n')
    .map_or(0, |i| i + 1);
  let column = caret - line_start;
  if down {
    let rel = chars[caret..].iter().position(|c| *c == '\n')?;
    let next_start = caret + rel + 1;
    let next_len = chars[next_start..]
      .iter()
      .position(|c| *c == '\n')
      .unwrap_or(chars.len() - next_start);
    Some(next_start + column.min(next_len))
  } else {
    if line_start == 0 {
      return None;
    }
    let previous_start = chars[..line_start - 1]
      .iter()
      .rposition(|c| *c == '\n')
      .map_or(0, |i| i + 1);
    let previous_len = line_start - 1 - previous_start;
    Some(previous_start + column.min(previous_len))
  }
}
