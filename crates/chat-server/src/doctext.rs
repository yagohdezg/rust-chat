//! Best-effort text extraction from binary document formats so their contents
//! can be inlined into the prompt (see `FileWorkspace::attachment_context`).
//!
//! All parsing here is CPU-bound and must run on a blocking thread; callers cap
//! the input size first. Unsupported or malformed inputs return `None`.

use std::io::{Cursor, Read};

use quick_xml::events::Event;
use quick_xml::Reader;

/// Files larger than this are not parsed: the extracted text would exceed what
/// can be inlined anyway, and parsing huge inputs could stall a worker.
pub const MAX_PARSE_BYTES: usize = 25 * 1024 * 1024;

/// Extract text from a binary document, choosing a parser by file extension.
/// Returns `None` for unsupported formats, oversized inputs, or empty results.
pub fn extract_text(filename: &str, bytes: &[u8]) -> Option<String> {
    if bytes.len() > MAX_PARSE_BYTES {
        return None;
    }
    let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
    let text = match ext.as_str() {
        "pdf" => pdf_extract::extract_text_from_mem(bytes).ok(),
        "docx" => extract_docx(bytes),
        "pptx" => extract_pptx(bytes),
        "xlsx" => extract_xlsx(bytes),
        "html" | "htm" | "xhtml" => Some(html_to_text(&String::from_utf8_lossy(bytes))),
        _ => None,
    }?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn open_zip(bytes: &[u8]) -> Option<zip::ZipArchive<Cursor<&[u8]>>> {
    zip::ZipArchive::new(Cursor::new(bytes)).ok()
}

fn read_zip_entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<Vec<u8>> {
    let mut file = zip.by_name(name).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Word documents: all runs (`w:t`) in the main document, newline per paragraph.
fn extract_docx(bytes: &[u8]) -> Option<String> {
    let mut zip = open_zip(bytes)?;
    let xml = read_zip_entry(&mut zip, "word/document.xml")?;
    Some(collect_element_text(
        &String::from_utf8_lossy(&xml),
        &["w:t"],
        &["w:p"],
    ))
}

/// PowerPoint: text runs (`a:t`) from every slide, slides joined by blank lines.
fn extract_pptx(bytes: &[u8]) -> Option<String> {
    let mut zip = open_zip(bytes)?;
    let mut names: Vec<String> = zip
        .file_names()
        .filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))
        .map(str::to_string)
        .collect();
    names.sort();
    let mut out = String::new();
    for name in names {
        if let Some(xml) = read_zip_entry(&mut zip, &name) {
            out.push_str(&collect_element_text(
                &String::from_utf8_lossy(&xml),
                &["a:t"],
                &["a:p"],
            ));
            out.push('\n');
        }
    }
    Some(out)
}

/// Excel workbooks: each worksheet rendered as tab-separated rows, resolved
/// against the shared-string table.
fn extract_xlsx(bytes: &[u8]) -> Option<String> {
    let mut zip = open_zip(bytes)?;
    let shared = read_zip_entry(&mut zip, "xl/sharedStrings.xml")
        .map(|xml| parse_shared_strings(&String::from_utf8_lossy(&xml)))
        .unwrap_or_default();
    let mut sheets: Vec<String> = zip
        .file_names()
        .filter(|n| n.starts_with("xl/worksheets/sheet") && n.ends_with(".xml"))
        .map(str::to_string)
        .collect();
    sheets.sort();
    let mut out = String::new();
    for (i, name) in sheets.iter().enumerate() {
        if let Some(xml) = read_zip_entry(&mut zip, name) {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&parse_sheet(&String::from_utf8_lossy(&xml), &shared));
        }
    }
    Some(out)
}

/// Concatenate the text inside `text_tags`, emitting a newline on each
/// `break_tags` close. Nested same-name tags are tracked with a depth counter.
fn collect_element_text(xml: &str, text_tags: &[&str], break_tags: &[&str]) -> String {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut capture = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if text_tags.iter().any(|t| e.name().as_ref() == t.as_bytes()) {
                    capture += 1;
                }
            }
            Ok(Event::End(e)) => {
                let name = e.name();
                let name = name.as_ref();
                if text_tags.iter().any(|t| name == t.as_bytes()) {
                    capture = capture.saturating_sub(1);
                }
                if break_tags.iter().any(|t| name == t.as_bytes()) {
                    out.push('\n');
                }
            }
            Ok(Event::Text(t)) => {
                if capture > 0 {
                    if let Ok(s) = t.decode() {
                        out.push_str(&s);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    out
}

fn parse_shared_strings(xml: &str) -> Vec<String> {
    let mut reader = Reader::from_str(xml);
    let mut strings = Vec::new();
    let mut current = String::new();
    let mut in_si = false;
    let mut in_t = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"si" => {
                    in_si = true;
                    current.clear();
                }
                b"t" => in_t = true,
                _ => {}
            },
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"t" => in_t = false,
                b"si" => {
                    in_si = false;
                    strings.push(std::mem::take(&mut current));
                }
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if in_si && in_t {
                    if let Ok(s) = t.decode() {
                        current.push_str(&s);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    strings
}

fn parse_sheet(xml: &str, shared: &[String]) -> String {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell_type = String::new();
    let mut value = String::new();
    let mut in_v = false;
    let mut in_is = false;
    let mut in_t = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"c" => {
                    cell_type.clear();
                    value.clear();
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"t" {
                            cell_type = String::from_utf8_lossy(&attr.value).into_owned();
                        }
                    }
                }
                b"v" => in_v = true,
                b"is" => in_is = true,
                b"t" => in_t = true,
                _ => {}
            },
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"v" => in_v = false,
                b"t" => in_t = false,
                b"is" => in_is = false,
                b"c" => {
                    let text = if cell_type == "s" {
                        value
                            .parse::<usize>()
                            .ok()
                            .and_then(|i| shared.get(i).cloned())
                            .unwrap_or_default()
                    } else {
                        value.clone()
                    };
                    row.push(text);
                }
                b"row" => {
                    out.push_str(&row.join("\t"));
                    out.push('\n');
                    row.clear();
                }
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if in_v || (in_is && in_t) {
                    if let Ok(s) = t.decode() {
                        value.push_str(&s);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    if !row.is_empty() {
        out.push_str(&row.join("\t"));
        out.push('\n');
    }
    out
}

/// Strip HTML tags and decode the common entities. Script/style contents are
/// dropped; block-level tags become newlines.
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    let mut in_literal = false;
    while i < html.len() {
        if html.as_bytes()[i] == b'<' {
            if let Some(end) = html[i..].find('>') {
                let tag = &html[i + 1..i + end];
                let name = tag
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if name == "script" || name == "style" {
                    in_literal = !tag.starts_with('/');
                }
                if matches!(
                    name.as_str(),
                    "p" | "br" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                ) {
                    out.push('\n');
                }
                i += end + 1;
                continue;
            }
        }
        let ch = html[i..].chars().next().unwrap();
        if !in_literal {
            out.push(ch);
        }
        i += ch.len_utf8();
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}
