use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd, html};
use std::collections::HashMap;

pub(super) fn render(source: &str) -> String {
    render_with_image_fallback(source, None, None)
}

pub(super) fn render_with_generated_images(
    source: &str,
    stable_id: &str,
    aliases: &HashMap<String, String>,
) -> String {
    render_with_image_fallback(
        source,
        Some(&format!("/sessions/{stable_id}/generated-images")),
        Some((stable_id, aliases)),
    )
}

fn render_with_image_fallback(
    source: &str,
    image_fallback: Option<&str>,
    generated_images: Option<(&str, &HashMap<String, String>)>,
) -> String {
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

    enum LinkRender {
        Unsafe,
        Normal,
        Direct(String),
    }

    let mut link_stack = Vec::new();
    let events = events.into_iter().filter_map(|event| match event {
        Event::Html(value) | Event::InlineHtml(value) => Some(Event::Text(value)),
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let safe = safe_link(&dest_url);
            if !safe {
                link_stack.push(LinkRender::Unsafe);
                return None;
            }
            let direct = relative_image_link(&dest_url)
                .then(|| {
                    generated_images.and_then(|(stable_id, aliases)| {
                        aliases.get(dest_url.as_ref()).map(|filename| {
                            format!("/sessions/{stable_id}/generated-images/{filename}")
                        })
                    })
                })
                .flatten();
            if let Some(direct) = direct {
                let start = format!(
                    "<a class=\"generated-image-name\" href=\"{}\">",
                    escape_html(&direct)
                );
                link_stack.push(LinkRender::Direct(direct));
                return Some(Event::InlineHtml(CowStr::Boxed(start.into_boxed_str())));
            }
            let dest_url = if relative_image_link(&dest_url) {
                image_fallback
                    .map(|destination| CowStr::Boxed(destination.to_owned().into_boxed_str()))
                    .unwrap_or(dest_url)
            } else {
                dest_url
            };
            link_stack.push(LinkRender::Normal);
            Some(Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }))
        }
        Event::End(TagEnd::Link) => match link_stack.pop().unwrap_or(LinkRender::Unsafe) {
            LinkRender::Unsafe => None,
            LinkRender::Normal => Some(Event::End(TagEnd::Link)),
            LinkRender::Direct(url) => {
                let url = escape_html(&url);
                let end = format!(
                    "</a><a class=\"generated-image-thumbnail-link\" href=\"{url}\" aria-label=\"Open full-size generated image\"><img class=\"generated-image-thumbnail\" src=\"{url}\" alt=\"Generated image preview\" loading=\"lazy\" decoding=\"async\"></a>"
                );
                Some(Event::InlineHtml(CowStr::Boxed(end.into_boxed_str())))
            }
        },
        Event::Start(Tag::Image { .. }) => Some(Event::Text(CowStr::Borrowed("Image: "))),
        Event::End(TagEnd::Image) => None,
        _ => Some(event),
    });
    let mut rendered = String::with_capacity(source.len());
    html::push_html(&mut rendered, events);
    format!("<div class=\"rich-text\">{rendered}</div>")
}

fn relative_image_link(destination: &str) -> bool {
    if destination.starts_with(['/', '#', '?']) || destination.contains(':') {
        return false;
    }
    let path = destination.split(['?', '#']).next().unwrap_or(destination);
    matches!(
        path.rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp")
    )
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
    fn relative_image_links_can_fall_back_to_the_session_gallery() {
        let aliases = HashMap::new();
        let rendered = render_with_generated_images(
            "[preview](outputs/example.png) [document](docs/readme.md) [remote](https://example.invalid/image.png)",
            "stable-session",
            &aliases,
        );
        assert!(
            rendered.contains("href=\"/sessions/stable-session/generated-images\">preview</a>")
        );
        assert!(rendered.contains("href=\"docs/readme.md\">document</a>"));
        assert!(rendered.contains("href=\"https://example.invalid/image.png\">remote</a>"));
    }

    #[test]
    fn known_relative_image_aliases_link_directly_to_the_generated_file() {
        let aliases = HashMap::from([(
            "outputs/example.png".to_owned(),
            "exec-synthetic.png".to_owned(),
        )]);
        let rendered = render_with_generated_images(
            "[preview](outputs/example.png)",
            "stable-session",
            &aliases,
        );
        assert!(rendered.contains(
            "href=\"/sessions/stable-session/generated-images/exec-synthetic.png\">preview</a>"
        ));
        assert!(rendered.contains("class=\"generated-image-thumbnail\""));
        assert!(
            rendered
                .contains("src=\"/sessions/stable-session/generated-images/exec-synthetic.png\"")
        );
        assert!(rendered.contains("loading=\"lazy\""));
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
