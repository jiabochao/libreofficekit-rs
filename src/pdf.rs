//! PDF export options, serialized into the JSON filter-options form that LibreOffice's PDF
//! export filter understands (`{"Name":{"type":"boolean","value":"true"}}`, LibreOffice 7.4+).

use std::fmt::Write as _;

/// `SelectPdfVersion` values of the PDF export filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfVersion {
    /// LibreOffice's default (PDF 1.7).
    Default,
    Pdf15,
    Pdf16,
    Pdf17,
    Pdf20,
    PdfA1b,
    PdfA2b,
    PdfA3b,
    PdfA4,
}

impl PdfVersion {
    fn code(self) -> i32 {
        match self {
            PdfVersion::Default => 0,
            PdfVersion::PdfA1b => 1,
            PdfVersion::PdfA2b => 2,
            PdfVersion::PdfA3b => 3,
            PdfVersion::PdfA4 => 4,
            PdfVersion::Pdf15 => 15,
            PdfVersion::Pdf16 => 16,
            PdfVersion::Pdf17 => 17,
            PdfVersion::Pdf20 => 20,
        }
    }
}

/// A raw filter-data value for [`PdfOptions::set`].
#[derive(Debug, Clone, PartialEq)]
pub enum FilterValue {
    Bool(bool),
    Long(i32),
    String(String),
}

/// Options for exporting a document to PDF.
///
/// Every field left as `None` keeps LibreOffice's default. Unlisted filter settings
/// (see <https://help.libreoffice.org/latest/en-US/text/shared/guide/pdf_params.html>)
/// can be passed with [`PdfOptions::set`].
#[derive(Debug, Clone, Default)]
pub struct PdfOptions {
    pub version: Option<PdfVersion>,
    /// PDF/UA (universal accessibility) compliance.
    pub pdf_ua: Option<bool>,
    pub tagged: Option<bool>,
    /// Page range such as `"1-3,5"`.
    pub page_range: Option<String>,
    /// Export headings as PDF bookmarks.
    pub export_bookmarks: Option<bool>,
    /// Export comments as PDF annotations.
    pub export_notes: Option<bool>,
    pub export_form_fields: Option<bool>,
    pub export_hidden_slides: Option<bool>,
    /// Calc: put each sheet on a single page.
    pub single_page_sheets: Option<bool>,
    /// JPEG quality 1..=100.
    pub jpeg_quality: Option<u8>,
    pub lossless_images: Option<bool>,
    /// Downsample images to at most this DPI.
    pub max_image_resolution: Option<u32>,
    pub embed_standard_fonts: Option<bool>,
    /// Password required to open the PDF.
    pub open_password: Option<String>,
    /// Password required to change permissions; enables permission restrictions.
    pub permission_password: Option<String>,
    /// Raw filter properties applied after (and overriding) the typed fields.
    pub extra: Vec<(String, FilterValue)>,
}

impl PdfOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets an arbitrary PDF export filter property, overriding the typed fields.
    pub fn set(mut self, name: impl Into<String>, value: FilterValue) -> Self {
        self.extra.push((name.into(), value));
        self
    }

    fn entries(&self) -> Vec<(String, FilterValue)> {
        use FilterValue::{Bool, Long};
        let mut v: Vec<(String, FilterValue)> = Vec::new();
        let mut push = |k: &str, val: FilterValue| v.push((k.to_string(), val));

        if let Some(ver) = self.version {
            push("SelectPdfVersion", Long(ver.code()));
        }
        if let Some(b) = self.pdf_ua {
            push("PDFUACompliance", Bool(b));
        }
        if let Some(b) = self.tagged {
            push("UseTaggedPDF", Bool(b));
        }
        if let Some(r) = &self.page_range {
            push("PageRange", FilterValue::String(r.clone()));
        }
        if let Some(b) = self.export_bookmarks {
            push("ExportBookmarks", Bool(b));
        }
        if let Some(b) = self.export_notes {
            push("ExportNotes", Bool(b));
        }
        if let Some(b) = self.export_form_fields {
            push("ExportFormFields", Bool(b));
        }
        if let Some(b) = self.export_hidden_slides {
            push("ExportHiddenSlides", Bool(b));
        }
        if let Some(b) = self.single_page_sheets {
            push("SinglePageSheets", Bool(b));
        }
        if let Some(q) = self.jpeg_quality {
            push("Quality", Long(q.clamp(1, 100) as i32));
        }
        if let Some(b) = self.lossless_images {
            push("UseLosslessCompression", Bool(b));
        }
        if let Some(dpi) = self.max_image_resolution {
            push("ReduceImageResolution", Bool(true));
            push("MaxImageResolution", Long(dpi.min(i32::MAX as u32) as i32));
        }
        if let Some(b) = self.embed_standard_fonts {
            push("EmbedStandardFonts", Bool(b));
        }
        if let Some(p) = &self.open_password {
            push("EncryptFile", Bool(true));
            push("DocumentOpenPassword", FilterValue::String(p.clone()));
        }
        if let Some(p) = &self.permission_password {
            push("RestrictPermissions", Bool(true));
            push("PermissionPassword", FilterValue::String(p.clone()));
        }
        for (k, val) in &self.extra {
            v.retain(|(name, _)| name != k);
            v.push((k.clone(), val.clone()));
        }
        v
    }

    /// The filter-options string passed to `saveAs`, or `None` when all defaults are kept.
    pub fn to_filter_options(&self) -> Option<String> {
        let entries = self.entries();
        if entries.is_empty() {
            return None;
        }
        let mut out = String::from("{");
        for (i, (name, value)) in entries.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let (ty, val) = match value {
                FilterValue::Bool(b) => ("boolean", b.to_string()),
                FilterValue::Long(n) => ("long", n.to_string()),
                FilterValue::String(s) => ("string", s.clone()),
            };
            write_json_string(&mut out, name);
            out.push_str(":{\"type\":\"");
            out.push_str(ty);
            out.push_str("\",\"value\":");
            write_json_string(&mut out, &val);
            out.push('}');
        }
        out.push('}');
        Some(out)
    }
}

/// JSON string escaping. Commas are escaped too: `doc_saveAs` splits the filter options on
/// `,` and re-joins them with `", "`, which would otherwise alter string values.
fn write_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ',' => out.push_str("\\u002c"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_produce_no_options() {
        assert_eq!(PdfOptions::default().to_filter_options(), None);
    }

    #[test]
    fn serializes_json() {
        let opts = PdfOptions {
            version: Some(PdfVersion::PdfA2b),
            page_range: Some("1-2,4".into()),
            ..Default::default()
        }
        .set("ExportNotes", FilterValue::Bool(true));
        assert_eq!(
            opts.to_filter_options().unwrap(),
            r#"{"SelectPdfVersion":{"type":"long","value":"2"},"PageRange":{"type":"string","value":"1-2\u002c4"},"ExportNotes":{"type":"boolean","value":"true"}}"#
        );
    }
}
