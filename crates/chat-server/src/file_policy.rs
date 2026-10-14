use std::fmt::Write as _;

#[derive(Debug, Clone)]
pub struct FilePolicy {
    max_bytes: u64,
    allowed_types: Vec<String>,
}

impl FilePolicy {
    pub fn new(max_bytes: u64, allowed_types: Vec<String>) -> Self {
        Self {
            max_bytes,
            allowed_types,
        }
    }

    pub fn check(&self, name: &str, bytes: &[u8]) -> Result<String, String> {
        if bytes.len() as u64 > self.max_bytes {
            return Err(format!(
                "exceeds the {} limit",
                format_bytes(self.max_bytes)
            ));
        }
        let mime = detect_mime(name, bytes);
        if !self.is_allowed(name, &mime) {
            return Err(format!("type `{mime}` is not allowed"));
        }
        Ok(mime)
    }

    fn is_allowed(&self, name: &str, mime: &str) -> bool {
        if self.allowed_types.is_empty() {
            return true;
        }
        let ext = extension(name);
        self.allowed_types
            .iter()
            .any(|rule| type_matches(rule, mime, &ext))
    }
}

fn type_matches(rule: &str, mime: &str, ext: &str) -> bool {
    let rule = rule.trim().to_ascii_lowercase();
    if rule.is_empty() {
        return false;
    }
    if let Some(prefix) = rule.strip_suffix("/*") {
        return mime.split('/').next() == Some(prefix);
    }
    if rule.contains('/') {
        return rule == mime;
    }
    rule == ext
}

pub fn detect_mime(name: &str, bytes: &[u8]) -> String {
    if let Some(mime) = sniff_mime(bytes) {
        return mime.to_string();
    }
    if let Some(mime) = mime_for(name) {
        return mime.to_string();
    }
    "application/octet-stream".to_string()
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    let starts = |sig: &[u8]| bytes.starts_with(sig);
    match bytes {
        _ if starts(b"\x89PNG\r\n\x1a\n") => Some("image/png"),
        _ if starts(b"\xff\xd8\xff") => Some("image/jpeg"),
        _ if starts(b"GIF87a") || starts(b"GIF89a") => Some("image/gif"),
        _ if starts(b"BM") => Some("image/bmp"),
        _ if starts(b"II*\x00") || starts(b"MM\x00*") => Some("image/tiff"),
        _ if starts(b"%PDF-") => Some("application/pdf"),
        _ if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") => Some("application/zip"),
        _ if starts(b"\x1f\x8b") => Some("application/gzip"),
        _ if starts(b"BZh") => Some("application/x-bzip2"),
        _ if starts(b"\xfd7zXZ\x00") => Some("application/x-xz"),
        _ if starts(b"7z\xbc\xaf\x27\x1c") => Some("application/x-7z-compressed"),
        _ if starts(b"Rar!\x1a\x07") => Some("application/vnd.rar"),
        _ if starts(b"\x7fELF") => Some("application/x-elf"),
        _ if starts(b"\x00asm") => Some("application/wasm"),
        _ if starts(b"OggS") => Some("audio/ogg"),
        _ if starts(b"ID3") || starts(b"\xff\xfb") => Some("audio/mpeg"),
        _ if starts(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) => Some("image/webp"),
        _ if starts(b"RIFF") && bytes.get(8..12) == Some(b"WAVE".as_slice()) => Some("audio/wav"),
        _ if bytes.get(4..8) == Some(b"ftyp".as_slice()) => Some("video/mp4"),
        _ => None,
    }
}

pub fn mime_for(name: &str) -> Option<&'static str> {
    Some(match extension(name).as_str() {
        "txt" | "md" | "log" => "text/plain",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "json" => "application/json",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "tar" => "application/x-tar",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "wasm" => "application/wasm",
        "exe" | "dll" | "so" => "application/x-elf",
        _ => return None,
    })
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    let mut out = String::new();
    if bytes >= GIB {
        let _ = write!(out, "{} GiB", bytes / GIB);
    } else if bytes >= MIB {
        let _ = write!(out, "{} MiB", bytes / MIB);
    } else if bytes >= KIB {
        let _ = write!(out, "{} KiB", bytes / KIB);
    } else {
        let _ = write!(out, "{bytes} B");
    }
    out
}
