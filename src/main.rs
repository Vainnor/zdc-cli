use clap::{Parser, Subcommand};
use comfy_table::Table;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;
use chrono::{Utc, TimeZone};
use regex::Regex;
use std::collections::HashSet;
use strsim::normalized_levenshtein;

#[derive(Parser)]
#[command(name = "zdc")]
#[command(version = "0.1.2")]
#[command(about = "A Cli tool for vZDC", long_about = None)]
struct Args {
    #[arg(short, long)]
    verbose: bool,
    #[arg(long = "no-open")]
    no_open: bool,
    #[arg(short = 'p', long)]
    pubs: Option<String>,
    #[arg(short, long)]
    list: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Route {
        origin: String,
        destination: String,
        #[arg(long)]
        raw: bool,
    },
    Metar {
        station: String,
        #[arg(long)]
        raw: bool,
        #[arg(long)]
        json: bool,
    },
    Taf {
        station: String,
        #[arg(long)]
        raw: bool,
        #[arg(long)]
        json: bool,
    },
    Weather {
        station: String,
        #[arg(long)]
        raw: bool,
        #[arg(long)]
        json: bool,
    },
    Chart {
        airport: String,
        #[arg(help = "Search terms (procedure name, approach, runway, etc.)")]
        query: Vec<String>,
        #[arg(short = 'l', long = "link", help = "Print PDF URL only")]
        link: bool,
        #[arg(long, help = "Airac cycle (optional)")]
        airac: Option<i32>,
    },
}

#[derive(Deserialize, Serialize)]
struct Config {
    pubs: HashMap<String, String>,
}

fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("ZDC_CONFIG") {
        return PathBuf::from(p);
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("zdc").join("pubs.toml");
    }
    if let Some(mut home) = dirs::home_dir() {
        home.push(".config");
        home.push("zdc");
        home.push("pubs.toml");
        return home;
    }
    PathBuf::from("pubs.toml")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChartType {
    Sid,
    Star,
    Iap,
    Apd,
    Unknown,
}

impl ChartType {
    fn infer(name: &str) -> Self {
        let up = name.to_uppercase();
        if up.contains("ILS")
            || up.contains("LOC")
            || up.contains("VOR")
            || up.contains("RNAV")
            || up.contains("RNP")
            || up.contains("GPS")
            || up.contains("NDB")
            || up.contains("RWY")
        {
            return ChartType::Iap;
        }
        if up.contains("DIAGRAM") || up.contains("AIRPORT DIAGRAM") {
            return ChartType::Apd;
        }
        if up.contains("ARRIVAL") || up.contains("ARR") || up.contains("STAR") {
            return ChartType::Star;
        }
        if up.contains("DEPARTURE") || up.contains("DEP") || up.contains("SID") {
            return ChartType::Sid;
        }
        ChartType::Unknown
    }
}

#[derive(Clone, Debug)]
struct ChartQuery {
    _airport: String,
    chart_name: String,
    chart_type: ChartType,
}

impl ChartQuery {
    fn new(airport: &str, chart_name: &str) -> Self {
        let norm = normalize_chart_name(chart_name, Some(airport));
        let chart_type = ChartType::infer(&norm);
        ChartQuery {
            _airport: airport.to_uppercase(),
            chart_name: norm,
            chart_type,
        }
    }
}

fn airport_name_map(code: &str) -> Option<&'static str> {
    match code.to_uppercase().as_str() {
        "IAD" => Some("DULLES"),
        "DCA" => Some("WASHINGTON"),
        "BWI" => Some("BALTIMORE"),
        "RIC" => Some("RICHMOND"),
        "ORF" => Some("NORFOLK"),
        "RDU" => Some("RALEIGH"),
        "OAK" => Some("OAKLAND"),
        _ => None,
    }
}

fn normalize_chart_name(name: &str, airport: Option<&str>) -> String {
    let s = name.trim().to_uppercase();
    // simple aliases
    if s == "TAXI" {
        return "AIRPORT DIAGRAM".to_string();
    }
    // match pattern like CNDEL5 or RNO1
    let re = Regex::new(r"^([A-Z]+)(\d)$").unwrap();
    if let Some(caps) = re.captures(&s) {
        let mut base = caps.get(1).unwrap().as_str().to_string();
        let digit = caps.get(2).unwrap().as_str();
        let word = match digit {
            "1" => "ONE",
            "2" => "TWO",
            "3" => "THREE",
            "4" => "FOUR",
            "5" => "FIVE",
            "6" => "SIX",
            "7" => "SEVEN",
            "8" => "EIGHT",
            "9" => "NINE",
            d => d,
        };
        if let Some(ap) = airport {
            if base == ap.to_uppercase() {
                if let Some(exp) = airport_name_map(ap) {
                    base = exp.to_string();
                }
            }
        }
        return format!("{base} {word}");
    }
    s
}

