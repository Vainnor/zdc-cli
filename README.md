# vZDC

Small Rust CLI for vZDC — quick access to pubs, charts, METAR/TAF, preferred routes and simple weather lookups.

Quick features
- pubs aliases -> open/print pub URL from configurable TOML
- chart lookup (aviationapi v2) -> open or print PDF URLs; fuzzy matching + ambiguity handling
- metar / taf -> fetch, pretty-print and show raw text (AviationWeather.gov)
- route -> fetch preferred routes (aviationapi preferred-routes)
- weather -> runs metar + taf for one station
- verbose mode for debugging

Prereqs
- Rust toolchain (rustc + cargo)
- Network access
- Optional: system PDF viewer for `chart` open behavior

Quick install
- From crates.io (after publishing):
```bash
cargo install zdc
```

- From Git:
```bash
cargo install --git https://github.com/<you>/zdc --tag v0.1.0
```

- From a local checkout:
```bash
cargo install --path .
# or build
cargo build --release
./target/release/zdc ...
```

Quick usage
```bash
# list configured pubs
zdc --list

# print pub URL for alias "iad"
zdc -p iad

# preferred routes (pretty table)
zdc route IAD BWI
zdc route Kiad kbwi --raw

# METAR / TAF
zdc metar KIAD
zdc metar KJFK --raw
zdc taf KJFK --json

# Combined weather (metar + taf)
zdc weather KIAD

# charts (opens the PDF, prints URL if open fails)
zdc chart OAK ILS 28R
# print URL only
zdc chart OAK ILS 28R -l

# verbose for debug
zdc -v chart OAK ILS 28R
```

Commands (summary)
- route <ORIGIN> <DEST> [--raw] — preferred route lookup
- metar <STATION> [--raw] [--json] — METAR fetch/parse
- taf <STATION> [--raw] [--json] — TAF fetch/parse
- weather <STATION> [--raw] [--json] — run metar then taf
- chart <AIRPORT> <QUERY...> [-l|--link] [--airac <CYCLE>] — chart search/open
- -p / --pubs <ALIAS> — print pub URL for alias
- --list — list configured pub aliases
- -v / --verbose — verbose mode

Configuration
- Pubs config TOML (default)
    - macOS / Linux default: $XDG_CONFIG_HOME/zdc/pubs.toml or ~/.config/zdc/pubs.toml
    - override with env var: ZDC_CONFIG=/path/to/file.toml

Example pubs.toml
```toml
[pubs]
iad = "https://vzdc.org/f/iad"
bwi = "https://vzdc.org/f/bwi"
dca = "https://vzdc.org/f/dca"
pct = "https://vzdc.org/f/pct"
pct-zdc2 = "https://vzdc.org/f/pct-zdc2"
pct-zdc3 = "https://vzdc.org/f/pct-zdc3"
zdc = "https://vzdc.org/f/zdc"
zdc2 = "https://vzdc.org/f/zdc2"
zdc3 = "https://vzdc.org/f/zdc3"
zbw = "https://vzdc.org/f/zbw"
zid = "https://vzdc.org/f/zid"
zjx = "https://vzdc.org/f/zjx"
zny = "https://vzdc.org/f/zny"
zob = "https://vzdc.org/f/zob"
ztl = "https://vzdc.org/f/ztl"
loa-usnv = "https://vzdc.org/f/loa-usnv"
staff = "https://vzdc.org/f/staff"
chp-charts = "https://vzdc.org/f/chp-charts"
jrv-charts = "https://vzdc.org/f/jrv-charts"
mtv-charts = "https://vzdc.org/f/charts-mtv"
shd-charts = "https://vzdc.org/f/shd-charts"
atpa = "https://vzdc.org/f/atpa"
ids = "https://vzdc.org/f/ids"
train-radar = "https://vzdc.org/f/train-radar"
train-tower = "https://vzdc.org/f/train-tower"
sfra-pilot = "https://vzdc.org/f/sfra-pilot"
sfra = "https://vzdc.org/f/sfra"
```

Environment overrides
- ZDC_CONFIG — full path to pubs TOML
- ZDC_CHARTS_BASE — charts API base URL (default: `https://api-v2.aviationapi.com/v2`)
    - useful for testing or alternate chart backends

APIs used
- Charts: aviationapi v2 (default) — /v2/charts?airport=...
- METAR/TAF: AviationWeather.gov data API
- Preferred routes: aviationapi preferred-routes search endpoint

Behavior notes / tips
- Airport codes are normalized (leading `K` dropped when appropriate)
- `chart` tries to open the PDF with the system default; if that fails it prints the absolute URL(s)
- Use `-v` to surface requests/responses for debugging
- If you see a filename (e.g. `00294ad.pdf`) returned, the CLI normalizes it into an absolute URL using the charts base URL

Dependencies (high level)
- clap, tokio, reqwest, serde, serde_json, toml, dirs
- comfy-table for terminal tables
- regex + strsim for fuzzy chart matching
- open for launching URLs

Extending / contributing
- Add new subcommands in `src/main.rs` (use clap Subcommand)
- Keep sensitive defaults configurable via env vars or config file
- Run `cargo fmt` and `cargo clippy` before PRs

Troubleshooting
- charts: if you get "No charts found", run with `-v` to see which endpoint was queried and any API response body.
- opening PDFs: if open fails, CLI prints the URL; you can manually download/open it.
- config file not found: a default pubs.toml is created at first run; set ZDC_CONFIG to override.
