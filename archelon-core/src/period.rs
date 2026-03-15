use chrono::{Datelike as _, Duration, NaiveDate, NaiveDateTime};

use crate::journal::WeekStart;

/// A time period used for filtering entries.
#[derive(Debug, Clone)]
pub enum Period {
    /// Inclusive datetime range [start, end].
    Range(NaiveDateTime, NaiveDateTime),
    /// Match entries where the field is absent (null / not set).
    None,
}

impl Period {
    /// Returns true if this period matches the given optional field value.
    ///
    /// - `Period::None` matches when `field` is `None`.
    /// - `Period::Range` matches when `field` is `Some(dt)` and `start <= dt <= end`.
    pub fn matches(&self, field: Option<NaiveDateTime>) -> bool {
        match self {
            Period::None => field.is_none(),
            Period::Range(start, end) => field.is_some_and(|dt| dt >= *start && dt <= *end),
        }
    }

    /// Like [`matches`] but for event overlap: true when the event [event_start, event_end]
    /// overlaps with this period. Either bound may be absent (treated as unbounded).
    pub fn overlaps_event(
        &self,
        event_start: Option<NaiveDateTime>,
        event_end: Option<NaiveDateTime>,
    ) -> bool {
        match self {
            Period::None => event_start.is_none() && event_end.is_none(),
            Period::Range(ps, pe) => {
                if event_start.is_none() && event_end.is_none() {
                    return false;
                }
                let after_period_end = event_start.is_some_and(|es| es > *pe);
                let before_period_start = event_end.is_some_and(|ee| ee < *ps);
                !after_period_end && !before_period_start
            }
        }
    }
}

/// Parse a datetime from `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM`, or `YYYY-MM-DDTHH:MM:SS`.
/// Date-only input is treated as start-of-day (00:00:00).
pub fn parse_datetime(s: &str) -> Result<NaiveDateTime, String> {
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Ok(dt);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
        return Ok(dt);
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(0, 0, 0).unwrap());
    }
    Err(format!("`{s}` is not a valid date/datetime — expected YYYY-MM-DD or YYYY-MM-DDTHH:MM"))
}

/// Like [`parse_datetime`] but date-only input is treated as end-of-day (23:59:59).
pub fn parse_datetime_end(s: &str) -> Result<NaiveDateTime, String> {
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Ok(dt);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
        return Ok(dt);
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(23, 59, 59).unwrap());
    }
    Err(format!("`{s}` is not a valid date/datetime — expected YYYY-MM-DD or YYYY-MM-DDTHH:MM"))
}