#[derive(Clone, Debug)]
struct ChartInfo {
    chart_name: String,
    chart_code: String,
    pdf_path: String,
    _faa_ident: String,
    _icao_ident: String,
}

impl ChartInfo {
    fn chart_type(&self) -> ChartType {
        match self.chart_code.to_uppercase().as_str() {
            "DP" => ChartType::Sid,
            "STAR" => ChartType::Star,
            "IAP" => ChartType::Iap,
            "APD" => ChartType::Apd,
            _ => ChartType::Unknown,
        }
    }
}

#[derive(Clone, Debug)]
struct ChartMatch {
    chart: ChartInfo,
    score: f64,
}

async fn fetch_charts_from_api(
    client: &reqwest::Client,
    base: &str,
    airport: &str,
) -> Result<Vec<ChartInfo>, Box<dyn std::error::Error>> {
    let base = base.trim_end_matches('/');
    let url = format!("{}/charts?airport={}", base, airport.to_uppercase());
    let resp = client
        .get(&url)
        .header("User-Agent", "ZDC-Chart-CLI/1.0")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let body = resp.text().await?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    let mut out: Vec<ChartInfo> = Vec::new();

    // helper to extract strings from multiple possible keys
    let get_str = |obj: &serde_json::Map<String, serde_json::Value>, keys: &[&str]| {
        keys.iter().find_map(|&k| obj.get(k).and_then(|v| v.as_str()).map(|s| s.to_string()))
    };

    // top-level airport_data (fallback for faa/icao)
    let (top_faa, top_icao) = match json.get("airport_data") {
        Some(serde_json::Value::Object(map)) => (
            map.get("faa_ident").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            map.get("icao_ident").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        ),
        _ => ("".to_string(), "".to_string()),
    };

    // Primary shape: { "charts": { "airport_diagram": [ ... ], "approach": [ ... ], ... } }
    if let Some(serde_json::Value::Object(charts_map)) = json.get("charts") {
        for (category, arrv) in charts_map.iter() {
            if let serde_json::Value::Array(arr) = arrv {
                for item in arr.iter().filter_map(|v| v.as_object()) {
                    let chart_name =
                        get_str(item, &["chart_name", "title", "name", "chartTitle", "chart_title"])
                            .unwrap_or_default();
                    let pdf_path = get_str(item, &["pdf_url", "pdf", "pdf_path", "pdf_name", "file", "filename", "href", "link"])
                        .unwrap_or_default();
                    let faa = item
                        .get("faa_ident")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| top_faa.clone());
                    let icao = item
                        .get("icao_ident")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| top_icao.clone());
                    let chart_code = match category.as_str() {
                        "airport_diagram" => "APD".to_string(),
                        "departure" => "DP".to_string(),
                        "arrival" => "STAR".to_string(),
                        "approach" => "IAP".to_string(),
                        "general" => "GEN".to_string(),
                        other => other.to_uppercase(),
                    };
                    out.push(ChartInfo {
                        chart_name,
                        chart_code,
                        pdf_path,
                        _faa_ident: faa,
                        _icao_ident: icao,
                    });
                }
            }
        }
        return Ok(out);
    }

    // Fallback shapes: top-level arrays or nested maps
    match json {
        serde_json::Value::Array(arr) => {
            for item in arr.iter().filter_map(|v| v.as_object()) {
                let chart_name =
                    get_str(item, &["chart_name", "title", "name"]).unwrap_or_default();
                let pdf_path =
                    get_str(item, &["pdf_url", "pdf", "pdf_path", "pdf_name", "file", "filename"])
                        .unwrap_or_default();
                let faa = get_str(item, &["faa_ident", "faa", "ident"]).unwrap_or_default();
                let icao = get_str(item, &["icao_ident", "icao"]).unwrap_or_default();
                out.push(ChartInfo {
                    chart_name,
                    chart_code: "".to_string(),
                    pdf_path,
                    _faa_ident: faa,
                    _icao_ident: icao,
                });
            }
        }
        serde_json::Value::Object(map) => {
            for (_k, v) in map.iter() {
                if let serde_json::Value::Array(arr) = v {
                    for item in arr.iter().filter_map(|v| v.as_object()) {
                        let chart_name =
                            get_str(item, &["chart_name", "title", "name"]).unwrap_or_default();
                        let pdf_path = get_str(
                            item,
                            &["pdf_url", "pdf", "pdf_path", "pdf_name", "file", "filename"],
                        )
                            .unwrap_or_default();
                        let faa = get_str(item, &["faa_ident", "faa", "ident"]).unwrap_or_default();
                        let icao = get_str(item, &["icao_ident", "icao"]).unwrap_or_default();
                        out.push(ChartInfo {
                            chart_name,
                            chart_code: "".to_string(),
                            pdf_path,
                            _faa_ident: faa,
                            _icao_ident: icao,
                        });
                    }
                }
            }
        }
        _ => {}
    }

    Ok(out)
}

