//! Place search through OpenStreetMap's Nominatim service: one request per search, sent only when
//! you press Enter (never while typing), and at most one per second, as their usage policy asks.
//! Station names of the switched-on transit maps are matched locally by the caller.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::json;
use crate::tiles::USER_AGENT;
use crate::transit_data::url_encode;

/// One search result.
#[derive(Clone, Debug)]
pub struct Hit {
    pub name: String,
    /// Extra line (the system a station belongs to, or what kind of place it is).
    pub detail: String,
    pub lon: f64,
    pub lat: f64,
    /// Size of the place in radians of arc, used to pick a zoom (0 when `zoom` is set).
    pub span_rad: f64,
    /// A fixed zoom to fly to, when known.
    pub zoom: Option<f64>,
}

type Pending = Arc<Mutex<Option<Result<Vec<Hit>, String>>>>;

#[derive(Default)]
pub struct Search {
    pending: Pending,
    last_start: Option<Instant>,
    pub busy: bool,
    pub results: Vec<Hit>,
    pub message: String,
}

impl Search {
    /// Starts a search (ignored while one is running or if the last one was under a second ago).
    pub fn start(&mut self, ctx: &egui::Context, query: &str) {
        let query = query.trim().to_string();
        if query.len() < 2 || self.busy {
            return;
        }
        if self.last_start.map_or(false, |t| t.elapsed() < Duration::from_secs(1)) {
            self.message = "one moment…".into();
            return;
        }
        self.last_start = Some(Instant::now());
        self.busy = true;
        self.message.clear();
        self.results.clear();
        let pending = self.pending.clone();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().name("search".into()).spawn(move || {
            let res = fetch(&query);
            *pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(res);
            ctx.request_repaint();
        });
    }

    /// Picks up a finished search; call once per frame.
    pub fn poll(&mut self) {
        let done = self.pending.lock().unwrap_or_else(|e| e.into_inner()).take();
        match done {
            Some(Ok(hits)) => {
                self.busy = false;
                self.message = if hits.is_empty() { "no places found".into() } else { String::new() };
                self.results = hits;
            }
            Some(Err(e)) => {
                self.busy = false;
                self.message = e;
            }
            None => {}
        }
    }
}

fn fetch(query: &str) -> Result<Vec<Hit>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .user_agent(USER_AGENT)
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("https://nominatim.openstreetmap.org/search?q={}&format=jsonv2&limit=6", url_encode(query));
    let mut resp = agent.get(&url).header("Accept", "application/json").call().map_err(|e| format!("search failed: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().map_err(|e| format!("search failed: {e}"))?;
    if status != 200 {
        return Err(format!("search failed (HTTP {status})"));
    }
    parse_hits(&text)
}

/// Reads Nominatim's `jsonv2` answer.
pub fn parse_hits(text: &str) -> Result<Vec<Hit>, String> {
    let root = json::parse(text)?;
    let items = root.as_arr().ok_or("unexpected answer from the search service")?;
    let num = |j: &json::Json, key: &str| -> Option<f64> {
        match j.get(key)? {
            json::Json::Str(s) => s.parse().ok(),
            json::Json::Num(n) => Some(*n),
            _ => None,
        }
    };
    let mut hits = Vec::new();
    for it in items {
        let (Some(lat), Some(lon)) = (num(it, "lat"), num(it, "lon")) else { continue };
        let full = it.str_at("display_name").unwrap_or("").to_string();
        let mut parts = full.split(',').map(str::trim);
        let name = parts.next().unwrap_or("").to_string();
        let detail: Vec<&str> = parts.collect();
        let detail = detail.iter().rev().take(3).rev().copied().collect::<Vec<_>>().join(", ");
        // boundingbox = [south, north, west, east] as strings
        let span_rad = it
            .get("boundingbox")
            .and_then(|b| b.as_arr())
            .and_then(|b| {
                let v: Vec<f64> = b
                    .iter()
                    .filter_map(|x| match x {
                        json::Json::Str(s) => s.parse().ok(),
                        json::Json::Num(n) => Some(*n),
                        _ => None,
                    })
                    .collect();
                (v.len() == 4).then(|| {
                    let dlat = (v[1] - v[0]).abs();
                    let dlon = (v[3] - v[2]).abs() * lat.to_radians().cos().max(0.05);
                    dlat.max(dlon).to_radians()
                })
            })
            .unwrap_or(0.02);
        if name.is_empty() {
            continue;
        }
        hits.push(Hit { name, detail, lon, lat, span_rad, zoom: None });
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nominatim_answers_are_read() {
        let text = r#"[{"place_id":1,"lat":"35.6812","lon":"139.7671","display_name":"Tokyo, Japan",
            "boundingbox":["35.5","35.9","139.5","139.9"]},
            {"lat":"40.7","lon":"-74.0","display_name":"New York, United States"}]"#;
        let h = parse_hits(text).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].name, "Tokyo");
        assert_eq!(h[0].detail, "Japan");
        assert!((h[0].lon - 139.7671).abs() < 1e-9);
        assert!(h[0].span_rad > 0.005 && h[0].span_rad < 0.01, "{}", h[0].span_rad);
        assert!(h[1].span_rad > 0.0);
        assert!(parse_hits("{}").is_err());
    }
}
