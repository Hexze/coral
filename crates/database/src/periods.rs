use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Period {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "daily" => Some(Self::Daily),
            "weekly" => Some(Self::Weekly),
            "monthly" => Some(Self::Monthly),
            "yearly" => Some(Self::Yearly),
            _ => None,
        }
    }

    pub fn key(&self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Yearly => "yearly",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Daily => "Daily",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
            Self::Yearly => "Yearly",
        }
    }

    pub fn duration(&self) -> Duration {
        match self {
            Self::Daily => Duration::hours(24),
            Self::Weekly => Duration::days(7),
            Self::Monthly => Duration::days(30),
            Self::Yearly => Duration::days(365),
        }
    }

    pub fn staleness(&self) -> Duration {
        match self {
            Self::Daily => Duration::hours(1),
            Self::Weekly => Duration::hours(12),
            Self::Monthly => Duration::days(1),
            Self::Yearly => Duration::days(7),
        }
    }

    pub fn fixed_preset(&self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Daily => Some(("past_24h", "Past 24 Hours")),
            Self::Weekly => Some(("past_7d", "Past 7 Days")),
            Self::Monthly => Some(("past_30d", "Past 30 Days")),
            Self::Yearly => None,
        }
    }

    pub fn last_reset(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        let eastern_date = (now - Duration::hours(5)).date_naive();

        let reset_utc = |date: NaiveDate| -> DateTime<Utc> {
            let utc_hour = if is_eastern_dst(date) { 13 } else { 14 };
            DateTime::from_naive_utc_and_offset(date.and_hms_opt(utc_hour, 30, 0).unwrap(), Utc)
        };

        let candidate = match self {
            Self::Daily => eastern_date,
            Self::Weekly => {
                let dow = eastern_date.weekday().num_days_from_sunday();
                eastern_date - Duration::days(dow as i64)
            }
            Self::Monthly => {
                NaiveDate::from_ymd_opt(eastern_date.year(), eastern_date.month(), 1).unwrap()
            }
            Self::Yearly => NaiveDate::from_ymd_opt(eastern_date.year(), 1, 1).unwrap(),
        };

        let reset = reset_utc(candidate);
        if now >= reset {
            return reset;
        }

        let prev = match self {
            Self::Daily => candidate - Duration::days(1),
            Self::Weekly => candidate - Duration::days(7),
            Self::Monthly => {
                if candidate.month() == 1 {
                    NaiveDate::from_ymd_opt(candidate.year() - 1, 12, 1).unwrap()
                } else {
                    NaiveDate::from_ymd_opt(candidate.year(), candidate.month() - 1, 1).unwrap()
                }
            }
            Self::Yearly => NaiveDate::from_ymd_opt(candidate.year() - 1, 1, 1).unwrap(),
        };
        reset_utc(prev)
    }
}


fn is_eastern_dst(date: NaiveDate) -> bool {
    let year = date.year();
    let march_1 = NaiveDate::from_ymd_opt(year, 3, 1).unwrap();
    let dow = march_1.weekday().num_days_from_sunday();
    let spring = NaiveDate::from_ymd_opt(year, 3, 1 + (7 - dow) % 7 + 7).unwrap();

    let nov_1 = NaiveDate::from_ymd_opt(year, 11, 1).unwrap();
    let dow = nov_1.weekday().num_days_from_sunday();
    let fall = NaiveDate::from_ymd_opt(year, 11, 1 + (7 - dow) % 7).unwrap();

    date >= spring && date < fall
}
