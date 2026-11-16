//! Wall-clock real-time clock (RTC) support and pure civil-calendar math.
//!
//! At boot, before `exit_boot_services`, the kernel reads the wall clock once
//! through UEFI `GetTime` (falling back to a direct CMOS `0x70`/`0x71` read if
//! firmware `GetTime` fails) and records the corresponding Unix timestamp
//! alongside `clock::ticks()`. Every subsequent `date` command or `date()`
//! WISP builtin adds the elapsed monotonic ticks to that baseline, optionally
//! shifted by the `sys.tz` offset (in minutes) stored on the save disk.

use alloc::format;
use alloc::string::String;

/// Civil calendar date and time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: i64,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

pub const fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

pub const fn days_in_month(year: i64, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Convert a proleptic Gregorian `(year, month, day)` into days since
/// `1970-01-01` (Howard Hinnant's civil calendar algorithm).
pub const fn days_from_civil(year: i64, month: u8, day: u8) -> i64 {
    let m = month as i64;
    let d = day as i64;
    let y = if m <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Convert days since `1970-01-01` into proleptic Gregorian `(year, month, day)`.
pub const fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u8, d as u8)
}

impl DateTime {
    pub fn is_valid(&self) -> bool {
        if !(1..=12).contains(&self.month) {
            return false;
        }
        let max_day = days_in_month(self.year, self.month);
        if self.day < 1 || self.day > max_day {
            return false;
        }
        self.hour <= 23 && self.minute <= 59 && self.second <= 60
    }

    pub fn to_unix_seconds(&self) -> Option<i64> {
        if !self.is_valid() {
            return None;
        }
        let sec = self.second.min(59) as i64;
        let days = days_from_civil(self.year, self.month, self.day);
        days.checked_mul(86_400)?
            .checked_add((self.hour as i64) * 3600 + (self.minute as i64) * 60 + sec)
    }

    pub fn from_unix_seconds(secs: i64) -> Self {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        let hour = (rem / 3600) as u8;
        let minute = ((rem % 3600) / 60) as u8;
        let second = (rem % 60) as u8;
        Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        }
    }

    pub fn format_iso(&self, tz_offset_minutes: i64) -> String {
        if tz_offset_minutes == 0 {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                self.year, self.month, self.day, self.hour, self.minute, self.second
            )
        } else {
            let sign = if tz_offset_minutes >= 0 { '+' } else { '-' };
            let abs = tz_offset_minutes.unsigned_abs();
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}{:02}:{:02}",
                self.year,
                self.month,
                self.day,
                self.hour,
                self.minute,
                self.second,
                sign,
                abs / 60,
                abs % 60
            )
        }
    }
}

#[cfg(target_os = "uefi")]
mod hw {
    use super::DateTime;
    use core::arch::asm;
    use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
    use uefi::table::{Boot, SystemTable};

    use crate::{clock, debugcon};

    static RTC_VALID: AtomicBool = AtomicBool::new(false);
    static BOOT_UNIX_SECS: AtomicI64 = AtomicI64::new(0);
    static BOOT_TICKS: AtomicU64 = AtomicU64::new(0);

    const CMOS_ADDR: u16 = 0x70;
    const CMOS_DATA: u16 = 0x71;

    #[inline]
    fn out8(port: u16, val: u8) {
        unsafe {
            asm!(
                "out dx, al",
                in("dx") port,
                in("al") val,
                options(nomem, nostack, preserves_flags),
            );
        }
    }

    #[inline]
    fn in8(port: u16) -> u8 {
        let val: u8;
        unsafe {
            asm!(
                "in al, dx",
                in("dx") port,
                out("al") val,
                options(nomem, nostack, preserves_flags),
            );
        }
        val
    }

    fn cmos_read(reg: u8) -> u8 {
        out8(CMOS_ADDR, reg);
        in8(CMOS_DATA)
    }

    fn cmos_uip() -> bool {
        cmos_read(0x0A) & 0x80 != 0
    }

    fn bcd_to_bin(val: u8) -> u8 {
        (val & 0x0F) + ((val >> 4) * 10)
    }

    fn read_cmos_once() -> (u8, u8, u8, u8, u8, u8, u8, u8) {
        for _ in 0..10_000 {
            if !cmos_uip() {
                break;
            }
            core::hint::spin_loop();
        }
        (
            cmos_read(0x00),
            cmos_read(0x02),
            cmos_read(0x04),
            cmos_read(0x07),
            cmos_read(0x08),
            cmos_read(0x09),
            cmos_read(0x32),
            cmos_read(0x0B),
        )
    }

