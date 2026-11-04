//! Retention dates (PHASES P07 step 1). Pure: no catalog, no clock (callers pass "today").
//!
//! A class keeps documents `years` after its anchor:
//! - `tax_year_end`: December 31 of the document's tax year;
//! - `engagement_close`: no engagement data exists in v1, so the tax-year end as well
//!   (STATE assumption, 2026-11-03);
//! - `filed_at`: the day the document was filed;
//! - `never`: kept for good.
//!
//! A dated anchor with no tax year falls back to the filing date. A document is eligible for
//! disposition from the day after its retention ends.

use time::{Date, Month};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    TaxYearEnd,
    EngagementClose,
    FiledAt,
    Never,
}

impl Anchor {
    /// The catalog's `retention_class.anchor` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Anchor::TaxYearEnd => "tax_year_end",
            Anchor::EngagementClose => "engagement_close",
            Anchor::FiledAt => "filed_at",
            Anchor::Never => "never",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "tax_year_end" => Anchor::TaxYearEnd,
            "engagement_close" => Anchor::EngagementClose,
            "filed_at" => Anchor::FiledAt,
            "never" => Anchor::Never,
            _ => return None,
        })
    }

    /// In words, for the Admin page and the Safeguards appendix.
    pub fn describe(self) -> &'static str {
        match self {
            Anchor::TaxYearEnd => "from the end of the tax year",
            Anchor::EngagementClose => "from the engagement's close (its tax-year end in v1)",
            Anchor::FiledAt => "from the day it was filed",
            Anchor::Never => "kept permanently",
        }
    }
}

/// `filed_at` (RFC 3339) as a date.
fn date_of(rfc3339: &str) -> Option<Date> {
    let d = rfc3339.get(..10)?;
    let fmt = time::macros::format_description!("[year]-[month]-[day]");
    Date::parse(d, &fmt).ok()
}

/// `d` moved `years` later (February 29 becomes February 28).
fn plus_years(d: Date, years: i32) -> Option<Date> {
    let y = d.year() + years;
    Date::from_calendar_date(y, d.month(), d.day())
        .or_else(|_| Date::from_calendar_date(y, d.month(), d.day() - 1))
        .ok()
}

/// The first day a document may be disposed of, or `None` when it is kept for good (or has
/// nothing to date it by).
pub fn eligible_on(
    anchor: Anchor,
    years: Option<i32>,
    tax_year: Option<i32>,
    filed_at: &str,
) -> Option<Date> {
    let years = years?;
    let end = match (anchor, tax_year) {
        (Anchor::Never, _) => return None,
        (Anchor::TaxYearEnd | Anchor::EngagementClose, Some(y)) => {
            Date::from_calendar_date(y, Month::December, 31).ok()?
        }
        _ => date_of(filed_at)?,
    };
    plus_years(end, years)?.next_day()
}

/// Where a document stands on `today`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Its retention has ended.
    Eligible(Date),
    /// Kept until the day before this date.
    Kept(Date),
    /// Kept for good.
    Permanent,
}

pub fn standing(
    anchor: Anchor,
    years: Option<i32>,
    tax_year: Option<i32>,
    filed_at: &str,
    today: Date,
) -> Standing {
    match eligible_on(anchor, years, tax_year, filed_at) {
        None => Standing::Permanent,
        Some(d) if d <= today => Standing::Eligible(d),
        Some(d) => Standing::Kept(d),
    }
}

/// Today's date (UTC), for callers.
/// Today in the office's calendar (local time: a US evening is still today, not tomorrow's UTC
/// date; QA P16).
pub fn today() -> Date {
    time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .date()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), day).unwrap()
    }

    #[test]
    fn dates_by_anchor() {
        // A 2018 return kept 7 years after the tax year ends: through 2025-12-31.
        assert_eq!(
            eligible_on(
                Anchor::TaxYearEnd,
                Some(7),
                Some(2018),
                "2019-03-01T00:00:00Z"
            ),
            Some(d(2026, 1, 1))
        );
        // No tax year: from the filing date.
        assert_eq!(
            eligible_on(Anchor::TaxYearEnd, Some(4), None, "2020-02-29T10:00:00Z"),
            Some(d(2024, 3, 1))
        );
        assert_eq!(
            eligible_on(Anchor::FiledAt, Some(4), Some(2010), "2021-02-28T00:00:00Z"),
            Some(d(2025, 3, 1))
        );
        assert_eq!(
            eligible_on(Anchor::EngagementClose, Some(7), Some(2019), "t"),
            Some(d(2027, 1, 1))
        );
        assert_eq!(eligible_on(Anchor::Never, None, Some(2001), "t"), None);
        assert_eq!(eligible_on(Anchor::FiledAt, Some(4), None, "garbage"), None);
    }

    #[test]
    fn standing_on_a_day() {
        let today = d(2026, 11, 3);
        let s = |y| standing(Anchor::TaxYearEnd, Some(7), Some(y), "t", today);
        assert_eq!(s(2018), Standing::Eligible(d(2026, 1, 1)));
        assert_eq!(s(2019), Standing::Kept(d(2027, 1, 1)));
        assert_eq!(
            standing(Anchor::Never, None, None, "t", today),
            Standing::Permanent
        );
        for a in [
            Anchor::TaxYearEnd,
            Anchor::EngagementClose,
            Anchor::FiledAt,
            Anchor::Never,
        ] {
            assert_eq!(Anchor::parse(a.as_str()), Some(a));
        }
    }
}
