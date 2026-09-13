use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd, html};

pub(super) fn render(source: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let events: Vec<_> = Parser::new_ext(source, options).collect();
    if events.iter().all(plain_event) {
        return format!(
            "<p class=\"transcript-paragraph\">{}</p>",
            escape_html(source)
        );
    }

    let mut link_stack = Vec::new();
    let events = events.into_iter().filter_map(|event| match event {
        Event::Html(value) | Event::InlineHtml(value) => Some(Event::Text(value)),
        Event::Start(Tag::Link { ref dest_url, .. }) => {
            let safe = safe_link(dest_url);
            link_stack.push(safe);
            safe.then_some(event)
        }
        Event::End(TagEnd::Link) => link_stack.pop().unwrap_or(false).then_some(event),
        Event::Start(Tag::Image { .. }) => Some(Event::Text(CowStr::Borrowed("Image: "))),
        Event::End(TagEnd::Image) => None,
        _ => Some(event),
    });
    let mut rendered = String::with_capacity(source.len());
    html::push_html(&mut rendered, events);
    format!("<div class=\"rich-text\">{rendered}</div>")
}

pub(super) fn render_plain(source: &str) -> String {
    format!(
        "<p class=\"transcript-paragraph\">{}</p>",
        escape_html(source)
    )
}

fn plain_event(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Start(Tag::Paragraph | Tag::HtmlBlock)
            | Event::End(TagEnd::Paragraph | TagEnd::HtmlBlock)
            | Event::Text(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::SoftBreak
            | Event::HardBreak
    )
}

fn safe_link(destination: &str) -> bool {
    if destination.chars().any(char::is_control) || destination.trim() != destination {
        return false;
    }
    let Some(colon) = destination.find(':') else {
        return true;
    };
    if destination[..colon]
        .bytes()
        .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'+' | b'-' | b'.'))
    {
        return true;
    }
    matches!(
        destination[..colon].to_ascii_lowercase().as_str(),
        "http" | "https" | "mailto"
    )
}

fn escape_html(source: &str) -> String {
    let mut escaped = String::with_capacity(source.len());
    for character in source.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#x27;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_gfm_code_and_tables() {
        let rendered = render(
            "## Result\n\n```rust\nfn main() {}\n```\n\n| A | B |\n| - | - |\n| one | ~~two~~ |",
        );
        assert!(rendered.contains("<h2>Result</h2>"));
        assert!(
            rendered.contains("<pre><code class=\"language-rust\">fn main() {}\n</code></pre>")
        );
        assert!(rendered.contains("<table>"));
        assert!(rendered.contains("<del>two</del>"));
    }

    #[test]
    fn plain_text_preserves_whitespace_and_escapes_html() {
        assert_eq!(
            render("ordinary  text\n<script>alert('x')</script>"),
            "<p class=\"transcript-paragraph\">ordinary  text\n&lt;script&gt;alert(&#x27;x&#x27;)&lt;/script&gt;</p>"
        );
    }

    #[test]
    fn removes_remote_images_raw_html_and_unsafe_link_destinations() {
        let rendered = render(
            "**safe** <img src=\"https://example.invalid/a.png\">\n\n![remote](https://example.invalid/b.png) [bad](JaVaScRiPt:alert(1)) [data](data:text/html,x) [good](https://example.invalid)",
        );
        assert!(!rendered.contains("<img"), "{rendered}");
        assert!(!rendered.contains("href=\"JaVaScRiPt:"), "{rendered}");
        assert!(!rendered.contains("href=\"data:"), "{rendered}");
        assert!(rendered.contains("&lt;img src=\"https://example.invalid/a.png\"&gt;"));
        assert!(rendered.contains("Image: remote"));
        assert!(rendered.contains("bad"));
        assert!(rendered.contains("data"));
        assert!(rendered.contains("href=\"https://example.invalid\""));
    }

    #[test]
    fn link_policy_allows_only_relative_and_explicit_safe_schemes() {
        for destination in [
            "relative/path",
            "/absolute/path",
            "#fragment",
            "HTTPS://example.invalid",
            "http://example.invalid",
            "mailto:test@example.invalid",
        ] {
            assert!(safe_link(destination), "{destination}");
        }
        for destination in [
            "javascript:alert(1)",
            "data:text/html,x",
            "file:///private/path",
            " https://example.invalid",
            "https://example.invalid\nmalicious",
        ] {
            assert!(!safe_link(destination), "{destination}");
        }
    }

    #[test]
    fn plain_text_escape_expansion_stays_linear() {
        let source = "&".repeat(super::super::queries::MESSAGE_PREVIEW_CHARS as usize);
        let rendered = render(&source);
        assert!(
            rendered.len() <= source.len() * 5 + 64,
            "{} bytes",
            rendered.len()
        );
    }
}