/// Parse a period string into a [`Period`].
///
/// Accepted formats:
/// - `none`                                  → [`Period::None`] (field is absent)
/// - `today`                                 → today 00:00:00 .. 23:59:59
/// - `this_week`                             → Monday (or Sunday) .. Saturday of the current week
/// - `this_month`                            → first .. last day of the current calendar month
/// - `YYYY-MM-DD`                            → that day 00:00:00 .. 23:59:59
/// - `YYYY-MM-DD,YYYY-MM-DD`                → start 00:00:00 .. end 23:59:59
/// - `YYYY-MM-DDTHH:MM,YYYY-MM-DDTHH:MM`   → exact datetime range (start inclusive, end inclusive)
pub fn parse_period(s: &str, week_start: WeekStart) -> Result<Period, String> {
    if s == "none" {
        return Ok(Period::None);
    }

    let today = chrono::Local::now().date_naive();

    if s == "today" {
        let start = today.and_hms_opt(0, 0, 0).unwrap();
        let end = today.and_hms_opt(23, 59, 59).unwrap();
        return Ok(Period::Range(start, end));
    }

    if s == "yesterday" {
        let d = today - Duration::days(1);
        let start = d.and_hms_opt(0, 0, 0).unwrap();
        let end = d.and_hms_opt(23, 59, 59).unwrap();
        return Ok(Period::Range(start, end));
    }

    if s == "tomorrow" {
        let d = today + Duration::days(1);
        let start = d.and_hms_opt(0, 0, 0).unwrap();
        let end = d.and_hms_opt(23, 59, 59).unwrap();
        return Ok(Period::Range(start, end));
    }

    if s == "this_week" {
        let days_back = match week_start {
            WeekStart::Monday => today.weekday().num_days_from_monday(),
            WeekStart::Sunday => today.weekday().num_days_from_sunday(),
        };
        let week_start_date = today - Duration::days(days_back as i64);
        let week_end_date = week_start_date + Duration::days(6);
        let start = week_start_date.and_hms_opt(0, 0, 0).unwrap();
        let end = week_end_date.and_hms_opt(23, 59, 59).unwrap();
        return Ok(Period::Range(start, end));
    }

    if s == "last_week" {
        let d = today - Duration::days(7);
        let days_back = match week_start {
            WeekStart::Monday => d.weekday().num_days_from_monday(),
            WeekStart::Sunday => d.weekday().num_days_from_sunday(),
        };
        let start_date = d - Duration::days(days_back as i64);
        let end_date = start_date + Duration::days(6);
        let start = start_date.and_hms_opt(0, 0, 0).unwrap();
        let end = end_date.and_hms_opt(23, 59, 59).unwrap();
        return Ok(Period::Range(start, end));
    }

    if s == "next_week" {
        let d = today + Duration::days(7);
        let days_back = match week_start {
            WeekStart::Monday => d.weekday().num_days_from_monday(),
            WeekStart::Sunday => d.weekday().num_days_from_sunday(),
        };
        let start_date = d - Duration::days(days_back as i64);
        let end_date = start_date + Duration::days(6);
        let start = start_date.and_hms_opt(0, 0, 0).unwrap();
        let end = end_date.and_hms_opt(23, 59, 59).unwrap();
        return Ok(Period::Range(start, end));
    }

    if s == "this_month" {
        let month_start = today.with_day(1).unwrap();
        let next_month = NaiveDate::from_ymd_opt(
            if today.month() == 12 { today.year() + 1 } else { today.year() },
            if today.month() == 12 { 1 } else { today.month() + 1 },
            1,
        )
        .unwrap();
        let month_end = next_month - Duration::days(1);
        let start = month_start.and_hms_opt(0, 0, 0).unwrap();
        let end = month_end.and_hms_opt(23, 59, 59).unwrap();
        return Ok(Period::Range(start, end));
    }

    if s == "last_month" {
        let (year, month) = if today.month() == 1 {
            (today.year() - 1, 12u32)
        } else {
            (today.year(), today.month() - 1)
        };
        let start_date = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
        let end_date = today.with_day(1).unwrap() - Duration::days(1);
        let start = start_date.and_hms_opt(0, 0, 0).unwrap();
        let end = end_date.and_hms_opt(23, 59, 59).unwrap();
        return Ok(Period::Range(start, end));
    }

    if s == "next_month" {
        let (year, month) = if today.month() == 12 {
            (today.year() + 1, 1u32)
        } else {
            (today.year(), today.month() + 1)
        };
        let start_date = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
        let (ey, em) = if month == 12 { (year + 1, 1u32) } else { (year, month + 1) };
        let end_date = NaiveDate::from_ymd_opt(ey, em, 1).unwrap() - Duration::days(1);
        let start = start_date.and_hms_opt(0, 0, 0).unwrap();
        let end = end_date.and_hms_opt(23, 59, 59).unwrap();
        return Ok(Period::Range(start, end));
    }

    // Comma-separated range (e.g. "2026-03-01,2026-03-07" or "2026-03-01T09:00,2026-03-01T17:00")
    if let Some((left, right)) = s.split_once(',') {
        let start = parse_datetime(left).map_err(|_| {
            format!("`{left}` is not a valid date/datetime in period `{s}`")
        })?;
        let end = parse_datetime_end(right).map_err(|_| {
            format!("`{right}` is not a valid date/datetime in period `{s}`")
        })?;
        return Ok(Period::Range(start, end));
    }

    // Single date
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let start = d.and_hms_opt(0, 0, 0).unwrap();
        let end = d.and_hms_opt(23, 59, 59).unwrap();
        return Ok(Period::Range(start, end));
    }

    Err(format!(
        "`{s}` is not a valid period — accepted: \
         none | today | yesterday | tomorrow | \
         this_week | last_week | next_week | \
         this_month | last_month | next_month | \
         YYYY-MM-DD | YYYY-MM-DD,YYYY-MM-DD | \
         YYYY-MM-DDTHH:MM,YYYY-MM-DDTHH:MM"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").unwrap()
    }

    #[test]
    fn parse_single_date() {
        let p = parse_period("2026-03-05", WeekStart::Monday).unwrap();
        let Period::Range(s, e) = p else { panic!() };
        assert_eq!(s, dt("2026-03-05T00:00:00"));
        assert_eq!(e, dt("2026-03-05T23:59:59"));
    }

    #[test]
    fn parse_date_range() {
        let p = parse_period("2026-03-01,2026-03-07", WeekStart::Monday).unwrap();
        let Period::Range(s, e) = p else { panic!() };
        assert_eq!(s, dt("2026-03-01T00:00:00"));
        assert_eq!(e, dt("2026-03-07T23:59:59"));
    }

    #[test]
    fn parse_datetime_range() {
        let p = parse_period("2026-03-01T09:00,2026-03-01T17:30", WeekStart::Monday).unwrap();
        let Period::Range(s, e) = p else { panic!() };
        assert_eq!(s, dt("2026-03-01T09:00:00"));
        assert_eq!(e, dt("2026-03-01T17:30:00"));
    }

    #[test]
    fn parse_none() {
        let p = parse_period("none", WeekStart::Monday).unwrap();
        assert!(matches!(p, Period::None));
    }

    #[test]
    fn period_none_matches_absent() {
        assert!(Period::None.matches(None));
        assert!(!Period::None.matches(Some(dt("2026-03-05T10:00:00"))));
    }

    #[test]
    fn period_range_matches_inclusive() {
        let p = Period::Range(dt("2026-03-05T00:00:00"), dt("2026-03-05T23:59:59"));
        assert!(p.matches(Some(dt("2026-03-05T12:00:00"))));
        assert!(!p.matches(Some(dt("2026-03-06T00:00:00"))));
        assert!(!p.matches(None));
    }

    #[test]
    fn overlaps_event_no_event_never_matches() {
        // Entry has no event at all — must not match any Range period.
        let p = Period::Range(dt("2026-03-08T00:00:00"), dt("2026-03-08T23:59:59"));
        assert!(!p.overlaps_event(None, None));
    }

    #[test]
    fn overlaps_event_spanning_period() {
        // Event spans the entire month; a single-day period inside it must match.
        let p = Period::Range(dt("2026-03-08T00:00:00"), dt("2026-03-08T23:59:59"));
        assert!(p.overlaps_event(
            Some(dt("2026-03-01T00:00:00")),
            Some(dt("2026-03-31T23:59:59")),
        ));
    }

    #[test]
    fn overlaps_event_outside_period() {
        // Event is entirely after the period.
        let p = Period::Range(dt("2026-03-08T00:00:00"), dt("2026-03-08T23:59:59"));
        assert!(!p.overlaps_event(
            Some(dt("2026-03-10T00:00:00")),
            Some(dt("2026-03-20T23:59:59")),
        ));
    }
}
