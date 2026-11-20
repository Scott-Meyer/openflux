//! Weather for the keyboard's weather tile. openfluxd looks up the tile's city with Open-Meteo
//! (open-meteo.com): its geocoding, forecast and air-quality APIs. Weather data by Open-Meteo.com,
//! licensed CC BY 4.0.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WeatherHourForecast {
    pub hour_label: String,
    pub temp: f64,
    pub condition: String,
    pub is_day: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WeatherSnapshot {
    pub city: String,
    pub temp: f64,
    pub min_temp: f64,
    pub max_temp: f64,
    pub condition: String,
    pub is_day: bool,
    pub celsius: bool,
    pub ready: bool,
    #[serde(default)]
    pub sunrise: String,
    #[serde(default)]
    pub sunset: String,
    #[serde(default)]
    pub aqi: Option<u32>,
    #[serde(default)]
    pub utc_offset_seconds: Option<i64>,
    #[serde(default)]
    pub hourly: Vec<WeatherHourForecast>,
}

/// The city a weather module shows until one is chosen, as in `assets/default_config.json`.
pub const DEFAULT_WEATHER_CITY: &str = "Denver";

#[derive(Clone, Default)]
struct SharedWeatherState {
    desired_city: String,
    desired_celsius: bool,
    generation: u64,
    snapshot: Option<WeatherSnapshot>,
    last_attempt: Option<Instant>,
    last_attempted_city: String,
    consecutive_failures: u32,
    in_flight: bool,
}

/// Fetches the weather for a city, in Celsius when the flag is set.
pub type WeatherFetcher = Arc<dyn Fn(&str, bool) -> Option<WeatherSnapshot> + Send + Sync>;

pub struct WeatherBridge {
    shared: Arc<Mutex<SharedWeatherState>>,
    fetcher: WeatherFetcher,
}

impl Default for WeatherBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl WeatherBridge {
    pub fn new() -> Self {
        Self::with_fetcher(Arc::new(fetch_open_meteo))
    }

    pub fn with_fetcher(fetcher: WeatherFetcher) -> Self {
        Self {
            shared: Arc::new(Mutex::new(SharedWeatherState::default())),
            fetcher,
        }
    }

    pub fn wmo_code_to_condition(code: i64, is_day: bool) -> String {
        match code {
            0 => {
                if is_day {
                    "Clear"
                } else {
                    "Clear Night"
                }
            }
            1 | 2 => "Partly Cloudy",
            3 => "Cloudy",
            45 | 48 => "Fog",
            51..=57 => "Drizzle",
            61..=67 | 80..=82 => "Rain",
            71..=77 | 85 | 86 => "Snow",
            95..=99 => "Thunderstorm",
            _ => "Cloudy",
        }
        .to_string()
    }

    /// Returns the latest weather for `city` in the requested unit without waiting. Until a fetch
    /// for exactly that city and unit has finished, it returns a `ready: false` snapshot so the
    /// keyboard doesn't show an old temperature or unit. Replies to superseded fetches are dropped.
    pub fn poll_nonblocking(&self, city: &str, celsius: bool) -> WeatherSnapshot {
        let city_clean = if city.trim().is_empty() {
            DEFAULT_WEATHER_CITY
        } else {
            city.trim()
        };
        let mut g = self.shared.lock().unwrap();
        let config_changed = g.desired_city != city_clean
            || g.desired_celsius != celsius
            || g.last_attempted_city.is_empty();
        if config_changed {
            g.desired_city = city_clean.to_string();
            g.desired_celsius = celsius;
            g.generation = g.generation.wrapping_add(1);
            g.snapshot = None;
            g.consecutive_failures = 0;
        }
        let retry_interval = if g.consecutive_failures > 0 {
            let exp = 60u64.saturating_mul(1u64 << g.consecutive_failures.saturating_sub(1).min(4));
            Duration::from_secs(exp.min(600))
        } else {
            Duration::from_secs(600)
        };
        let due = g
            .last_attempt
            .map(|t| t.elapsed() >= retry_interval)
            .unwrap_or(true);
        let need_fetch = config_changed || due;
        if need_fetch && (!g.in_flight || config_changed) {
            g.in_flight = true;
            g.last_attempt = Some(Instant::now());
            g.last_attempted_city = city_clean.to_string();
            let target_city = city_clean.to_string();
            let spawn_gen = g.generation;
            let shared_cl = self.shared.clone();
            let fetcher_cl = self.fetcher.clone();
            thread::spawn(move || {
                let fetched = (fetcher_cl)(&target_city, celsius);
                let mut sg = shared_cl.lock().unwrap();
                sg.in_flight = false;
                if sg.generation != spawn_gen {
                    // A newer city or unit change superseded this fetch; discard it.
                    return;
                }
                if let Some(snap) = fetched {
                    sg.consecutive_failures = 0;
                    sg.snapshot = Some(snap);
                } else {
                    sg.consecutive_failures = sg.consecutive_failures.saturating_add(1);
                }
            });
        }
        g.snapshot.clone().unwrap_or_else(|| WeatherSnapshot {
            city: city_clean.to_string(),
            temp: 0.0,
            min_temp: 0.0,
            max_temp: 0.0,
            condition: String::new(),
            is_day: true,
            celsius,
            ready: false,
            sunrise: String::new(),
            sunset: String::new(),
            aqi: None,
            utc_offset_seconds: None,
            hourly: Vec::new(),
        })
    }
}

