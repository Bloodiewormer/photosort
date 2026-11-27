use std::collections::BTreeSet;

/// Escapes XML special characters for safe inclusion in XML text nodes or attribute values.
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Metadata passed to build an XMP packet or sidecar.
#[derive(Debug, Clone, Default)]
pub struct XmpData<'a> {
    pub categories: &'a [&'a str],
    pub tags: &'a [&'a str],
    pub works: &'a [&'a str],
    pub persons: &'a [&'a str],
    pub hierarchical_categories: &'a [&'a str],
}

/// Generates a valid UTF-8 XMP XML packet from the provided metadata.
pub fn generate_xmp(data: &XmpData<'_>) -> String {
    // Collect all subjects for dc:subject (categories, tags, works, persons)
    let mut subjects = BTreeSet::new();
    for c in data.categories {
        let trimmed = c.trim();
        if !trimmed.is_empty() {
            subjects.insert(trimmed);
        }
    }
    for t in data.tags {
        let trimmed = t.trim();
        if !trimmed.is_empty() {
            subjects.insert(trimmed);
        }
    }
    for w in data.works {
        let trimmed = w.trim();
        if !trimmed.is_empty() {
            subjects.insert(trimmed);
        }
    }
    for p in data.persons {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            subjects.insert(trimmed);
        }
    }

    // Collect hierarchical categories for lr:hierarchicalSubject (if any subcategories exist)
    let mut hier_subjects = BTreeSet::new();
    for h in data.hierarchical_categories {
        let trimmed = h.trim();
        if !trimmed.is_empty() {
            hier_subjects.insert(trimmed);
        }
    }

    // Collect named persons for iptcExt:PersonInImage
    let mut persons = BTreeSet::new();
    for p in data.persons {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            persons.insert(trimmed);
        }
    }

    let mut xml = String::from("<?xpacket begin=\"\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    xml.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n");
    xml.push_str("  <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
    xml.push_str("    <rdf:Description rdf:about=\"\"\n");
    xml.push_str("        xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n");
    xml.push_str("        xmlns:lr=\"http://ns.adobe.com/lightroom/1.0/\"\n");
    xml.push_str("        xmlns:iptcExt=\"http://iptc.org/std/Iptc4xmpExt/2008-02-29/\">\n");

    if !subjects.is_empty() {
        xml.push_str("      <dc:subject>\n");
        xml.push_str("        <rdf:Bag>\n");
        for s in subjects {
            xml.push_str(&format!("          <rdf:li>{}</rdf:li>\n", xml_escape(s)));
        }
        xml.push_str("        </rdf:Bag>\n");
        xml.push_str("      </dc:subject>\n");
    }

    if !hier_subjects.is_empty() {
        xml.push_str("      <lr:hierarchicalSubject>\n");
        xml.push_str("        <rdf:Bag>\n");
        for h in hier_subjects {
            xml.push_str(&format!("          <rdf:li>{}</rdf:li>\n", xml_escape(h)));
        }
        xml.push_str("        </rdf:Bag>\n");
        xml.push_str("      </lr:hierarchicalSubject>\n");
    }

    if !persons.is_empty() {
        xml.push_str("      <iptcExt:PersonInImage>\n");
        xml.push_str("        <rdf:Bag>\n");
        for p in persons {
            xml.push_str(&format!("          <rdf:li>{}</rdf:li>\n", xml_escape(p)));
        }
        xml.push_str("        </rdf:Bag>\n");
        xml.push_str("      </iptcExt:PersonInImage>\n");
    }

    xml.push_str("    </rdf:Description>\n");
    xml.push_str("  </rdf:RDF>\n");
    xml.push_str("</x:xmpmeta>\n");
    xml.push_str("<?xpacket end=\"w\"?>\n");
    xml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escape_handles_special_characters() {
        let input = "Manga & Anime <\"Best\" 'Shonen'> & More";
        let escaped = xml_escape(input);
        assert_eq!(
            escaped,
            "Manga &amp; Anime &lt;&quot;Best&quot; &apos;Shonen&apos;&gt; &amp; More"
        );
    }

    #[test]
    fn generate_xmp_includes_dc_subject_and_hierarchical_and_persons() {
        let data = XmpData {
            categories: &["Página promocional"],
            tags: &["portada"],
            works: &["Frieren"],
            persons: &["Fern"],
            hierarchical_categories: &["Manga|Scanlation|Página promocional"],
        };
        let xmp = generate_xmp(&data);

        assert!(xmp.starts_with("<?xpacket begin=\"\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>"));
        assert!(xmp.ends_with("<?xpacket end=\"w\"?>\n"));
        assert!(xmp.contains("<dc:subject>"));
        assert!(xmp.contains("<rdf:li>Fern</rdf:li>"));
        assert!(xmp.contains("<rdf:li>Frieren</rdf:li>"));
        assert!(xmp.contains("<rdf:li>Página promocional</rdf:li>"));
        assert!(xmp.contains("<rdf:li>portada</rdf:li>"));
        assert!(xmp.contains("<lr:hierarchicalSubject>"));
        assert!(xmp.contains("<rdf:li>Manga|Scanlation|Página promocional</rdf:li>"));
        assert!(xmp.contains("<iptcExt:PersonInImage>"));
        assert!(xmp.contains("<rdf:li>Fern</rdf:li>"));
    }

    #[test]
    fn generate_xmp_omits_hierarchical_when_no_subcategories() {
        let data = XmpData {
            categories: &["Fotos"],
            tags: &["vacaciones"],
            works: &[],
            persons: &[],
            hierarchical_categories: &[],
        };
        let xmp = generate_xmp(&data);
        assert!(xmp.contains("<dc:subject>"));
        assert!(!xmp.contains("<lr:hierarchicalSubject>"));
        assert!(!xmp.contains("<iptcExt:PersonInImage>"));
    }
}