fn find_chart_by_name(
    charts: &[ChartInfo],
    query: &ChartQuery,
    ambiguity_threshold: f64,
) -> (Option<ChartInfo>, Vec<ChartMatch>) {
    if charts.is_empty() {
        return (None, Vec::new());
    }
    let q_upper = query.chart_name.to_uppercase();
    let token_re = Regex::new(r"[A-Z0-9]+").unwrap();
    let query_tokens: Vec<String> = token_re
        .find_iter(&q_upper)
        .map(|m| m.as_str().to_string())
        .collect();

    let mut matches: Vec<ChartMatch> = Vec::new();
    for ch in charts.iter() {
        if ch.chart_name.contains(", CONT.") {
            continue;
        }
        let chart_up = ch.chart_name.to_uppercase();
        let mut score = normalized_levenshtein(&q_upper, &chart_up);
        if query.chart_type != ChartType::Unknown && ch.chart_type() == query.chart_type {
            score += 0.15;
        }
        if score > 0.2 {
            matches.push(ChartMatch {
                chart: ch.clone(),
                score,
            });
        }
    }
    if matches.is_empty() {
        return (None, Vec::new());
    }
    matches.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let best = matches[0].clone();
    if (best.score - 1.0).abs() < std::f64::EPSILON {
        return (Some(best.chart.clone()), matches);
    }

    if query_tokens.len() > 1 {
        let full_matches: Vec<ChartMatch> = matches
            .iter()
            .cloned()
            .filter(|m| {
                let chart_tokens: HashSet<_> = token_re
                    .find_iter(&m.chart.chart_name.to_uppercase())
                    .map(|t| t.as_str().to_string())
                    .collect();
                query_tokens
                    .iter()
                    .all(|qt| chart_tokens.iter().any(|ct| ct == qt || ct.contains(qt)))
            })
            .collect();
        if full_matches.len() == 1 {
            return (Some(full_matches[0].chart.clone()), matches);
        } else if !full_matches.is_empty() {
            return (None, full_matches);
        }
    }

    if matches.len() > 1 {
        let second = matches[1].score;
        if best.score - second < ambiguity_threshold {
            let close: Vec<ChartMatch> = matches
                .into_iter()
                .filter(|m| m.score >= best.score - ambiguity_threshold)
                .collect();
            return (None, close);
        }
    }

    (Some(best.chart.clone()), matches)
}

fn find_all_chart_pages(charts: &[ChartInfo], base_chart: &ChartInfo) -> Vec<ChartInfo> {
    let mut pages: Vec<(usize, ChartInfo)> = Vec::new();
    let base_name = if let Some(pos) = base_chart.chart_name.find(", CONT.") {
        base_chart.chart_name[..pos].to_string()
    } else {
        base_chart.chart_name.clone()
    };
    for c in charts.iter() {
        if c.chart_name == base_name {
            pages.push((0, c.clone()));
            continue;
        }
        let prefix = format!("{}, CONT.", base_name);
        if c.chart_name.starts_with(&prefix) {
            if let Some(pos) = c.chart_name.find(", CONT.") {
                let cont = c.chart_name[pos + 7..].trim();
                let n = cont.parse::<usize>().unwrap_or(999);
                pages.push((n, c.clone()));
            } else {
                pages.push((999, c.clone()));
            }
        }
    }
    pages.sort_by_key(|(n, _)| *n);
    pages.into_iter().map(|(_, c)| c).collect()
}

