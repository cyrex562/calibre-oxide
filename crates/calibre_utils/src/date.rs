use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use regex::Regex;
use lazy_static::lazy_static;

lazy_static! {
    static ref ISO_DATE_PATTERN: Regex = Regex::new(r"^\d{4}[/.-]\d{1,2}[/.-]\d{1,2}").unwrap();
}

pub const UNDEFINED_DATE_YEAR: i32 = 101; 

/// Parse a date string into a DateTime<Utc>.
/// 
/// `assume_utc`: if true, naive datetimes are assumed to be UTC. Otherwise Local.
/// `as_utc`: if true, return converted to UTC.
pub fn parse_date(date_string: &str, assume_utc: bool) -> Option<DateTime<Utc>> {
    if date_string.is_empty() {
        return None;
    }
    
    // Check for "undefined" date equivalent manually if needed
    // For now, try parsing standard formats.

    // Try RFC3339 / ISO8601 first
    if let Ok(dt) = DateTime::parse_from_rfc3339(date_string) {
        return Some(dt.with_timezone(&Utc));
    }
    
    // Try naive formats
    let formats = [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d",
        "%Y/%m/%d",
    ];

    for fmt in formats {
        if let Ok(naive) = NaiveDateTime::parse_from_str(date_string, fmt) {
            // If parsed as naive, apply logic
            return Some(naive_to_utc(naive, assume_utc));
        }
        // Try parsing just date
         if let Ok(naive_date) = chrono::NaiveDate::parse_from_str(date_string, fmt) {
             let naive = naive_date.and_hms_opt(0,0,0).unwrap();
             return Some(naive_to_utc(naive, assume_utc));
         }
    }
    
    None
}

fn naive_to_utc(naive: NaiveDateTime, assume_utc: bool) -> DateTime<Utc> {
    if assume_utc {
        DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
    } else {
        // Local conversion is tricky without machine local timezone context.
        // For server/cli environment using Utc is safer, or explicitly Local.
        // We will use Local if available, but for reproducibility Utc might be better default for unknown.
        // Chrono Local:
        let local_dt = Local.from_local_datetime(&naive).single().unwrap_or_else(|| Local.from_utc_datetime(&naive));
        local_dt.with_timezone(&Utc)
    }
}

pub fn now() -> DateTime<Utc> {
    Utc::now()
}

pub fn utcnow() -> DateTime<Utc> {
    Utc::now()
}

pub fn isoformat(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Simple subset format implementation
/// Replaces yyyy, MM, dd, etc. 
pub fn format_date(dt: &DateTime<Utc>, format: &str) -> String {
    let mut s = format.to_string();
    
    // Simple replacements (not fully compliant with Qt/LDML but covers basics)
    // Order matters to avoid replacing subset strings
    let year = dt.year();
    let month = dt.month();
    let day = dt.day();
    let hour = dt.hour();
    let minute = dt.minute();
    let second = dt.second();

    // yyyy
    s = s.replace("yyyy", &format!("{:04}", year));
    s = s.replace("yy", &format!("{:02}", year % 100));
    
    // MM (01-12), M (1-12) -- handle MM first
    // Use regex to avoid replacing M in other words if possible? 
    // But format strings are usually specific.
    // For manual replacement verify pattern.
    
    // Actually using chrono strftime for parts is easier if we map format specifiers.
    // Qt: yyyy -> %Y, yy -> %y, MM -> %m, M -> %-m (linux)
    // dd -> %d, d -> %-d
    // hh -> %H, h -> %-H (or I/l for 12h)
    // mm -> %M
    // ss -> %S
    // ap/AP -> %p (ish)
    
    // Naive replacement:
    s = s.replace("yyyy", &format!("{:04}", year));
    s = s.replace("dd", &format!("{:02}", day));
    s = s.replace("MM", &format!("{:02}", month));
    s = s.replace("hh", &format!("{:02}", hour));
    s = s.replace("mm", &format!("{:02}", minute));
    s = s.replace("ss", &format!("{:02}", second));
    
    s
}

use chrono::Datelike;
use chrono::Timelike;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_iso() {
        let dt = parse_date("2023-01-01T12:00:00Z", true).unwrap();
        assert_eq!(dt.year(), 2023);
        assert_eq!(dt.month(), 1);
    }
    
    #[test]
    fn test_format() {
        let dt = parse_date("2023-05-04T01:02:03Z", true).unwrap();
        let s = format_date(&dt, "yyyy-MM-dd");
        assert_eq!(s, "2023-05-04");
    }
}
