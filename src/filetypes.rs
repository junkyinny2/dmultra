//! File-type capture lists.
//!
//!   - General "Downloaded file types" default list:
//!     "3GP 7Z AAC ACE AIF APK ARJ ASF AVI BIN BZ2 EXE GZ GZIP IMG ISO LZH
//!      M4A M4V MKV MOV MP3 MP4 MPA MPE MPEG MPG MSI MSU OGG OGV PDF PLJ
//!      PPS PPT QT R0* R1* RA RAR RM RMVB SEA SIT SITX TAR TIF TIFF WAV
//!      WMA WMV Z ZIP"
//! Wildcard patterns are supported (`R0*`, `R1*` for split RAR
//! archives). Matching is case-insensitive.

/// Default general download file types.
pub const DEFAULT_FILE_TYPES: &str = "3GP 7Z AAC ACE AIF APK ARJ ASF AVI BIN BZ2 EXE GZ GZIP IMG ISO LZH M4A M4V MKV MOV MP3 MP4 MPA MPE MPEG MPG MSI MSU OGG OGV PDF PLJ PPS PPT QT R0* R1* RA RAR RM RMVB SEA SIT SITX TAR TIF TIFF WAV WMA WMV Z ZIP";

/// Media / web-player capture list (adds DOC/DOCX/FLV/WEBM to the general list).

/// A single file-type pattern, e.g. `ZIP` or `R0*`.
#[derive(Debug, Clone)]
pub struct Pattern {
    pub raw: String,
    /// Prefix before the `*` (empty when no wildcard).
    prefix: String,
    /// True when the pattern ends with `*`.
    wildcard: bool,
}

pub fn parse_list(list: &str) -> Vec<Pattern> {
    list.split_whitespace()
        .map(|tok| {
            let raw = tok.trim_start_matches('.').to_ascii_uppercase();
            let prefix = raw.strip_suffix('*').map(|p| p.to_string());
            match prefix {
                Some(prefix) => Pattern { raw, prefix, wildcard: true },
                None => {
                    let prefix = raw.clone();
                    Pattern { raw, prefix, wildcard: false }
                }
            }
        })
        .collect()
}

fn extension_of(name: &str) -> String {
    match name.rsplit_once('.') {
        // An extension of 1..=5 chars at the end, and not the whole name.
        Some((head, ext)) if !ext.is_empty() && !head.is_empty() => ext.to_string(),
        _ => String::new(),
    }
}

/// Does `name` (a URL path or local filename) match any pattern?
pub fn matches(name: &str, patterns: &[Pattern]) -> bool {
    let ext = extension_of(name).to_ascii_uppercase();
    if ext.is_empty() {
        return false;
    }
    patterns.iter().any(|p| {
        if p.wildcard {
            ext.starts_with(&p.prefix) && !p.prefix.is_empty()
        } else {
            ext == p.raw
        }
    })
}

/// Extract the last path segment of a URL, percent-decoded.
pub fn url_last_segment(url: &str) -> String {
    let path = match url.split_once("://").map(|(_, rest)| rest) {
        Some(rest) => rest,
        None => url,
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let seg = path.rsplit('/').next().unwrap_or(path);
    if seg.is_empty() {
        return String::new();
    }
    percent_encoding::percent_decode_str(seg)
        .decode_utf8_lossy()
        .replace(['\\', ':', '*', '?', '"', '<', '>', '|'], "_")
}

/// Left-pane categories ("Download categories"). Returns the
/// category label for a filename, or None when it belongs to no
/// category (shows only under "All").
pub fn category_of(name: &str) -> Option<&'static str> {
    const COMPRESSED: &[&str] = &[
        "7Z", "ACE", "ARJ", "BZ2", "CAB", "GZ", "GZIP", "IMG", "ISO", "LZH", "RAR",
        "R0", "R1", "SEA", "SIT", "SITX", "TAR", "TBZ2", "TGZ", "Z", "ZIP", "ZOO",
    ];
    const DOCUMENTS: &[&str] = &[
        "CHM", "DOC", "DOCX", "EPUB", "HLP", "MDB", "PDF", "PPS", "PPT", "RTF",
        "TEX", "TXT", "WPD", "XLS", "XLSX",
    ];
    const MUSIC: &[&str] = &[
        "AAC", "AIF", "APE", "CDA", "FLAC", "M4A", "MID", "MP3", "OGG", "OPUS",
        "RA", "RAM", "WAV", "WMA",
    ];
    const VIDEO: &[&str] = &[
        "3GP", "ASF", "AVI", "DIVX", "FLV", "M4V", "MKV", "MOV", "MP4", "MPE",
        "MPEG", "MPG", "OGM", "OGV", "QT", "RM", "RMVB", "VRO", "WEBM", "WMV",
    ];
    const PROGRAMS: &[&str] = &["APK", "BIN", "DEB", "DMG", "EXE", "MSI", "MSU", "PKG", "RPM", "SEA"];
    let ext = extension_of(name).to_ascii_uppercase();
    if ext.is_empty() {
        return None;
    }
    // R0*/R1* wildcard split archives.
    if ext.starts_with("R0") || ext.starts_with("R1") {
        return Some("Compressed");
    }
    for (set, label) in [
        (COMPRESSED, "Compressed"),
        (DOCUMENTS, "Documents"),
        (MUSIC, "Music"),
        (VIDEO, "Video"),
        (PROGRAMS, "Programs"),
    ] {
        if set.contains(&ext.as_str()) {
            return Some(label);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_list_matches_common_types() {
        let pats = parse_list(DEFAULT_FILE_TYPES);
        assert!(matches("file.ZIP", &pats));
        assert!(matches("http://x/y/setup.exe", &pats));
        assert!(matches("movie.part1.rar", &pats));
        assert!(matches("archive.r03", &pats)); // R0* wildcard
        assert!(matches("archive.r12", &pats)); // R1* wildcard
        assert!(!matches("page.html", &pats));
        assert!(!matches("noext", &pats));
    }

    #[test]
    fn segment_extraction() {
        assert_eq!(url_last_segment("https://a/b/c/file%20name.zip?x=1"), "file name.zip");
        assert_eq!(url_last_segment("https://a/b/"), "");
    }
}
