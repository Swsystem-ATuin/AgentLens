use chrono::{DateTime, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

// Zero is the persisted/form sentinel for the explicitly selected "All" option.
pub(crate) const PAGE_SIZES: [usize; 4] = [0, 25, 50, 100];
pub(crate) const LIVE_INTERVALS: [u64; 4] = [5, 10, 30, 60];
pub(crate) const FONT_SCALES: [u16; 4] = [90, 100, 110, 125];
type TimezoneTransitions = Arc<Vec<(i64, String)>>;
type TimezoneCache = Mutex<HashMap<String, TimezoneTransitions>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UserPreferences {
    pub(crate) timezone_mode: String,
    pub(crate) timezone_name: String,
    pub(crate) date_time_format: String,
    pub(crate) appearance: String,
    pub(crate) font_scale: u16,
    pub(crate) page_size: usize,
    pub(crate) live_interval_seconds: u64,
    pub(crate) live_paused: bool,
    pub(crate) transcript_detail: String,
    pub(crate) code_wrap: bool,
    pub(crate) default_sessions_view: String,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            timezone_mode: "utc".into(),
            timezone_name: "UTC".into(),
            date_time_format: "iso".into(),
            appearance: "system".into(),
            font_scale: 100,
            page_size: 50,
            live_interval_seconds: 5,
            live_paused: true,
            transcript_detail: "standard".into(),
            code_wrap: false,
            default_sessions_view: "all".into(),
        }
    }
}

impl UserPreferences {
    pub(crate) fn display_page_size(&self) -> usize {
        if self.page_size == 0 {
            // SQLite limits and offsets are signed integers. Leave room for the lookahead
            // row, without allocating this amount: fetch/truncate only touch actual rows.
            (isize::MAX - 1) as usize
        } else {
            self.page_size
        }
    }