    fn read_cmos_datetime() -> Option<DateTime> {
        let mut prev = read_cmos_once();
        for _ in 0..16 {
            let next = read_cmos_once();
            if next == prev && !cmos_uip() {
                let (mut sec, mut min, mut hour, mut day, mut month, mut year, mut century, reg_b) =
                    next;
                let is_binary = reg_b & 0x04 != 0;
                let is_24h = reg_b & 0x02 != 0;
                let pm = !is_24h && (hour & 0x80 != 0);
                hour &= 0x7F;
                if !is_binary {
                    sec = bcd_to_bin(sec);
                    min = bcd_to_bin(min);
                    hour = bcd_to_bin(hour);
                    day = bcd_to_bin(day);
                    month = bcd_to_bin(month);
                    year = bcd_to_bin(year);
                    century = bcd_to_bin(century);
                }
                if !is_24h {
                    if pm {
                        if hour < 12 {
                            hour = hour.saturating_add(12);
                        }
                    } else if hour == 12 {
                        hour = 0;
                    }
                }
                let full_year = if (19..=22).contains(&century) {
                    (century as i64) * 100 + (year as i64)
                } else {
                    2000 + (year as i64)
                };
                let dt = DateTime {
                    year: full_year,
                    month,
                    day,
                    hour,
                    minute: min,
                    second: sec,
                };
                if dt.is_valid() {
                    return Some(dt);
                }
                return None;
            }
            prev = next;
        }
        None
    }

    /// Capture the boot-time wall clock before `exit_boot_services`, using
    /// UEFI `GetTime` with a CMOS RTC fallback.
    pub fn init(st: &SystemTable<Boot>) {
        let uefi_dt = st.runtime_services().get_time().ok().and_then(|t| {
            let dt = DateTime {
                year: i64::from(t.year()),
                month: t.month(),
                day: t.day(),
                hour: t.hour(),
                minute: t.minute(),
                second: t.second().min(59),
            };
            dt.is_valid().then_some(dt)
        });

        let (source, dt_opt) = match uefi_dt {
            Some(dt) => ("UEFI GetTime", Some(dt)),
            None => ("CMOS RTC", read_cmos_datetime()),
        };

        if let Some(dt) = dt_opt {
            if let Some(unix_secs) = dt.to_unix_seconds() {
                BOOT_UNIX_SECS.store(unix_secs, Ordering::Relaxed);
                BOOT_TICKS.store(clock::ticks(), Ordering::Relaxed);
                RTC_VALID.store(true, Ordering::Release);
                let _ = debugcon::write_fmt(format_args!(
                    "[rtc] initialized from {source}: {}\n",
                    dt.format_iso(0)
                ));
                return;
            }
        }
        debugcon::write_str("[rtc] warning: neither UEFI GetTime nor CMOS RTC returned a valid date\n");
    }

    /// Current wall-clock [`DateTime`] shifted by `tz_offset_minutes`.
    pub fn now(tz_offset_minutes: i64) -> Option<DateTime> {
        if !RTC_VALID.load(Ordering::Acquire) {
            if let Some(dt) = read_cmos_datetime() {
                if let Some(unix_secs) = dt.to_unix_seconds() {
                    BOOT_UNIX_SECS.store(unix_secs, Ordering::Relaxed);
                    BOOT_TICKS.store(clock::ticks(), Ordering::Relaxed);
                    RTC_VALID.store(true, Ordering::Release);
                }
            }
        }
        if !RTC_VALID.load(Ordering::Acquire) {
            return None;
        }
        let base_secs = BOOT_UNIX_SECS.load(Ordering::Relaxed);
        let base_ticks = BOOT_TICKS.load(Ordering::Relaxed);
        let elapsed_ms = clock::ticks()
            .saturating_sub(base_ticks)
            .saturating_mul(clock::MS_PER_TICK);
        let elapsed_secs = (elapsed_ms / 1000) as i64;
        let shifted = base_secs
            .saturating_add(elapsed_secs)
            .saturating_add(tz_offset_minutes.saturating_mul(60));
        Some(DateTime::from_unix_seconds(shifted))
    }
}

#[cfg(target_os = "uefi")]
pub use hw::{init, now};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch_and_leap_years_round_trip() {
        let epoch = DateTime {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        };
        assert_eq!(epoch.to_unix_seconds(), Some(0));
        assert_eq!(DateTime::from_unix_seconds(0), epoch);

        // 2000 is a leap year (divisible by 400); 2100 is not (divisible by 100).
        assert!(is_leap_year(2000));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2100));

        let feb29 = DateTime {
            year: 2024,
            month: 2,
            day: 29,
            hour: 23,
            minute: 59,
            second: 59,
        };
        let secs = feb29.to_unix_seconds().unwrap();
        assert_eq!(DateTime::from_unix_seconds(secs), feb29);
        assert_eq!(
            DateTime::from_unix_seconds(secs + 1),
            DateTime {
                year: 2024,
                month: 3,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            }
        );
    }

    #[test]
    fn year_rollover_and_iso_formatting() {
        let ny_eve = DateTime {
            year: 2026,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            second: 50,
        };
        let secs = ny_eve.to_unix_seconds().unwrap();
        let ny_day = DateTime::from_unix_seconds(secs + 15);
        assert_eq!(
            ny_day,
            DateTime {
                year: 2027,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 5,
            }
        );
        assert_eq!(ny_day.format_iso(0), "2027-01-01 00:00:05 UTC");
        assert_eq!(ny_day.format_iso(-240), "2027-01-01 00:00:05 -04:00");
        assert_eq!(ny_day.format_iso(330), "2027-01-01 00:00:05 +05:30");
    }
}