fn format_iso_hhmm_12h(iso: &str) -> String {
    let time_part = iso.split('T').nth(1).unwrap_or(iso);
    let mut parts = time_part.split(':');
    let Ok(h24) = parts.next().unwrap_or("").parse::<u32>() else {
        return String::new();
    };
    let Ok(m) = parts.next().unwrap_or("0").parse::<u32>() else {
        return String::new();
    };
    let ampm = if h24 >= 12 { "PM" } else { "AM" };
    let h12 = if h24 % 12 == 0 { 12 } else { h24 % 12 };
    format!("{h12}:{m:02} {ampm}")
}

fn format_iso_hour_12h(iso: &str) -> String {
    let time_part = iso.split('T').nth(1).unwrap_or(iso);
    let Ok(h24) = time_part.split(':').next().unwrap_or("").parse::<u32>() else {
        return String::new();
    };
    let ampm = if h24 >= 12 { "PM" } else { "AM" };
    let h12 = if h24 % 12 == 0 { 12 } else { h24 % 12 };
    format!("{h12} {ampm}")
}

fn fetch_open_meteo(city: &str, celsius: bool) -> Option<WeatherSnapshot> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(6))
        .build()
        .ok()?;
    let geo_resp = client
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query(&[
            ("name", city),
            ("count", "1"),
            ("language", "en"),
            ("format", "json"),
        ])
        .send()
        .ok()?;
    if !geo_resp.status().is_success() {
        return None;
    }
    let geo: serde_json::Value = geo_resp.json().ok()?;
    let loc = geo.get("results")?.as_array()?.first()?;
    let lat = loc.get("latitude")?.as_f64()?;
    let lon = loc.get("longitude")?.as_f64()?;
    let resolved_name = loc
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(city)
        .to_string();
    let unit = if celsius { "celsius" } else { "fahrenheit" };
    let fc_resp = client
        .get("https://api.open-meteo.com/v1/forecast")
        .query(&[
            ("latitude", lat.to_string()),
            ("longitude", lon.to_string()),
            ("current", "temperature_2m,weather_code,is_day".into()),
            ("hourly", "temperature_2m,weather_code,is_day".into()),
            (
                "daily",
                "temperature_2m_max,temperature_2m_min,sunrise,sunset".into(),
            ),
            ("temperature_unit", unit.into()),
            ("timezone", "auto".into()),
            ("forecast_days", "2".into()),
        ])
        .send()
        .ok()?;
    if !fc_resp.status().is_success() {
        return None;
    }
    let fc: serde_json::Value = fc_resp.json().ok()?;
    let aqi = client
        .get("https://air-quality-api.open-meteo.com/v1/air-quality")
        .query(&[
            ("latitude", lat.to_string()),
            ("longitude", lon.to_string()),
            ("current", "us_aqi".into()),
        ])
        .timeout(Duration::from_secs(3))
        .send()
        .ok()
        .and_then(|r| {
            if r.status().is_success() {
                r.json::<serde_json::Value>().ok()
            } else {
                None
            }
        })
        .and_then(|v| v.get("current")?.get("us_aqi")?.as_u64())
        .map(|n| n as u32);

    parse_open_meteo_forecast(&fc, &resolved_name, celsius, aqi)
}

