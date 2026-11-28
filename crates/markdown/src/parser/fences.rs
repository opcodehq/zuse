//! Preserve explicit closing-fence metadata and source spelling.

use std::{borrow::Cow, ops::Range};

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

#[derive(Default)]
pub(super) struct FenceTracker {
    body_end: Option<usize>,
}

impl FenceTracker {
    /// Pulldown includes the closer's markers and trailing spaces in the
    /// block range. A marker-looking code line is included in a text event.
    pub(super) fn closer(
        &mut self,
        source: &str,
        event: &Event<'_>,
        range: &Range<usize>,
    ) -> Option<Range<usize>> {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(_))) => {
                self.body_end = Some(range.start);
            }
            Event::Text(_) => {
                if let Some(end) = &mut self.body_end {
                    *end = range.end;
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                let body_end = self.body_end.take()?;
                let fence = &source[range.clone()];
                let marker = *fence.as_bytes().first()?;
                let opener = fence.bytes().take_while(|&byte| byte == marker).count();
                let final_line = fence.rsplit_once(['\r', '\n'])?.1;
                let closer = final_line
                    .trim_end_matches([' ', '\t'])
                    .bytes()
                    .rev()
                    .take_while(|&byte| byte == marker)
                    .count();
                let trailing = final_line.len() - final_line.trim_end_matches([' ', '\t']).len();
                let start = range.end - trailing - closer;
                return (closer >= opener && start >= body_end).then_some(start..range.end);
            }
            _ => {}
        }
        None
    }
}

/// Pulldown 0.12 accepts trailing spaces on closing fences. Let it recognize
/// trailing tabs too, retaining the spelling of lines it treats as code.
pub(super) fn normalize_fence_tabs(source: &str) -> Cow<'_, str> {
    if !source.contains('\t') {
        return Cow::Borrowed(source);
    }
    let mut trial = None;
    let mut start = 0;
    for line in source.split_inclusive(['\r', '\n']) {
        let content = line.trim_end_matches([' ', '\t', '\r', '\n']);
        let trailing = &line[content.len()..];
        if trailing.contains('\t') && (content.ends_with("~~~") || content.ends_with("```")) {
            let bytes = trial.get_or_insert_with(|| source.as_bytes().to_vec());
            for byte in &mut bytes[start + content.len()..start + line.len()] {
                if *byte == b'\t' {
                    *byte = b' ';
                }
            }
        }
        start += line.len();
    }
    let Some(trial) = trial else {
        return Cow::Borrowed(source);
    };
    let trial = String::from_utf8(trial).expect("only ASCII tabs changed");
    let mut accepted = None;
    let mut fences = FenceTracker::default();
    for (event, range) in Parser::new_ext(&trial, super::options()).into_offset_iter() {
        if let Some(closer) = fences.closer(&trial, &event, &range)
            && trial.as_bytes()[closer.clone()] != source.as_bytes()[closer.clone()]
        {
            let bytes = accepted.get_or_insert_with(|| source.as_bytes().to_vec());
            bytes[closer.clone()].copy_from_slice(&trial.as_bytes()[closer]);
        }
    }
    accepted.map_or(Cow::Borrowed(source), |bytes| {
        Cow::Owned(String::from_utf8(bytes).expect("only ASCII tabs changed"))
    })
}
