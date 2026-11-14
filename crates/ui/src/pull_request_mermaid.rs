use std::collections::{HashMap, HashSet};

pub(super) fn needs_vertical(diagram_width: f32, column_width: Option<f32>) -> bool {
    column_width.is_some_and(|width| diagram_width > width * 1.2)
}

pub(super) fn vertical_source(source: &str) -> Option<String> {
    if source.len() > crate::markdown::mermaid::MAX_SOURCE_BYTES {
        return None;
    }
    let header_end = source.find('\n')?;
    let header = &source[..header_end];
    let mut words = header.split_whitespace();
    if !matches!(words.next(), Some("flowchart" | "graph"))
        || words.next() != Some("LR")
        || words.next().is_some()
    {
        return None;
    }

    let mut outgoing = HashMap::new();
    let mut incoming = HashSet::new();
    let mut nodes = HashSet::new();
    for statement in source[header_end + 1..].split(['\n', ';']) {
        let statement = statement.trim();
        if statement.is_empty() {
            continue;
        }
        let chain = statement
            .split("-->")
            .map(node_id)
            .collect::<Option<Vec<_>>>()?;
        if chain.len() < 2 {
            return None;
        }
        for pair in chain.windows(2) {
            if pair[0] == pair[1]
                || outgoing.insert(pair[0], pair[1]).is_some()
                || !incoming.insert(pair[1])
            {
                return None;
            }
            nodes.extend(pair.iter().copied());
        }
    }
    if nodes.len() < 5 || outgoing.len() + 1 != nodes.len() {
        return None;
    }
    let mut current = *nodes.iter().find(|node| !incoming.contains(**node))?;
    let mut visited = 1;
    while let Some(next) = outgoing.get(current) {
        current = next;
        visited += 1;
        if visited > nodes.len() {
            return None;
        }
    }
    if visited != nodes.len() {
        return None;
    }
    let direction = header.rfind("LR")?;
    let mut result = source.to_owned();
    result.replace_range(direction..direction + 2, "TD");
    Some(result)
}

fn node_id(token: &str) -> Option<&str> {
    let token = token.trim();
    let end = token
        .find(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .unwrap_or(token.len());
    let id = &token[..end];
    if id.is_empty() {
        return None;
    }
    let label = token[end..].trim();
    if !label.is_empty() {
        let content = label.strip_prefix('[')?.strip_suffix(']')?;
        if content.contains(['[', ']', '\n', ';']) {
            return None;
        }
    }
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_only_simple_connected_chains() {
        let source = "flowchart LR\nA[Fetch context] --> B[Run checks] --> C[Validate] --> D[Review] --> E[Publish]";
        assert_eq!(
            vertical_source(source),
            Some(source.replacen("LR", "TD", 1))
        );
        for source in [
            "flowchart LR\nA-->B\nB-->C\nB-->D\nD-->E",
            "flowchart LR\nA-->B\nB-->C\nD-->E\nE-->D",
            "flowchart LR\nA-->B-->C-->D-->A",
            "flowchart LR\nA-->B-->C-->D",
            "flowchart TD\nA-->B-->C-->D-->E",
            "sequenceDiagram\nA-->B-->C-->D-->E",
            "flowchart LR\nA-->B-->C-->D-->E\nstyle A fill:red",
            "flowchart LR\nsubgraph X\nA-->B-->C-->D-->E\nend",
            "flowchart LR\nA-->B-->|yes|C-->D-->E",
            "flowchart LR\nA{Decision}-->B-->C-->D-->E",
        ] {
            assert_eq!(vertical_source(source), None, "{source}");
        }
    }

    #[test]
    fn adapts_only_when_the_column_is_too_narrow() {
        assert!(!needs_vertical(900.0, None));
        assert!(!needs_vertical(900.0, Some(760.0)));
        assert!(needs_vertical(1000.0, Some(760.0)));
        assert!(!needs_vertical(1000.0, Some(1000.0)));
    }

    #[test]
    fn converted_diagram_keeps_labels_and_renders_vertically() {
        let source = "flowchart LR\nA[Fetch context] --> B[Run checks] --> C[Validate candidates] --> D[Review evidence] --> E[Publish findings]";
        let palette = crate::markdown::mermaid::Palette::from_theme(&crate::theme::Theme::light());
        let original = crate::markdown::mermaid::render(source, &palette).unwrap();
        let vertical =
            crate::markdown::mermaid::render(&vertical_source(source).unwrap(), &palette).unwrap();
        for label in [
            "Fetch context",
            "Run checks",
            "Validate candidates",
            "Review evidence",
            "Publish findings",
        ] {
            assert!(original.contains(label));
            assert!(vertical.contains(label));
        }
        let original =
            crate::image_media::decode_image("image/svg+xml", original.into_bytes()).unwrap();
        let vertical =
            crate::image_media::decode_image("image/svg+xml", vertical.into_bytes()).unwrap();
        assert!(original.width > original.height);
        assert!(vertical.height > vertical.width);
    }
}