pub fn parse_open_meteo_forecast(
    fc: &serde_json::Value,
    resolved_name: &str,
    celsius: bool,
    aqi: Option<u32>,
) -> Option<WeatherSnapshot> {
    let cur = fc.get("current")?;
    let cur_time = cur.get("time").and_then(|v| v.as_str()).unwrap_or("");
    let temp = cur.get("temperature_2m")?.as_f64()?;
    let code = cur.get("weather_code")?.as_i64().unwrap_or(0);
    let is_day = cur.get("is_day").and_then(|v| v.as_i64()).unwrap_or(1) != 0;
    let utc_offset_seconds = fc.get("utc_offset_seconds").and_then(|v| v.as_i64());
    let daily = fc.get("daily");
    let max_t = daily
        .and_then(|d| d.get("temperature_2m_max"))
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_f64())
        .unwrap_or(temp);
    let min_t = daily
        .and_then(|d| d.get("temperature_2m_min"))
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_f64())
        .unwrap_or(temp);
    let sunrise = daily
        .and_then(|d| d.get("sunrise"))
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(format_iso_hhmm_12h)
        .unwrap_or_default();
    let sunset = daily
        .and_then(|d| d.get("sunset"))
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(format_iso_hhmm_12h)
        .unwrap_or_default();

    let mut hourly = Vec::new();
    if let Some(hr) = fc.get("hourly") {
        let times = hr.get("time").and_then(|v| v.as_array());
        let temps = hr.get("temperature_2m").and_then(|v| v.as_array());
        let codes = hr.get("weather_code").and_then(|v| v.as_array());
        let is_days = hr.get("is_day").and_then(|v| v.as_array());
        if let (Some(ts), Some(tps)) = (times, temps) {
            for (i, t_val) in ts.iter().enumerate() {
                let Some(t_str) = t_val.as_str() else {
                    continue;
                };
                if !cur_time.is_empty() && t_str <= cur_time {
                    continue;
                }
                let Some(htemp) = tps.get(i).and_then(|v| v.as_f64()) else {
                    continue;
                };
                let hcode = codes
                    .and_then(|c| c.get(i))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(code);
                let hday = is_days
                    .and_then(|d| d.get(i))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(1)
                    != 0;
                hourly.push(WeatherHourForecast {
                    hour_label: format_iso_hour_12h(t_str),
                    temp: htemp,
                    condition: WeatherBridge::wmo_code_to_condition(hcode, hday),
                    is_day: hday,
                });
                if hourly.len() >= 3 {
                    break;
                }
            }
        }
    }

    Some(WeatherSnapshot {
        city: resolved_name.to_string(),
        temp,
        min_temp: min_t,
        max_temp: max_t,
        condition: WeatherBridge::wmo_code_to_condition(code, is_day),
        is_day,
        celsius,
        ready: true,
        sunrise,
        sunset,
        aqi,
        utc_offset_seconds,
        hourly,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_wmo_codes_cleanly() {
        assert_eq!(WeatherBridge::wmo_code_to_condition(0, true), "Clear");
        assert_eq!(
            WeatherBridge::wmo_code_to_condition(0, false),
            "Clear Night"
        );
        assert_eq!(
            WeatherBridge::wmo_code_to_condition(2, true),
            "Partly Cloudy"
        );
        assert_eq!(WeatherBridge::wmo_code_to_condition(61, true), "Rain");
        assert_eq!(WeatherBridge::wmo_code_to_condition(73, true), "Snow");
        assert_eq!(
            WeatherBridge::wmo_code_to_condition(95, true),
            "Thunderstorm"
        );
    }

    #[test]
    fn failed_weather_fetch_backs_off_instead_of_looping_on_every_poll() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_cl = calls.clone();
        let wb = WeatherBridge::with_fetcher(Arc::new(move |_city, _c| {
            calls_cl.fetch_add(1, Ordering::SeqCst);
            None
        }));
        for _ in 0..8 {
            assert!(!wb.poll_nonblocking("BadCityXYZ", false).ready);
            thread::sleep(Duration::from_millis(15));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Changing city immediately attempts once for the new city
        assert!(!wb.poll_nonblocking("Denver", false).ready);
        thread::sleep(Duration::from_millis(20));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn switching_units_while_fetch_in_flight_clears_ready_and_discards_stale_unit() {
        use std::sync::Condvar;
        let f_entered = Arc::new((Mutex::new(false), Condvar::new()));
        let f_release = Arc::new((Mutex::new(false), Condvar::new()));
        let fe = f_entered.clone();
        let fr = f_release.clone();

        let wb = WeatherBridge::with_fetcher(Arc::new(move |city, celsius| {
            if !celsius {
                {
                    let (m, cv) = &*fe;
                    *m.lock().unwrap() = true;
                    cv.notify_all();
                }
                let (m, cv) = &*fr;
                let mut g = m.lock().unwrap();
                while !*g {
                    g = cv.wait(g).unwrap();
                }
            }
            Some(WeatherSnapshot {
                city: city.to_string(),
                temp: if celsius { 20.0 } else { 68.0 },
                min_temp: if celsius { 12.0 } else { 54.0 },
                max_temp: if celsius { 24.0 } else { 75.0 },
                condition: "Clear".into(),
                is_day: true,
                celsius,
                ready: true,
                sunrise: "6:45 AM".into(),
                sunset: "4:45 PM".into(),
                aqi: Some(12),
                utc_offset_seconds: Some(-25200),
                hourly: Vec::new(),
            })
        }));

        // Start Fahrenheit fetch (blocks in flight)
        let s0 = wb.poll_nonblocking("Denver", false);
        assert!(!s0.ready);
        {
            let (m, cv) = &*f_entered;
            let mut g = m.lock().unwrap();
            while !*g {
                g = cv.wait(g).unwrap();
            }
        }
        // Switch to Celsius while the Fahrenheit fetch is still in flight.
        let s1 = wb.poll_nonblocking("Denver", true);
        assert!(!s1.ready);
        assert!(s1.celsius);

        // The slow Fahrenheit result is discarded; the Celsius snapshot lands.
        {
            let (m, cv) = &*f_release;
            *m.lock().unwrap() = true;
            cv.notify_all();
        }
        let mut got_c = None;
        for _ in 0..40 {
            let s = wb.poll_nonblocking("Denver", true);
            if s.ready {
                got_c = Some(s);
                break;
            }
            thread::sleep(Duration::from_millis(15));
        }
        let c = got_c.expect("expected Celsius snapshot");
        assert!(c.celsius);
        assert!((c.temp - 20.0).abs() < 0.01);
    }

    #[test]
    fn parses_cross_timezone_late_night_forecast_across_midnight() {
        let fc = serde_json::json!({
            "utc_offset_seconds": 32400,
            "current": {
                "time": "2026-11-19T23:30",
                "temperature_2m": 12.4,
                "weather_code": 0,
                "is_day": 0
            },
            "daily": {
                "temperature_2m_max": [16.0, 15.0],
                "temperature_2m_min": [8.0, 7.0],
                "sunrise": ["2026-11-19T06:21", "2026-11-20T06:22"],
                "sunset": ["2026-11-19T16:33", "2026-11-20T16:32"]
            },
            "hourly": {
                "time": [
                    "2026-11-19T22:00",
                    "2026-11-19T23:00",
                    "2026-11-20T00:00",
                    "2026-11-20T01:00",
                    "2026-11-20T02:00"
                ],
                "temperature_2m": [13.0, 12.6, 11.8, 11.2, 10.9],
                "weather_code": [0, 0, 1, 2, 3],
                "is_day": [0, 0, 0, 0, 0]
            }
        });
        let snap = parse_open_meteo_forecast(&fc, "Tokyo", true, Some(18)).unwrap();
        assert_eq!(snap.utc_offset_seconds, Some(32400));
        assert_eq!(snap.sunrise, "6:21 AM");
        assert_eq!(snap.sunset, "4:33 PM");
        assert_eq!(snap.aqi, Some(18));
        assert_eq!(snap.hourly.len(), 3);
        assert_eq!(snap.hourly[0].hour_label, "12 AM");
        assert_eq!(snap.hourly[1].hour_label, "1 AM");
        assert_eq!(snap.hourly[2].hour_label, "2 AM");
        assert_eq!(snap.hourly[0].condition, "Partly Cloudy");
    }
}