    pub(crate) fn display_page(&self, requested: u32) -> u32 {
        if self.page_size == 0 { 1 } else { requested }
    }

    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if !matches!(self.timezone_mode.as_str(), "utc" | "browser" | "iana") {
            return Err("invalid timezone mode");
        }
        if self.timezone_name.len() > 64 || self.timezone_name.chars().any(char::is_control) {
            return Err("timezone name is invalid");
        }
        if self.timezone_mode == "iana" && self.timezone_name.parse::<Tz>().is_err() {
            return Err("timezone must be a valid IANA name such as Europe/London");
        }
        if !matches!(
            self.date_time_format.as_str(),
            "iso" | "long-24" | "long-12"
        ) {
            return Err("invalid date and time format");
        }
        if !matches!(
            self.appearance.as_str(),
            "system" | "light" | "dark" | "high-contrast"
        ) {
            return Err("invalid appearance");
        }
        if !FONT_SCALES.contains(&self.font_scale) {
            return Err("invalid font scale");
        }
        if !PAGE_SIZES.contains(&self.page_size) {
            return Err("invalid page size");
        }
        if !LIVE_INTERVALS.contains(&self.live_interval_seconds) {
            return Err("invalid live-reader interval");
        }
        if !matches!(
            self.transcript_detail.as_str(),
            "compact" | "standard" | "expanded"
        ) {
            return Err("invalid transcript detail setting");
        }
        if !matches!(
            self.default_sessions_view.as_str(),
            "all" | "active" | "archived"
        ) {
            return Err("invalid default sessions view");
        }
        Ok(())
    }

    pub(crate) fn sanitized(mut self) -> Self {
        let defaults = Self::default();
        if !matches!(self.timezone_mode.as_str(), "utc" | "browser" | "iana") {
            self.timezone_mode = defaults.timezone_mode;
        }
        if self.timezone_name.len() > 64
            || self.timezone_name.chars().any(char::is_control)
            || self.timezone_name.parse::<Tz>().is_err()
        {
            self.timezone_name = defaults.timezone_name;
            if self.timezone_mode == "iana" {
                self.timezone_mode = "utc".into();
            }
        }
        if !matches!(
            self.date_time_format.as_str(),
            "iso" | "long-24" | "long-12"
        ) {
            self.date_time_format = defaults.date_time_format;
        }
        if !matches!(
            self.appearance.as_str(),
            "system" | "light" | "dark" | "high-contrast"
        ) {
            self.appearance = defaults.appearance;
        }
        if !FONT_SCALES.contains(&self.font_scale) {
            self.font_scale = defaults.font_scale;
        }
        if !PAGE_SIZES.contains(&self.page_size) {
            self.page_size = defaults.page_size;
        }
        if !LIVE_INTERVALS.contains(&self.live_interval_seconds) {
            self.live_interval_seconds = defaults.live_interval_seconds;
        }
        if !matches!(
            self.transcript_detail.as_str(),
            "compact" | "standard" | "expanded"
        ) {
            self.transcript_detail = defaults.transcript_detail;
        }
        if !matches!(
            self.default_sessions_view.as_str(),
            "all" | "active" | "archived"
        ) {
            self.default_sessions_view = defaults.default_sessions_view;
        }
        self
    }

    pub(crate) fn timezone_label(&self) -> &str {
        match self.timezone_mode.as_str() {
            "browser" => "browser local time (UTC without JavaScript)",
            "iana" => &self.timezone_name,
            _ => "UTC",
        }
    }

    pub(crate) fn sql_timezone_modifier(&self) -> String {
        match self.timezone_mode.as_str() {
            "iana" => format!("iana:{}", self.timezone_name),
            _ => "+00:00".into(),
        }
    }

    pub(crate) fn format_timestamp(&self, raw: &str) -> String {
        if self.timezone_mode == "utc" && self.date_time_format == "iso" {
            return raw.to_owned();
        }
        let timestamp = DateTime::parse_from_rfc3339(raw).map(|value| value.with_timezone(&Utc));
        let timestamp = match timestamp {
            Ok(timestamp) => timestamp,
            Err(_) => match NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f") {
                Ok(timestamp) => timestamp.and_utc(),
                Err(_) => return raw.to_owned(),
            },
        };
        if self.timezone_mode == "browser" {
            return raw.to_owned();
        }
        let formatted = if self.timezone_mode == "iana" {
            self.timezone_name
                .parse::<Tz>()
                .ok()
                .map(|timezone| format_datetime(timestamp.with_timezone(&timezone), self))
        } else {
            Some(format_datetime(timestamp.with_timezone(&Utc), self))
        };
        formatted.unwrap_or_else(|| raw.to_owned())
    }

    pub(crate) fn detail_open(&self, normal_default: bool) -> bool {
        match self.transcript_detail.as_str() {
            "compact" => false,
            "expanded" => true,
            _ => normal_default,
        }
    }
}

pub(crate) fn sqlite_timezone_expression(timezone: &str, timestamp: &str) -> String {
    let Some(name) = timezone.strip_prefix("iana:") else {
        return format!("'{}'", safe_fixed_offset(timezone));
    };
    let Ok(zone) = name.parse::<Tz>() else {
        return "'+00:00'".into();
    };
    let transitions = timezone_transitions(name, zone);
    let mut expression = String::from("CASE");
    for (start, offset) in transitions.iter().skip(1).rev() {
        expression.push_str(&format!(
            " WHEN unixepoch({timestamp})>={start} THEN '{offset}'"
        ));
    }
    let fallback = transitions
        .first()
        .map(|(_, offset)| offset.as_str())
        .unwrap_or("+00:00");
    expression.push_str(&format!(" ELSE '{fallback}' END"));
    expression
}

fn safe_fixed_offset(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() == 6
        && matches!(bytes[0], b'+' | b'-')
        && bytes[1..3].iter().all(u8::is_ascii_digit)
        && bytes[3] == b':'
        && bytes[4..6].iter().all(u8::is_ascii_digit)
    {
        value
    } else {
        "+00:00"
    }
}

