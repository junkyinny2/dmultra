//! Batch download URL expansion (IDM-style wildcards).
//!
//! Supports:
//! - Numeric ranges: `file[01-20].zip`, `img[1-5].png`
//! - Alternation: `file.{jpg,png}` or `{a,b,c}`
//! - Combined: `dir/file[01-03].{jpg,png}`

pub fn expand_urls(pattern: &str) -> Vec<String> {
    // Alternation first.
    if let Some((prefix, rest)) = split_first_brace(pattern) {
        let mut out = Vec::new();
        for alt in split_alternatives(rest) {
            // rest contains the closing brace; reconstruct.
            let full = format!("{prefix}{alt}");
            out.extend(expand_urls(&full));
        }
        if !out.is_empty() {
            return out;
        }
    }
    // Numeric range in square brackets.
    if let Some((prefix, range, suffix)) = split_range(pattern) {
        let (start, end, width) = parse_range(&range);
        let mut out = Vec::new();
        for n in start..=end {
            let name = format!("{}{:0width$}{}", prefix, n, suffix, width = width);
            out.push(name);
        }
        if !out.is_empty() {
            return out;
        }
    }
    vec![pattern.to_string()]
}

fn split_first_brace(s: &str) -> Option<(&str, &str)> {
    let idx = s.find('{')?;
    let prefix = &s[..idx];
    Some((prefix, &s[idx..]))
}

fn split_alternatives(s: &str) -> Vec<String> {
    // s starts with '{' ... '}' ...
    let close = match s.find('}') {
        Some(i) => i,
        None => return vec![s.to_string()],
    };
    let inner = &s[1..close];
    let tail = &s[close + 1..];
    inner
        .split(',')
        .map(|alt| format!("{}{}", alt, tail))
        .collect()
}

fn split_range(s: &str) -> Option<(String, String, String)> {
    let open = s.find('[')?;
    let close = s[open..].find(']')? + open;
    let prefix = s[..open].to_string();
    let range = s[open + 1..close].to_string();
    let suffix = s[close + 1..].to_string();
    Some((prefix, range, suffix))
}

fn parse_range(r: &str) -> (i64, i64, usize) {
    let parts: Vec<&str> = r.split('-').collect();
    if parts.len() == 2 {
        let width = parts[0].len().max(parts[1].len());
        let start = parts[0].trim().parse::<i64>().unwrap_or(0);
        let end = parts[1].trim().parse::<i64>().unwrap_or(start);
        (start, end, width)
    } else {
        (0, 0, 1)
    }
}
