use crate::common::{not_implemented, ParsedDocument, Result};
use async_trait::async_trait;
use std::path::Path;

#[async_trait]
pub trait DocumentService: Send + Sync {
    async fn parse(&self, path: &Path) -> Result<ParsedDocument>;
    async fn parse_pdf(&self, bytes: &[u8]) -> Result<ParsedDocument>;
    async fn parse_docx(&self, bytes: &[u8]) -> Result<ParsedDocument>;
    async fn parse_xlsx(&self, bytes: &[u8]) -> Result<ParsedDocument>;
    async fn parse_csv(&self, bytes: &[u8]) -> Result<ParsedDocument>;
    async fn parse_image(&self, bytes: &[u8]) -> Result<ParsedDocument>;
    async fn extract_text(&self, doc: &ParsedDocument) -> Result<String>;
}

pub struct StubDocumentService;

#[async_trait]
impl DocumentService for StubDocumentService {
    async fn parse(&self, _path: &Path) -> Result<ParsedDocument> {
        not_implemented("DocumentService::parse")
    }
    async fn parse_pdf(&self, _bytes: &[u8]) -> Result<ParsedDocument> {
        not_implemented("DocumentService::parse_pdf")
    }
    async fn parse_docx(&self, _bytes: &[u8]) -> Result<ParsedDocument> {
        not_implemented("DocumentService::parse_docx")
    }
    async fn parse_xlsx(&self, _bytes: &[u8]) -> Result<ParsedDocument> {
        not_implemented("DocumentService::parse_xlsx")
    }
    async fn parse_csv(&self, _bytes: &[u8]) -> Result<ParsedDocument> {
        not_implemented("DocumentService::parse_csv")
    }
    async fn parse_image(&self, _bytes: &[u8]) -> Result<ParsedDocument> {
        not_implemented("DocumentService::parse_image")
    }
    async fn extract_text(&self, _doc: &ParsedDocument) -> Result<String> {
        not_implemented("DocumentService::extract_text")
    }
}

use crate::common::{DimiError, ResourceClass};
use crate::services::ocr::OcrEngine;
use crate::services::scheduler::TokioSchedulerService;
use std::sync::Arc;

pub struct DefaultDocumentService {
    ocr: Arc<dyn OcrEngine>,
    scheduler: Arc<TokioSchedulerService>,
}

impl DefaultDocumentService {
    pub fn new(ocr: Arc<dyn OcrEngine>, scheduler: Arc<TokioSchedulerService>) -> Self {
        Self { ocr, scheduler }
    }
}

#[async_trait]
impl DocumentService for DefaultDocumentService {
    async fn parse(&self, path: &Path) -> Result<ParsedDocument> {
        let bytes = tokio::fs::read(path).await?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match ext.as_str() {
            "pdf" => self.parse_pdf(&bytes).await,
            "docx" => self.parse_docx(&bytes).await,
            "xlsx" | "xls" => self.parse_xlsx(&bytes).await,
            "csv" => self.parse_csv(&bytes).await,
            "png" | "jpg" | "jpeg" | "tiff" | "bmp" => self.parse_image(&bytes).await,
            "txt" | "md" => Ok(ParsedDocument {
                text: String::from_utf8_lossy(&bytes).into_owned(),
                structure: serde_json::Value::Null,
                metadata: serde_json::json!({ "mime_type": "text/plain" }),
            }),
            other => Err(DimiError::Unsupported(format!(
                "unsupported document extension: .{other}"
            ))),
        }
    }

    async fn parse_pdf(&self, bytes: &[u8]) -> Result<ParsedDocument> {
        let bytes_owned = bytes.to_vec();
        let text = self
            .scheduler
            .run("parse.pdf", ResourceClass::Cpu, 0, async {
                tokio::task::spawn_blocking(move || {
                    pdf_extract::extract_text_from_mem(&bytes_owned)
                        .map_err(|e| DimiError::Internal(format!("pdf parse failed: {e}")))
                })
                .await
                .map_err(|e| DimiError::Internal(format!("pdf parse task panicked: {e}")))?
            })
            .await?;

        if text.trim().is_empty() {
            return Err(DimiError::Degraded(
                "PDF has no extractable text layer (likely a scanned document); \
                 page rasterization + OCR for scanned PDFs is not yet implemented"
                    .into(),
            ));
        }

        Ok(ParsedDocument {
            text,
            structure: serde_json::Value::Null,
            metadata: serde_json::json!({ "mime_type": "application/pdf" }),
        })
    }

