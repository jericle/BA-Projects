//! Market-wide news from Google News RSS.
//!
//! `GET https://news.google.com/rss/search?q={query}&hl=en-US&gl=US&ceid=US:en`
//!
//! Free, no key, ~100 items per query with RFC 2822 timestamps and a publisher
//! element. Good enough for the market-wide "top 20" feed where Yahoo's search
//! endpoint returns listicles.

use anyhow::Result;
use chrono::{DateTime, Utc};
use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Debug, Clone)]
pub struct GItem {
    pub title: String,
    pub link: String,
    pub publisher: String,
    pub published_ts: i64,
}

pub async fn fetch_query(client: &reqwest::Client, query: &str) -> Result<Vec<GItem>> {
    let url = format!(
        "https://news.google.com/rss/search?q={}&hl=en-US&gl=US&ceid=US:en",
        urlencode(query)
    );
    let xml = crate::http::get_text(client, &url).await?;
    Ok(parse_rss(&xml))
}

/// One `<item>` in flight. `source_attr` is a fallback publisher; `source_text`
/// is the real publisher name and wins when present.
#[derive(Default)]
struct ItemBuilder {
    title: String,
    link: String,
    source_attr: String,
    source_text: String,
    pub_date: String,
    description: String,
}

pub fn parse_rss(xml: &str) -> Vec<GItem> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut items = Vec::new();
    let mut cur: Option<ItemBuilder> = None;
    let mut field: Option<String> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let qname = e.name();
                let name = local_name(qname.as_ref());
                match name {
                    "item" => cur = Some(ItemBuilder::default()),
                    "title" | "link" | "pubDate" | "description" | "source" => {
                        field = Some(name.to_string());
                        if name == "source" {
                            // The <source url="..."> attribute is only a fallback.
                            if let Some(v) = attribute(&e, b"url") {
                                if let Some(c) = cur.as_mut() {
                                    c.source_attr = v;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let qname = e.name();
                let name = local_name(qname.as_ref());
                if name == "source" {
                    if let (Some(v), Some(c)) = (attribute(&e, b"url"), cur.as_mut()) {
                        c.source_attr = v;
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if let (Some(f), Some(c)) = (field.as_deref(), cur.as_mut()) {
                    let Ok(text) = t.unescape() else { continue };
                    let slot = match f {
                        "title" => &mut c.title,
                        "link" => &mut c.link,
                        "pubDate" => &mut c.pub_date,
                        "source" => &mut c.source_text,
                        _ => &mut c.description,
                    };
                    if !slot.is_empty() {
                        slot.push(' ');
                    }
                    slot.push_str(text.trim());
                }
            }
            Ok(Event::End(e)) => {
                let qname = e.name();
                let name = local_name(qname.as_ref());
                if name == "item" {
                    if let Some(c) = cur.take() {
                        let ts = DateTime::parse_from_rfc2822(&c.pub_date)
                            .map(|d| d.with_timezone(&Utc).timestamp())
                            .unwrap_or_else(|_| Utc::now().timestamp());

                        let publisher = if c.source_text.trim().is_empty() {
                            publisher_name(&c.source_attr)
                        } else {
                            c.source_text.trim().to_string()
                        };

                        // Google appends a redundant " - Publisher" to the headline.
                        let mut title = c.title.trim().to_string();
                        for suffix in [format!(" - {publisher}"), format!(" – {publisher}")] {
                            if let Some(stripped) = title.strip_suffix(&suffix) {
                                title = stripped.trim().to_string();
                                break;
                            }
                        }
                        if title.is_empty() {
                            title = strip_tags(&c.description).chars().take(140).collect();
                        }
                        if !title.is_empty() {
                            items.push(GItem {
                                title,
                                link: c.link.trim().to_string(),
                                publisher,
                                published_ts: ts,
                            });
                        }
                    }
                    field = None;
                } else if matches!(
                    name,
                    "title" | "link" | "pubDate" | "description" | "source"
                ) {
                    field = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    items
}

/// Google News wraps its description in markup; flatten it to plain text.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").trim().to_string()
}

fn local_name(raw: &[u8]) -> &str {
    // Strip any namespace prefix; RSS here is unprefixed but be safe.
    let r = match raw.iter().rposition(|b| *b == b':') {
        Some(i) => &raw[i + 1..],
        None => raw,
    };
    std::str::from_utf8(r).unwrap_or("")
}

fn attribute(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.as_ref() == key {
            return a.unescape_value().ok().map(|v| v.to_string());
        }
    }
    None
}

/// Derive a human publisher name from the `url` attribute, e.g.
/// `https://www.reuters.com/` -> `Reuters`.
fn publisher_name(url: &str) -> String {
    if url.is_empty() {
        return "Google News".to_string();
    }
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    let first = host.split('/').next().unwrap_or("");
    let base = first.split('.').next().unwrap_or("");
    if base.is_empty() {
        "Google News".to_string()
    } else {
        let mut chars = base.chars();
        match chars.next() {
            Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            None => "Google News".to_string(),
        }
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
<item>
  <title>Nvidia beats on revenue - Reuters</title>
  <link>https://news.google.com/rss/articles/abc</link>
  <pubDate>Fri, 25 Sep 2026 16:24:49 GMT</pubDate>
  <source url="https://www.reuters.com/">Reuters</source>
</item>
</channel></rss>"#;

    #[test]
    fn parses_items_and_publisher() {
        let items = parse_rss(SAMPLE);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].publisher, "Reuters");
        // The redundant " - Publisher" suffix is stripped from the headline.
        assert_eq!(items[0].title, "Nvidia beats on revenue");
        // Fri 25 Sep 2026 16:24:49 GMT
        assert_eq!(items[0].published_ts, 1_790_353_489);
    }

    #[test]
    fn falls_back_to_url_when_source_text_missing() {
        let xml = r#"<rss><channel><item>
            <title>Something happened</title>
            <link>https://x/y</link>
            <pubDate>Fri, 25 Sep 2026 16:24:49 GMT</pubDate>
            <source url="https://www.barrons.com/"></source>
          </item></channel></rss>"#;
        let items = parse_rss(xml);
        assert_eq!(items[0].publisher, "Barrons");
        assert_eq!(items[0].title, "Something happened");
    }

    #[test]
    fn encodes_queries() {
        assert_eq!(urlencode("stock market when:1d"), "stock+market+when%3A1d");
    }
}
