use chrono::{DateTime, Datelike, NaiveDate, Utc};
use regex::Regex;
use serde_json::Value;
use std::{fs::File, io::BufReader, path::Path, sync::LazyLock};

static FILENAME_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"((?:19|20)\d{2})[-_.]?(\d{2})[-_.]?(\d{2})").unwrap());

pub fn valid_date(date: NaiveDate) -> bool {
    (1900..=Utc::now().year() + 1).contains(&date.year())
}

pub fn filename_date(name: &str) -> Option<NaiveDate> {
    for capture in FILENAME_DATE.captures_iter(name) {
        let found = capture.get(0)?;
        if name[..found.start()]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_digit())
        {
            continue;
        }
        let following_digits = name[found.end()..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        if !matches!(following_digits, 0 | 6 | 9) {
            continue;
        }
        let year = capture[1].parse().ok()?;
        let month = capture[2].parse().ok()?;
        let day = capture[3].parse().ok()?;
        if let Some(date) = NaiveDate::from_ymd_opt(year, month, day) {
            return Some(date);
        }
    }
    None
}

pub fn google_timestamp(metadata: &Value) -> Option<DateTime<Utc>> {
    let value = metadata.get("photoTakenTime")?.get("timestamp")?;
    let timestamp = value.as_i64().or_else(|| value.as_str()?.parse().ok())?;
    if timestamp <= 0 {
        return None;
    }
    let timestamp = DateTime::from_timestamp(timestamp, 0)?;
    valid_date(timestamp.date_naive()).then_some(timestamp)
}

pub fn google_date(metadata: &Value) -> Option<NaiveDate> {
    google_timestamp(metadata).map(|date| date.date_naive())
}

pub fn choose_date(name: &str, metadata: &Value) -> Option<NaiveDate> {
    google_date(metadata).or_else(|| filename_date(name))
}

pub fn embedded_date(path: &Path) -> Option<NaiveDate> {
    let file = File::open(path).ok()?;
    let exif = exif::Reader::new()
        .read_from_container(&mut BufReader::new(file))
        .ok()?;
    for tag in [exif::Tag::DateTimeOriginal, exif::Tag::DateTimeDigitized] {
        let Some(field) = exif.get_field(tag, exif::In::PRIMARY) else {
            continue;
        };
        if let exif::Value::Ascii(values) = &field.value {
            for bytes in values {
                let text = std::str::from_utf8(bytes).ok()?;
                if let Some(date) = text
                    .get(..10)
                    .and_then(|text| NaiveDate::parse_from_str(text, "%Y:%m:%d").ok())
                {
                    return Some(date);
                }
            }
        }
    }
    None
}

pub fn archive_date_parts(path: &str) -> (Option<String>, Option<String>) {
    let parts: Vec<_> = path.split('/').collect();
    for (index, year) in parts.iter().enumerate().take(parts.len().saturating_sub(2)) {
        let numeric = year.len() == 4
            && year
                .parse::<i32>()
                .is_ok_and(|year| (1900..=2099).contains(&year));
        if !numeric && !matches!(*year, "Senza data" | "Unknown year") {
            continue;
        }
        let remaining = &parts[index + 1..parts.len() - 1];
        let year = if *year == "Senza data" {
            "Unknown year"
        } else {
            year
        };
        let is_kind = |value: &str| matches!(value, "foto" | "video" | "photos" | "videos");
        if remaining.first().is_some_and(|value| is_kind(value)) {
            return (Some(year.to_string()), Some("Unknown month".to_string()));
        }
        if remaining.len() >= 2 && is_kind(remaining[1]) {
            let month = remaining[0];
            if month == "Unknown month"
                || (month.len() == 2
                    && month
                        .parse::<u32>()
                        .is_ok_and(|month| (1..=12).contains(&month)))
            {
                return (Some(year.to_string()), Some(month.to_string()));
            }
        }
    }
    (None, None)
}