fn timezone_transitions(name: &str, zone: Tz) -> TimezoneTransitions {
    static CACHE: OnceLock<TimezoneCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(transitions) = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(name)
    {
        return Arc::clone(transitions);
    }
    const START: i64 = -2_208_988_800;
    const END: i64 = 7_289_654_400;
    let start = START;
    let end = END;
    let mut transitions = vec![(start, timezone_offset(zone, start))];
    let mut previous_time = start;
    let mut previous_offset = transitions[0].1.clone();
    let mut time = start + 86_400;
    while time <= end {
        let offset = timezone_offset(zone, time);
        if offset != previous_offset {
            let mut low = previous_time;
            let mut high = time;
            while high - low > 1 {
                let middle = low + (high - low) / 2;
                if timezone_offset(zone, middle) == previous_offset {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            transitions.push((high, offset.clone()));
            previous_offset = offset;
        }
        previous_time = time;
        time += 86_400;
    }
    let transitions = Arc::new(transitions);
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(name.to_owned(), Arc::clone(&transitions));
    transitions
}

fn timezone_offset(zone: Tz, timestamp: i64) -> String {
    let Some(utc) = DateTime::from_timestamp(timestamp, 0).map(|value| value.naive_utc()) else {
        return "+00:00".into();
    };
    let seconds = zone.offset_from_utc_datetime(&utc).fix().local_minus_utc();
    let sign = if seconds < 0 { '-' } else { '+' };
    let absolute = seconds.unsigned_abs();
    format!("{sign}{:02}:{:02}", absolute / 3_600, absolute % 3_600 / 60)
}

fn format_datetime<TzValue: chrono::TimeZone>(
    timestamp: DateTime<TzValue>,
    preferences: &UserPreferences,
) -> String
where
    TzValue::Offset: std::fmt::Display,
{
    match preferences.date_time_format.as_str() {
        "long-24" => timestamp.format("%d %b %Y %H:%M:%S %Z").to_string(),
        "long-12" => timestamp.format("%d %b %Y %I:%M:%S %p %Z").to_string(),
        _ => timestamp.format("%Y-%m-%d %H:%M:%S %:z").to_string(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn all_rows_is_explicit_persistent_and_uses_safe_nonzero_display_arithmetic() {
        let all = super::UserPreferences {
            page_size: 0,
            ..super::UserPreferences::default()
        };
        assert!(all.validate().is_ok());
        assert_eq!(all.clone().sanitized().page_size, 0);
        assert_eq!(all.display_page(u32::MAX), 1);
        assert!(all.display_page_size() > 100);
        assert!(i64::try_from(all.display_page_size() + 1).is_ok());
        for size in [25, 50, 100] {
            let prefs = super::UserPreferences {
                page_size: size,
                ..super::UserPreferences::default()
            };
            assert_eq!(prefs.display_page_size(), size);
            assert_eq!(prefs.display_page(2), 2);
        }
    }
    use super::*;

    #[test]
    fn validates_and_sanitizes_typed_preferences() {
        let mut preferences = UserPreferences {
            timezone_mode: "iana".into(),
            timezone_name: "Europe/London".into(),
            ..UserPreferences::default()
        };
        assert!(preferences.validate().is_ok());
        preferences.page_size = 49;
        assert_eq!(preferences.validate(), Err("invalid page size"));
        preferences.timezone_name = "not/a-zone".into();
        preferences.timezone_mode = "future-mode".into();
        let sanitized = preferences.sanitized();
        assert_eq!(sanitized.timezone_mode, "utc");
        assert_eq!(sanitized.timezone_name, "UTC");
        assert_eq!(sanitized.page_size, 50);
    }

    #[test]
    fn formats_iana_time_with_daylight_saving_evidence() {
        let preferences = UserPreferences {
            timezone_mode: "iana".into(),
            timezone_name: "Europe/London".into(),
            ..UserPreferences::default()
        };
        assert_eq!(
            preferences.format_timestamp("2026-07-01T12:00:00Z"),
            "2026-07-01 13:00:00 +01:00"
        );
        assert_eq!(
            preferences.format_timestamp("2026-01-01T12:00:00Z"),
            "2026-01-01 12:00:00 +00:00"
        );
    }

    #[tokio::test]
    async fn sqlite_timezone_expression_uses_historical_iana_offsets() {
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        for (timestamp, expected) in [
            ("2026-01-31T23:30:00Z", "2026-01-31"),
            ("2026-06-30T23:30:00Z", "2026-07-01"),
        ] {
            let expression =
                sqlite_timezone_expression("iana:Europe/London", &format!("'{timestamp}'"));
            let sql = format!("SELECT date('{timestamp}',{expression})");
            let actual: String = sqlx::query_scalar(&sql).fetch_one(&pool).await.unwrap();
            assert_eq!(actual, expected);
        }
    }
}
