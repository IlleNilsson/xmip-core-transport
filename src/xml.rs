//! The little XML a protocol speaks: the text of every element by one
//! name, and the five entities that text travels with.
//!
//! Picked by hand rather than parsed. The documents a transport meets are
//! flat — an S3 listing naming keys, a Blob enumeration naming blobs, an
//! error naming a code — and the one question either side asks is a scan,
//! not a tree. A transport that needs a tree names a contract technology
//! and does not read XML here. Each object-store transport carried this
//! file until 2026-09-09; it moved up to the capability so a technology
//! shares it rather than copies it (ADR-0044).

/// The text of every `<name>` element, entities unescaped.
#[must_use]
pub fn texts(xml: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else {
            break;
        };
        found.push(unescape(&after[..end]));
        rest = &after[end + close.len()..];
    }
    found
}

/// The text of the first `<name>` element.
#[must_use]
pub fn first(xml: &str, name: &str) -> Option<String> {
    texts(xml, name).into_iter().next()
}

/// `text` as element content: the four characters XML reserves, escaped.
#[must_use]
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Element content as text: the five predefined entities, unescaped.
#[must_use]
pub fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_element_by_name_reads_back_with_entities_intact() {
        let xml = format!(
            "<List><Prefix>in/</Prefix><Key>{}</Key><Key>{}</Key></List>",
            escape("in/a&b.edi"),
            escape("in/<c>.edi")
        );
        assert!(xml.contains("<Key>in/a&amp;b.edi</Key>"));
        assert_eq!(texts(&xml, "Key"), vec!["in/a&b.edi", "in/<c>.edi"]);
        assert_eq!(first(&xml, "Prefix").as_deref(), Some("in/"));
        assert_eq!(first(&xml, "Absent"), None);
        assert!(texts(&xml, "Absent").is_empty());
        assert!(texts("<Key>unclosed", "Key").is_empty());
    }

    #[test]
    fn the_five_entities_round_trip() {
        let raw = "a&b<c>\"d\"'e'";
        assert_eq!(unescape(&escape(raw)), raw);
        assert_eq!(unescape("&apos;x&apos;"), "'x'");
    }
}