    async fn parse_docx(&self, bytes: &[u8]) -> Result<ParsedDocument> {
        let bytes_owned = bytes.to_vec();
        let text = self
            .scheduler
            .run("parse.docx", ResourceClass::Cpu, 0, async {
                tokio::task::spawn_blocking(move || extract_docx_text(&bytes_owned))
                    .await
                    .map_err(|e| DimiError::Internal(format!("docx parse task panicked: {e}")))?
            })
            .await?;

        Ok(ParsedDocument {
            text,
            structure: serde_json::Value::Null,
            metadata: serde_json::json!({
                "mime_type": "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            }),
        })
    }

    async fn parse_xlsx(&self, bytes: &[u8]) -> Result<ParsedDocument> {
        let bytes_owned = bytes.to_vec();
        let (text, structure) = self
            .scheduler
            .run("parse.xlsx", ResourceClass::Cpu, 0, async {
                tokio::task::spawn_blocking(move || extract_xlsx_text(&bytes_owned))
                    .await
                    .map_err(|e| DimiError::Internal(format!("xlsx parse task panicked: {e}")))?
            })
            .await?;

        Ok(ParsedDocument {
            text,
            structure,
            metadata: serde_json::json!({
                "mime_type": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            }),
        })
    }

    async fn parse_csv(&self, bytes: &[u8]) -> Result<ParsedDocument> {
        let bytes_owned = bytes.to_vec();
        let (text, structure) = self
            .scheduler
            .run("parse.csv", ResourceClass::Cpu, 0, async {
                tokio::task::spawn_blocking(move || extract_csv_text(&bytes_owned))
                    .await
                    .map_err(|e| DimiError::Internal(format!("csv parse task panicked: {e}")))?
            })
            .await?;

        Ok(ParsedDocument {
            text,
            structure,
            metadata: serde_json::json!({ "mime_type": "text/csv" }),
        })
    }

    async fn parse_image(&self, bytes: &[u8]) -> Result<ParsedDocument> {
        let text = self
            .scheduler
            .run("ocr.parse_image", ResourceClass::Cpu, 0, async {
                self.ocr.recognize(bytes).await
            })
            .await?;
        Ok(ParsedDocument {
            text,
            structure: serde_json::Value::Null,
            metadata: serde_json::json!({ "mime_type": "image", "source": "ocr" }),
        })
    }

    async fn extract_text(&self, doc: &ParsedDocument) -> Result<String> {
        Ok(doc.text.clone())
    }
}

fn extract_docx_text(bytes: &[u8]) -> Result<String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    use std::io::{Cursor, Read};

    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| DimiError::Internal(format!("docx is not a valid zip: {e}")))?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|e| DimiError::Internal(format!("docx missing word/document.xml: {e}")))?
        .read_to_string(&mut xml)
        .map_err(|e| DimiError::Internal(format!("docx document.xml is not valid utf-8: {e}")))?;

    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(false);

    let mut out = String::new();
    let mut in_text_run = false;
    let mut buf = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| DimiError::Internal(format!("docx xml parse error: {e}")))?
        {
            Event::Start(e) if e.local_name().as_ref() == b"t" => in_text_run = true,
            Event::End(e) if e.local_name().as_ref() == b"t" => in_text_run = false,
            Event::End(e) if e.local_name().as_ref() == b"p" => out.push('\n'),
            Event::Text(e) => {
                if in_text_run {
                    let decoded = e.decode().map_err(|err| {
                        DimiError::Internal(format!("docx text decode error: {err}"))
                    })?;
                    let unescaped = quick_xml::escape::unescape(&decoded).map_err(|err| {
                        DimiError::Internal(format!("docx entity unescape error: {err}"))
                    })?;
                    out.push_str(&unescaped);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

fn extract_xlsx_text(bytes: &[u8]) -> Result<(String, serde_json::Value)> {
    use calamine::{open_workbook_from_rs, Reader as CalamineReader, Xlsx};
    use std::fmt::Write as _;
    use std::io::Cursor;

    let mut workbook: Xlsx<_> = open_workbook_from_rs(Cursor::new(bytes.to_vec()))
        .map_err(|e| DimiError::Internal(format!("xlsx open failed: {e}")))?;

    let mut text = String::new();
    let mut sheets_json = serde_json::Map::new();

    for name in workbook.sheet_names().to_owned() {
        let range = workbook
            .worksheet_range(&name)
            .map_err(|e| DimiError::Internal(format!("xlsx sheet '{name}' read failed: {e}")))?;

        let mut rows_json = Vec::new();
        let _ = writeln!(text, "# Sheet: {name}");
        for row in range.rows() {
            let cells: Vec<String> = row.iter().map(|c| c.to_string()).collect();
            text.push_str(&cells.join("\t"));
            text.push('\n');
            rows_json.push(serde_json::Value::from(cells));
        }
        sheets_json.insert(name, serde_json::Value::from(rows_json));
    }

    Ok((text, serde_json::Value::Object(sheets_json)))
}

fn extract_csv_text(bytes: &[u8]) -> Result<(String, serde_json::Value)> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(bytes);

    let mut text = String::new();
    let mut rows_json = Vec::new();
    for result in reader.records() {
        let record = result.map_err(|e| DimiError::Internal(format!("csv parse error: {e}")))?;
        let cells: Vec<String> = record.iter().map(|c| c.to_string()).collect();
        text.push_str(&cells.join("\t"));
        text.push('\n');
        rows_json.push(serde_json::Value::from(cells));
    }
    Ok((text, serde_json::Value::from(rows_json)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::events::EventBus;
    use crate::services::ocr::StubOcrEngine;
    use crate::services::storage::SqliteStorageEngine;

    fn service() -> DefaultDocumentService {
        let storage: Arc<dyn crate::services::storage::StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        let scheduler = Arc::new(TokioSchedulerService::new(storage, EventBus::new()));
        DefaultDocumentService::new(Arc::new(StubOcrEngine), scheduler)
    }

    #[tokio::test]
    async fn csv_round_trips_rows() {
        let doc = service()
            .parse_csv(b"name,age\nAda,36\nGrace,85\n")
            .await
            .unwrap();
        assert!(doc.text.contains("Ada"));
        assert!(doc.text.contains("Grace"));
        let rows = doc.structure.as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1][0], "Ada");
    }

    fn build_minimal_docx(paragraphs: &[&str]) -> Vec<u8> {
        let body: String = paragraphs
            .iter()
            .map(|p| format!("<w:p><w:r><w:t>{p}</w:t></w:r></w:p>"))
            .collect();
        let document_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>{body}</w:body>
</w:document>"#
        );

        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("word/document.xml", options).unwrap();
            std::io::Write::write_all(&mut zip, document_xml.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        buf
    }

    #[tokio::test]
    async fn docx_extracts_paragraph_text() {
        let bytes = build_minimal_docx(&["Hello Dimi", "Second paragraph"]);
        let doc = service().parse_docx(&bytes).await.unwrap();
        assert!(doc.text.contains("Hello Dimi"));
        assert!(doc.text.contains("Second paragraph"));
        let hello_pos = doc.text.find("Hello Dimi").unwrap();
        let second_pos = doc.text.find("Second paragraph").unwrap();
        assert!(hello_pos < second_pos);
    }

    #[tokio::test]
    async fn parse_image_delegates_to_ocr_engine() {
        let err = service()
            .parse_image(b"not a real image")
            .await
            .unwrap_err();
        assert!(matches!(err, DimiError::NotImplemented(_)));
    }
}