fn absolute_pdf_url(base: &str, pdf_path: &str) -> String {
    let p = pdf_path.trim();
    if p.starts_with("http://") || p.starts_with("https://") || p.starts_with("file://") {
        return p.to_string();
    }
    if p.starts_with("//") {
        return format!("https:{}", p);
    }
    let base_trim = base.trim_end_matches('/');
    let domain_base = if let Some(pos) = base_trim.find("/v1") {
        &base_trim[..pos]
    } else if let Some(pos) = base_trim.find("/v2") {
        &base_trim[..pos]
    } else {
        base_trim
    };
    if p.starts_with('/') {
        format!("{}{}", domain_base.trim_end_matches('/'), p)
    } else {
        format!("{}/{}", domain_base.trim_end_matches('/'), p)
    }
}

fn load_or_create_config(path: &PathBuf) -> Config {
    if path.exists() {
        let s = fs::read_to_string(path).expect("failed to read config file");
        toml::from_str(&s).expect("failed to parse config")
    } else {
        let mut default = Config {
            pubs: HashMap::new(),
        };
        default.pubs.insert("the_fox".to_string(), "https://example.com/the_fox"
            .to_string());
        default.pubs.insert("green_dragon".to_string(),
                            "https://example.com/green_dragon".to_string());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        let s = toml::to_string_pretty(&default).expect("failed to serialize default config");
        fs::write(path, s).expect("failed to write default config");
        default
    }
}

fn normalize_alias(s: &str) -> String {
    s.to_lowercase().replace('-', "_").replace(' ', "_")
}

fn norm_airport_for_routes(src: &str) -> String {
    let mut s = src.trim().to_string();
    if s.chars().next().map(|c| c.eq_ignore_ascii_case(&'k')).unwrap_or(false) {
        s = s.chars().skip(1).collect();
    }
    s.to_uppercase()
}

fn format_unix(ts: i64) -> String {
    match Utc.timestamp_opt(ts, 0).single() {
        Some(dt) => dt.format("%Y-%m-%d %H:%M UTC").to_string(),
        None => ts.to_string(),
    }
}

async fn fetch_awc(
    client: &reqwest::Client,
    endpoint: &str,
    ids: &str,
    format: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let url = format!(
        "https://aviationweather.gov/api/data/{}?ids={}&format={}",
        endpoint, ids, format
    );
    let resp = client.get(&url).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(Box::<dyn std::error::Error>::from(format!(
            "api error {}: {}",
            status, body
        )));
    }
    if format == "raw" {
        return Ok(Value::String(body));
    }
    let json: Value = serde_json::from_str(&body)?;
    Ok(json)
}

fn into_vec(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        Value::Object(mut o) => {
            if let Some(arr) = o.remove("data") {
                match arr {
                    Value::Array(a) => a,
                    other => vec![other],
                }
            } else {
                vec![Value::Object(o)]
            }
        }
        other => vec![other],
    }
}

fn get_str_field(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| {
        if let Some(s) = x.as_str() {
            Some(s.to_string())
        } else if let Some(i) = x.as_i64() {
            Some(i.to_string())
        } else if let Some(f) = x.as_f64() {
            Some(format!("{}", f))
        } else {
            None
        }
    })
}

fn c_to_f(c: f64) -> f64 {
    c * 9.0 / 5.0 + 32.0
}

