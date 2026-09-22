//! The little XML a protocol speaks: the text of every element by one name.
//!
//! Picked by hand rather than parsed. The documents a transport meets are
//! flat — an S3 listing naming keys, a Blob enumeration naming blobs, an
//! error naming a code — and the one question either side asks is a scan,
//! not a tree. A transport that needs a tree names a contract technology
//! and does not read XML here. Each object-store transport carried this
//! file until 2026-09-09; it moved up to the capability so a technology
//! shares it rather than copies it (ADR-0044). The entities the text
//! travels with are `xmip-core-codec`'s, which every XML reader in the
//! estate unescapes with since 2026-09-22.

use crate::error::Result;

/// The text of every `<name>` element, entities unescaped.
///
/// # Errors
///
/// An element's text holds an entity XML does not define.
pub fn texts(xml: &str, name: &str) -> Result<Vec<String>> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else {
            break;
        };
        found.push(codec::xml::unescape(&after[..end])?);
        rest = &after[end + close.len()..];
    }
    Ok(found)
}

/// The text of the first `<name>` element.
///
/// # Errors
///
/// As [`texts`].
pub fn first(xml: &str, name: &str) -> Result<Option<String>> {
    Ok(texts(xml, name)?.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::xml::escape;

    #[test]
    fn every_element_by_name_reads_back_with_entities_intact() {
        let xml = format!(
            "<List><Prefix>in/</Prefix><Key>{}</Key><Key>{}</Key></List>",
            escape("in/a&b.edi"),
            escape("in/<c>.edi")
        );
        assert!(xml.contains("<Key>in/a&amp;b.edi</Key>"));
        assert_eq!(
            texts(&xml, "Key").expect("read"),
            vec!["in/a&b.edi", "in/<c>.edi"]
        );
        assert_eq!(first(&xml, "Prefix").expect("read").as_deref(), Some("in/"));
        assert_eq!(first(&xml, "Absent").expect("read"), None);
        assert!(texts("<Key>unclosed", "Key").expect("read").is_empty());
    }

    #[test]
    fn an_entity_xml_does_not_define_is_a_protocol_error() {
        let error = texts("<Key>a&nbsp;b</Key>", "Key").expect_err("refused");
        assert!(!error.retryable);
        assert!(error.message.contains("&nbsp;"), "{}", error.message);
    }
}
