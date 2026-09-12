pub(crate) const MAX_QUERY_CHARS: usize = 200;

pub(crate) fn fts_query(input: &str) -> String {
    input
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

pub(crate) fn highlighted_html(snippet: &str) -> String {
    let mut html = String::with_capacity(snippet.len());
    let mut marked = false;
    for character in snippet.chars() {
        match character {
            '\u{1e}' => {
                if !marked {
                    html.push_str("<mark>");
                    marked = true;
                }
            }
            '\u{1f}' => {
                if marked {
                    html.push_str("</mark>");
                    marked = false;
                }
            }
            '&' => html.push_str("&amp;"),
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            '"' => html.push_str("&quot;"),
            '\'' => html.push_str("&#x27;"),
            _ => html.push(character),
        }
    }
    if marked {
        html.push_str("</mark>");
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_terms_are_quoted_and_combined() {
        assert_eq!(
            fts_query("foo-bar  say\"hello"),
            "\"foo-bar\" AND \"say\"\"hello\""
        );
    }

    #[test]
    fn only_generated_highlight_markup_is_trusted() {
        assert_eq!(
            highlighted_html("<script>\u{1e}find & me\u{1f}</script>"),
            "&lt;script&gt;<mark>find &amp; me</mark>&lt;/script&gt;"
        );
    }
}
