//! Formatação de valores de cota para exibição (padrão do painel de referência).

use super::usage::UsageWindow;

/// Normaliza `reset_at` para segundos (aceita epoch em ms como defensivo).
pub fn normalize_reset_epoch(reset_at: i64) -> i64 {
    if reset_at > 4_000_000_000 {
        reset_at / 1000
    } else {
        reset_at
    }
}

/// Percentual restante (100 - usado), inteiro, piso, limitado a 0..=100.
pub fn remaining_percent(used_percent: f64) -> i64 {
    let remaining = 100.0 - used_percent;
    if remaining <= 0.0 {
        0
    } else if remaining >= 100.0 {
        100
    } else {
        remaining.floor() as i64
    }
}

/// Texto da janela: "77% (21:10)", "39% (17:13 em 24/09)", "100%" ou "–".
pub fn format_window_at(window: Option<&UsageWindow>, now: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(w) = window else {
        return "–".to_string();
    };
    let Some(resets_at) = w.resets_at else {
        return "–".to_string();
    };
    let resets_at = normalize_reset_epoch(resets_at);
    if now >= resets_at {
        return "100%".to_string();
    }
    let pct = remaining_percent(w.used_percent);
    match (
        Local.timestamp_opt(resets_at, 0).single(),
        Local.timestamp_opt(now, 0).single(),
    ) {
        (Some(reset_dt), Some(now_dt)) => {
            if reset_dt.date_naive() == now_dt.date_naive() {
                format!("{pct}% ({})", reset_dt.format("%H:%M"))
            } else {
                format!(
                    "{pct}% ({} em {})",
                    reset_dt.format("%H:%M"),
                    reset_dt.format("%d/%m")
                )
            }
        }
        (Some(reset_dt), None) => format!("{pct}% ({})", reset_dt.format("%H:%M")),
        _ => format!("{pct}%"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn window(used: f64, resets_at: Option<i64>) -> UsageWindow {
        UsageWindow {
            used_percent: used,
            window_minutes: Some(300),
            resets_at,
        }
    }

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .unwrap()
            .timestamp()
    }

    #[test]
    fn remaining_percent_floors_and_clamps() {
        assert_eq!(remaining_percent(23.0), 77);
        assert_eq!(remaining_percent(22.6), 77);
        assert_eq!(remaining_percent(100.0), 0);
        assert_eq!(remaining_percent(130.0), 0);
        assert_eq!(remaining_percent(-5.0), 100);
    }

    #[test]
    fn formats_same_day_with_time_only() {
        let now = ts(2026, 9, 18, 20, 0);
        let reset = ts(2026, 9, 18, 21, 10);
        assert_eq!(
            format_window_at(Some(&window(23.0, Some(reset))), now),
            "77% (21:10)"
        );
    }

    #[test]
    fn formats_other_day_with_date() {
        let now = ts(2026, 9, 21, 20, 0);
        let reset = ts(2026, 9, 24, 17, 13);
        assert_eq!(
            format_window_at(Some(&window(61.0, Some(reset))), now),
            "39% (17:13 em 24/09)"
        );
    }

    #[test]
    fn expired_reset_reads_as_100() {
        let now = ts(2026, 9, 18, 20, 0);
        assert_eq!(
            format_window_at(Some(&window(80.0, Some(now - 10))), now),
            "100%"
        );
    }

    #[test]
    fn missing_window_or_reset_is_dash() {
        let now = ts(2026, 9, 18, 20, 0);
        assert_eq!(format_window_at(None, now), "–");
        assert_eq!(format_window_at(Some(&window(10.0, None)), now), "–");
    }

    #[test]
    fn reset_in_millis_is_normalized() {
        let now = ts(2026, 9, 18, 20, 0);
        let reset = ts(2026, 9, 18, 21, 10) * 1000;
        assert_eq!(
            format_window_at(Some(&window(23.0, Some(reset))), now),
            "77% (21:10)"
        );
    }
}