fn build_metar_table(m: &Value) -> Table {
    let station = get_str_field(m, "icaoId")
        .or_else(|| get_str_field(m, "station_id"))
        .unwrap_or_default();
    let time = get_str_field(m, "reportTime")
        .or_else(|| m.get("obsTime").and_then(|n| n.as_i64().map(format_unix)))
        .unwrap_or_default();

    let wdir = m.get("wdir").and_then(|x| {
        if let Some(s) = x.as_str() {
            Some(s.to_string())
        } else if let Some(i) = x.as_i64() {
            Some(i.to_string())
        } else {
            None
        }
    });
    let wspd = m.get("wspd").and_then(|x| x.as_f64());
    let wgst = m.get("wgst").and_then(|x| x.as_f64());
    let mut wind_parts: Vec<String> = Vec::new();
    if let Some(w) = wdir {
        wind_parts.push(w);
    }
    if let Some(s) = wspd {
        wind_parts.push(format!("{} kt", s.round() as i64));
    }
    if let Some(g) = wgst {
        wind_parts.push(format!("G{} kt", g.round() as i64));
    }
    let wind = wind_parts.join(" ");

    let vis = get_str_field(m, "visib").unwrap_or_default();
    let temp = m.get("temp").and_then(|n| n.as_f64());
    let dewp = m.get("dewp").and_then(|n| n.as_f64());
    let temp_str = match (temp, dewp) {
        (Some(t), Some(d)) => format!(
            "{:.1}°C/{:.1}°C ({:.0}°F/{:.0}°F)",
            t,
            d,
            c_to_f(t).round(),
            c_to_f(d).round()
        ),
        (Some(t), None) => format!("{:.1}°C ({:.0}°F)", t, c_to_f(t).round()),
        _ => "".to_string(),
    };

    let altim = m.get("altim").and_then(|n| n.as_f64());
    let alt_str = if let Some(a) = altim {
        if a >= 50.0 {
            let inhg = a * 0.029529983071445;
            format!("{:.1} hPa ({:.2} inHg)", a, inhg)
        } else {
            let hpa = a / 0.029529983071445;
            format!("{:.2} inHg ({:.1} hPa)", a, hpa)
        }
    } else {
        "".to_string()
    };

    let fltcat = get_str_field(m, "fltCat").unwrap_or_default();
    let clouds = m.get("clouds").and_then(|c| {
        c.as_array().map(|arr| {
            arr.iter()
                .map(|layer| {
                    let cover =
                        layer.get("cover").and_then(|s| s.as_str()).unwrap_or("");
                    let base = layer
                        .get("base")
                        .and_then(|n| n.as_i64().map(|b| b.to_string()))
                        .or_else(|| {
                            layer
                                .get("base")
                                .and_then(|n| n.as_f64().map(|f| f.to_string()))
                        })
                        .unwrap_or_default();
                    if base.is_empty() {
                        cover.to_string()
                    } else {
                        format!("{}{}", cover, base)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
    }).unwrap_or_default();

    let mut table = Table::new();
    table.set_header(vec![
        "Station", "Time", "Wind", "Vis", "Temp/Dew", "Alt", "FlightCat", "Clouds",
    ]);
    table.add_row(vec![
        station.as_str(),
        time.as_str(),
        wind.as_str(),
        vis.as_str(),
        temp_str.as_str(),
        alt_str.as_str(),
        fltcat.as_str(),
        clouds.as_str(),
    ]);
    table
}

fn build_taf_table(t: &Value) -> Table {
    let mut table = Table::new();
    table.set_header(vec!["Period", "Wind", "Vis", "Wx", "Alt", "Clouds"]);

    if let Some(fcsts) = t.get("fcsts").and_then(|v| v.as_array()) {
        for f in fcsts {
            let from = f
                .get("timeFrom")
                .and_then(|n| n.as_i64())
                .map(format_unix)
                .unwrap_or_default();
            let to = f
                .get("timeTo")
                .and_then(|n| n.as_i64())
                .map(format_unix)
                .unwrap_or_default();

            let wdir = f
                .get("wdir")
                .and_then(|x| x.as_str().map(|s| s.to_string()))
                .or_else(|| f.get("wdir").and_then(|x| x.as_i64().map(|i| i.to_string())));
            let wspd = f.get("wspd").and_then(|n| n.as_f64());
            let wgst = f.get("wgst").and_then(|n| n.as_f64());
            let mut wind_parts: Vec<String> = Vec::new();
            if let Some(w) = wdir {
                wind_parts.push(w);
            }
            if let Some(s) = wspd {
                wind_parts.push(format!("{} kt", s.round() as i64));
            }
            if let Some(g) = wgst {
                wind_parts.push(format!("G{} kt", g.round() as i64));
            }
            let wind = wind_parts.join(" ");

            let vis = f
                .get("visib")
                .and_then(|x| {
                    if let Some(s) = x.as_str() {
                        Some(s.to_string())
                    } else if let Some(n) = x.as_f64() {
                        Some(format!("{}", n))
                    } else {
                        None
                    }
                })
                .unwrap_or_default();

            let wx = f
                .get("wxString")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            let alt = f.get("altim").and_then(|n| n.as_f64()).map(|a| {
                if a >= 50.0 {
                    let inhg = a * 0.029529983071445;
                    format!("{:.1} hPa ({:.2} inHg)", a, inhg)
                } else {
                    let hpa = a / 0.029529983071445;
                    format!("{:.2} inHg ({:.1} hPa)", a, hpa)
                }
            }).unwrap_or_default();

            let clouds = f.get("clouds").and_then(|c| {
                c.as_array().map(|arr| {
                    arr.iter()
                        .map(|layer| {
                            let cover = layer.get("cover").and_then(|s| s.as_str()).unwrap_or("");
                            let base = layer
                                .get("base")
                                .and_then(|n| n.as_i64().map(|b| b.to_string()))
                                .or_else(|| {
                                    layer
                                        .get("base")
                                        .and_then(|n| n.as_f64().map(|f| f.to_string()))
                                })
                                .unwrap_or_default();
                            if base.is_empty() {
                                cover.to_string()
                            } else {
                                format!("{}{}", cover, base)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
            }).unwrap_or_default();

            let period = if !from.is_empty() || !to.is_empty() {
                format!("{} - {}", from, to)
            } else {
                "".to_string()
            };

            table.add_row(vec![
                period.as_str(),
                wind.as_str(),
                vis.as_str(),
                wx.as_str(),
                alt.as_str(),
                clouds.as_str(),
            ]);
        }
    }

    table
}

async fn handle_metar(
    client: &reqwest::Client,
    station: &str,
    raw: bool,
    json: bool,
    _verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut st = station.trim().to_uppercase();
    let mut data = fetch_awc(client, "metar", &st, "json").await?;
    let mut arr = into_vec(data);
    if arr.is_empty() && st.len() == 3 && !st.starts_with('K') {
        st = format!("K{}", st);
        data = fetch_awc(client, "metar", &st, "json").await?;
        arr = into_vec(data);
    }
    if arr.is_empty() {
        eprintln!("No METAR data found for {}", st);
        return Ok(());
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }
    for entry in arr {
        let rawtxt = get_str_field(&entry, "rawOb")
            .or_else(|| get_str_field(&entry, "raw_text"))
            .unwrap_or_default();
        if !rawtxt.is_empty() {
            println!("{}", rawtxt);
            println!();
        } else if raw {
            println!("{}", serde_json::to_string_pretty(&entry)?);
            println!();
        }
        let table = build_metar_table(&entry);
        println!("{table}");
    }
    Ok(())
}

async fn handle_taf(
    client: &reqwest::Client,
    station: &str,
    raw: bool,
    json: bool,
    _verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut st = station.trim().to_uppercase();
    let mut data = fetch_awc(client, "taf", &st, "json").await?;
    let mut arr = into_vec(data);
    if arr.is_empty() && st.len() == 3 && !st.starts_with('K') {
        st = format!("K{}", st);
        data = fetch_awc(client, "taf", &st, "json").await?;
        arr = into_vec(data);
    }
    if arr.is_empty() {
        eprintln!("No TAF data found for {}", st);
        return Ok(());
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }
    for entry in arr {
        let rawtxt = get_str_field(&entry, "rawTAF").unwrap_or_default();
        if !rawtxt.is_empty() {
            println!("{}", rawtxt);
            println!();
        } else if raw {
            println!("{}", serde_json::to_string_pretty(&entry)?);
            println!();
        }
        let station_name = get_str_field(&entry, "icaoId").unwrap_or_else(|| st.clone());
        let issue = get_str_field(&entry, "issueTime").unwrap_or_default();
        let valid_from = entry
            .get("validTimeFrom")
            .and_then(|n| n.as_i64())
            .map(format_unix)
            .unwrap_or_default();
        let valid_to = entry
            .get("validTimeTo")
            .and_then(|n| n.as_i64())
            .map(format_unix)
            .unwrap_or_default();
        let header = format!(
            "{}  issued: {}  valid: {} - {}",
            station_name, issue, valid_from, valid_to
        );
        println!("{header}");
        let taf_table = build_taf_table(&entry);
        println!("{taf_table}");
    }
    Ok(())
}

#[allow(dead_code)]
fn flatten_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_lowercase(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(a) => a.iter().map(flatten_text).collect::<Vec<_>>().join(" "),
        Value::Object(o) => o.values().map(flatten_text).collect::<Vec<_>>().join(" "),
        Value::Null => "".to_string(),
    }
}

#[allow(dead_code)]
fn find_first_pdf_url(v: &Value, base: &str) -> Option<String> {
    // collect exact-URL candidates first, fallback to bare filenames
    let mut full_urls: Vec<String> = Vec::new();
    let mut file_names: Vec<String> = Vec::new();

    fn rec(val: &Value, full_urls: &mut Vec<String>, file_names: &mut Vec<String>) {
        match val {
            Value::String(s) => {
                let s = s.trim();
                if !s.to_lowercase().contains(".pdf") {
                    return;
                }
                if s.starts_with("http://")
                    || s.starts_with("https://")
                    || s.starts_with("file://")
                    || s.starts_with("//")
                {
                    full_urls.push(s.to_string());
                } else {
                    file_names.push(s.to_string());
                }
            }
            Value::Array(arr) => {
                for it in arr {
                    rec(it, full_urls, file_names);
                }
            }
            Value::Object(map) => {
                for (_k, v) in map {
                    rec(v, full_urls, file_names);
                }
            }
            _ => {}
        }
    }

    rec(v, &mut full_urls, &mut file_names);

    // prefer absolute/complete URLs
    if let Some(u) = full_urls.into_iter().next() {
        if u.starts_with("//") {
            return Some(format!("https:{}", u));
        }
        return Some(u);
    }

    // build an absolute URL from a bare filename or relative path
    if let Some(name) = file_names.into_iter().next() {
        let base = base.trim_end_matches('/');
        // derive domain base (strip /v2 if present) so we can join leading '/...' paths sensibly
        let domain_base = if let Some(pos) = base.find("/v2") {
            &base[..pos]
        } else {
            base
        };

        if name.starts_with('/') {
            // e.g. "/v2/charts/..." or "/files/..."
            return Some(format!("{}{}", domain_base.trim_end_matches('/'), name));
        }

        // if we only have a filename, try sensible locations under the provided base
        // prefer base/charts/<name> so we cover api-v2 hosting patterns
        return Some(format!("{}/charts/{}", base, name));
    }

    None
}

async fn handle_chart(
    client: &reqwest::Client,
    airport: &str,
    query: &[String],
    link_only: bool,
    _airac: Option<i32>,
    auto_open: bool,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let default_base = "https://api-v2.aviationapi.com/v2";
    let base = std::env::var("ZDC_CHARTS_BASE").unwrap_or_else(|_| default_base.into());

    if verbose {
        eprintln!("charts base: {}", base);
        eprintln!("airport arg: {}", airport);
        eprintln!("query tokens: {:?}", query);
    }

    let mut charts = fetch_charts_from_api(client, &base, airport).await?;
    if charts.is_empty() && airport.len() == 3 && !airport.starts_with('K') {
        let k_air = format!("K{}", airport.to_uppercase());
        if verbose {
            eprintln!("retry GET {}/charts?apt={}", base, k_air);
        }
        charts = fetch_charts_from_api(client, &base, &k_air).await?;
    }

    if charts.is_empty() {
        eprintln!("No charts found for {}", airport);
        return Ok(());
    }

    let q_str = query.join(" ");
    let cq = ChartQuery::new(airport, &q_str);
    let (maybe_chart, _matches) = find_chart_by_name(&charts, &cq, 0.15);

    if maybe_chart.is_none() {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Idx", "Title / Name", "Likely PDF"]);
        for (i, ch) in charts.iter().take(12).enumerate() {
            let pdf = absolute_pdf_url(&base, &ch.pdf_path);
            table.add_row(vec![
                i.to_string().as_str(),
                ch.chart_name.as_str(),
                pdf.as_str(),
            ]);
        }
        println!("Multiple possible charts (no strong match).");
        println!("{table}");
        println!("Refine your query or pass a more specific string.");
        return Ok(());
    }

    let chart = maybe_chart.unwrap();
    let pages = find_all_chart_pages(&charts, &chart);
    let pdf_urls: Vec<String> =
        pages.into_iter().map(|p| absolute_pdf_url(&base, &p.pdf_path)).collect();

    if link_only || !auto_open {
        for u in pdf_urls.iter() {
            println!("{}", u);
        }
        return Ok(());
    }

    if let Some(url) = pdf_urls.first() {
        if verbose {
            eprintln!("Opening {}", url);
        }
        match open::that(url) {
            Ok(_) => return Ok(()),
            Err(e) => {
                eprintln!("failed to open: {e}");
                println!("{}", url);
                for u in pdf_urls.iter().skip(1) {
                    println!("{}", u);
                }
                return Ok(());
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    if args.verbose {
        println!("🚀 vZDC initialized");
    }

    let cfg_path = config_path();
    let cfg = load_or_create_config(&cfg_path);

    if args.list {
        println!("Available pubs (from {:?}):", cfg_path);
        for (k, v) in &cfg.pubs {
            println!(" - {} -> {}", k, v);
        }
        return Ok(());
    }

    if let Some(alias) = args.pubs {
        let alias_norm = normalize_alias(&alias);
        let normalized: HashMap<_, _> = cfg
            .pubs
            .iter()
            .map(|(k, v)| (normalize_alias(k), v.clone()))
            .collect();

        if let Some(url) = normalized.get(&alias_norm) {
            if args.no_open {
                println!("{}", url);
            } else {
                match open::that(url) {
                    Ok(_) => {}
                    Err(_) => println!("{}", url),
                }
            }
            return Ok(());
        } else {
            eprintln!("Unknown pub '{}'. Run --list to see aliases.", alias);
            std::process::exit(2);
        }
    }

    let client = reqwest::Client::new();

    if let Some(cmd) = args.command {
        match cmd {
            Commands::Route { origin, destination, raw } => {
                let origin = norm_airport_for_routes(&origin);
                let destination = norm_airport_for_routes(&destination);
                let url = format!("https://api.aviationapi.com/v1/preferred-routes/search?origin={}&dest={}",
                                  origin, destination);
                if args.verbose { eprintln!("GET {}", url); }
                let resp = client.get(&url).send().await?;
                let status = resp.status();
                let body = resp.text().await?;
                if !status.is_success() {
                    eprintln!("api error {}: {}", status, body);
                    std::process::exit(1);
                }
                let json: Value = serde_json::from_str(&body)?;
                let rows = match json {
                    Value::Array(a) => a,
                    other => vec![other],
                };
                if rows.is_empty() {
                    println!("No preferred routes found for {} -> {}", origin, destination);
                    return Ok(());
                }
                if raw {
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                    return Ok(());
                }
                let mut keys = BTreeSet::new();
                for v in &rows {
                    if let Value::Object(map) = v {
                        for k in map.keys() {
                            keys.insert(k.clone());
                        }
                    } else {
                        keys.insert("value".to_string());
                    }
                }
                let headers: Vec<String> = keys.into_iter().collect();
                let mut table = Table::new();
                table.set_header(headers.iter().map(|s| s.as_str()).collect::<Vec<_>>());
                for v in rows {
                    let mut cells = Vec::with_capacity(headers.len());
                    for k in &headers {
                        let s = if let Value::Object(map) = &v {
                            let val = map.get(k).unwrap_or(&Value::Null);
                            match val {
                                Value::String(s) => s.clone(),
                                Value::Number(n) => n.to_string(),
                                Value::Bool(b) => b.to_string(),
                                Value::Array(a) => a.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "),
                                Value::Object(_) => serde_json::to_string(val).unwrap_or_default(),
                                Value::Null => "".to_string(),
                            }
                        } else {
                            v.to_string()
                        };
                        cells.push(s);
                    }
                    let row_refs: Vec<&str> = cells.iter().map(|s| s.as_str()).collect();
                    table.add_row(row_refs);
                }
                println!("{table}");
            }

            Commands::Metar { station, raw, json } => {
                handle_metar(&client, &station, raw, json, args.verbose).await?;
            }

            Commands::Taf { station, raw, json } => {
                handle_taf(&client, &station, raw, json, args.verbose).await?;
            }

            Commands::Weather { station, raw, json } => {
                handle_metar(&client, &station, raw, json, args.verbose).await?;
                println!();
                handle_taf(&client, &station, raw, json, args.verbose).await?;
            }

            Commands::Chart {
                airport,
                query,
                link,
                airac,
            } => {
                handle_chart(
                    &client,
                    &airport,
                    &query,
                    link,
                    airac,
                    /* auto_open = */ !args.no_open,
                    args.verbose,
                )
                    .await?;
            }
        }
    }

    Ok(())
}