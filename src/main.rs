use anyhow::{anyhow, Context, Result};
use chrono::{Datelike, NaiveDate};
use clap::{Parser, ValueEnum};
use dialoguer::{theme::ColorfulTheme, FuzzySelect, Input, Select};
use num_format::{Locale, ToFormattedString};
use rand::seq::SliceRandom;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, REFERER};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

// NEW: XML parser for SDMX-ML responses
use quick_xml::events::Event;
use quick_xml::Reader;

// ----------------------- Constants -----------------------
const IMF_SDMX_BASE: &str = "https://api.imf.org/external/sdmx/2.1";
const IMF_SDMX_STRUCTURE_BASE: &str = "https://sdmxcentral.imf.org/ws/public/sdmxapi/rest";
const IMF_DATAMAPPER_BASE: &str = "https://www.imf.org/external/datamapper/api/v1";

// ----------------------- SDMX CPI dataset (NEW STYLE KEY) -----------------------
// Your example:
//   /data/CPI/RUS.CPI._T.IX.M?startPeriod=2020-M01
// Key parts:
//   COUNTRY.INDEX_TYPE.COICOP_1999.TYPE_OF_TRANSFORMATION.FREQUENCY
const SDMX_CPI_DATASET: &str = "CPI";
const SDMX_CPI_INDEX_TYPE: &str = "CPI"; // headline index family
const SDMX_CPI_COICOP: &str = "_T"; // all-items
const SDMX_CPI_TRANSFORMATION: &str = "IX"; // index level
const SDMX_CPI_FREQ: &str = "M"; // monthly

// SDMX codelist for CPI areas
const SDMX_CL_AREA_CPI: &str = "CL_COUNTRY_ISO3";

// DataMapper fixed indicator for annual inflation rate
const DATAMAPPER_INDICATOR: &str = "PCPIPCH"; // annual inflation (%), avg consumer prices

// ----------------------- CLI -----------------------
#[derive(Copy, Clone, Debug, ValueEnum)]
enum Mode {
    Sdmx,
    Datamapper,
}

#[derive(Parser, Debug)]
#[command(
    name = "real-income",
    about = "Compute the inflation-adjusted (real) value of your income using IMF SDMX (monthly CPI index) or IMF DataMapper (annual inflation)."
)]
struct Args {
    /// Mode: sdmx (monthly CPI index, most precise) or datamapper (annual inflation approximation)
    #[arg(long, value_enum)]
    mode: Option<Mode>,

    /// Country code (optional; otherwise interactive dropdown)
    /// - SDMX CPI dataset uses IMF economy codes like RUS, USA, CHE, etc.
    /// - DataMapper uses ISO3 codes (e.g. CHE, DEU, USA)
    #[arg(long)]
    country: Option<String>,

    /// Start date:
    /// - SDMX: YYYY-MM
    /// - DataMapper: YYYY or YYYY-MM (month ignored)
    #[arg(long)]
    start: Option<String>,

    /// Nominal amount (optional; otherwise interactive)
    #[arg(long)]
    amount: Option<f64>,

    /// Optional end:
    /// - SDMX: YYYY-MM
    /// - DataMapper: YYYY (or YYYY-MM)
    #[arg(long)]
    end: Option<String>,

    /// Use disk cache (default true)
    #[arg(long, default_value_t = true)]
    cache: bool,

    /// Disable jokes
    #[arg(long, default_value_t = false)]
    no_jokes: bool,

    /// Print debug info
    #[arg(long, default_value_t = false)]
    verbose: bool,
}

// ----------------------- Shared Types -----------------------
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Item {
    code: String,
    name: String,
}

#[derive(Debug, Clone)]
struct YearInflation {
    year: i32,
    pct: f64,
}

// ----------------------- Main -----------------------
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let theme = ColorfulTheme::default();

    let sdmx_client = Client::builder()
        .user_agent("real-income/0.3.1 (rust reqwest)")
        .build()
        .context("Failed to build SDMX HTTP client")?;

    let datamapper_client = build_datamapper_client()?;

    let cache_dir = default_cache_dir()?;
    if args.cache {
        fs::create_dir_all(&cache_dir).ok();
    }

    // 1) Mode dropdown
    let mode = match args.mode {
        Some(m) => m,
        None => prompt_mode(&theme)?,
    };

    // 2) Amount
    let amount = match args.amount {
        Some(a) if a > 0.0 => a,
        Some(_) => return Err(anyhow!("Amount must be > 0")),
        None => prompt_amount(&theme)?,
    };

    // 3) Start
    let start_input = match args.start.clone() {
        Some(s) => s.trim().to_string(),
        None => match mode {
            Mode::Sdmx => prompt_start_monthly(&theme)?,
            Mode::Datamapper => prompt_start_yearly(&theme)?,
        },
    };

    let end_input: Option<String> = args.end.clone().and_then(|e| {
        let t = e.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    });

    match mode {
        Mode::Sdmx => {
            run_sdmx(
                &sdmx_client,
                &cache_dir,
                args.cache,
                args.verbose,
                &theme,
                args.country,
                start_input,
                amount,
                args.no_jokes,
                end_input.clone(),
            )
            .await?;
        }
        Mode::Datamapper => {
            run_datamapper(
                &datamapper_client,
                &cache_dir,
                args.cache,
                args.verbose,
                &theme,
                args.country,
                start_input,
                amount,
                args.no_jokes,
                end_input.clone(),
            )
            .await?;
        }
    }

    Ok(())
}

// ----------------------- Prompts -----------------------
fn prompt_mode(theme: &ColorfulTheme) -> Result<Mode> {
    let items = vec![
        "SDMX (recommended): Monthly CPI index level (most precise)",
        "DataMapper: Annual inflation approximation (PCPIPCH)",
    ];

    let idx = Select::with_theme(theme)
        .with_prompt("Choose mode")
        .items(&items)
        .default(0)
        .interact()
        .context("Mode selection failed")?;

    Ok(if idx == 0 { Mode::Sdmx } else { Mode::Datamapper })
}

fn prompt_amount(theme: &ColorfulTheme) -> Result<f64> {
    let a: f64 = Input::with_theme(theme)
        .with_prompt("Nominal amount (e.g. 100000)")
        .validate_with(|x: &f64| -> Result<(), &str> {
            if *x > 0.0 {
                Ok(())
            } else {
                Err("Must be > 0")
            }
        })
        .interact_text()?;

    Ok(a)
}

fn prompt_start_monthly(theme: &ColorfulTheme) -> Result<String> {
    let s: String = Input::with_theme(theme)
        .with_prompt("Start date (YYYY-MM), e.g. 2021-01")
        .validate_with(|input: &String| -> Result<(), &str> {
            if parse_ym(input).is_ok() {
                Ok(())
            } else {
                Err("Expected YYYY-MM")
            }
        })
        .interact_text()?;

    Ok(s.trim().to_string())
}

fn prompt_start_yearly(theme: &ColorfulTheme) -> Result<String> {
    let s: String = Input::with_theme(theme)
        .with_prompt("Start year (YYYY) or YYYY-MM (month ignored), e.g. 2021")
        .validate_with(|input: &String| -> Result<(), &str> {
            if parse_year_loose(input).is_ok() {
                Ok(())
            } else {
                Err("Expected YYYY or YYYY-MM")
            }
        })
        .interact_text()?;

    Ok(s.trim().to_string())
}

fn prompt_fuzzy_pick(theme: &ColorfulTheme, prompt: &str, items: &[Item]) -> Result<(String, String)> {
    let labels: Vec<String> = items
        .iter()
        .map(|x| format!("{} - {}", x.name, x.code))
        .collect();

    let idx = FuzzySelect::with_theme(theme)
        .with_prompt(prompt)
        .items(&labels)
        .default(0)
        .interact()
        .context("Selection failed")?;

    let it = &items[idx];
    Ok((it.code.clone(), it.name.clone()))
}

// ----------------------- Parsing helpers -----------------------
fn parse_ym(s: &str) -> Result<String> {
    let t = s.trim();
    if t.len() != 7 {
        return Err(anyhow!("Expected YYYY-MM"));
    }
    let parts: Vec<&str> = t.split('-').collect();
    if parts.len() != 2 {
        return Err(anyhow!("Expected YYYY-MM"));
    }
    let y: i32 = parts[0].parse()?;
    let m: u32 = parts[1].parse()?;
    if !(1..=12).contains(&m) {
        return Err(anyhow!("Month out of range"));
    }
    let _ = NaiveDate::from_ymd_opt(y, m, 1).ok_or_else(|| anyhow!("Invalid date"))?;
    Ok(format!("{:04}-{:02}", y, m))
}

fn parse_year_loose(s: &str) -> Result<i32> {
    let t = s.trim();
    let year_part = t.split('-').next().unwrap_or(t);
    let y: i32 = year_part.parse()?;
    if !(1800..=3000).contains(&y) {
        return Err(anyhow!("Year out of reasonable range"));
    }
    Ok(y)
}

// NEW: convert "YYYY-MM" => "YYYY-MMM" (SDMX monthly format "YYYY-M01")
fn ym_to_sdmx_period(ym: &str) -> Result<String> {
    let t = parse_ym(ym)?;
    let parts: Vec<&str> = t.split('-').collect();
    let y: i32 = parts[0].parse()?;
    let m: u32 = parts[1].parse()?;
    Ok(format!("{:04}-M{:02}", y, m))
}

// ----------------------- Cache dir -----------------------
fn default_cache_dir() -> Result<PathBuf> {
    let mut dir = dirs::cache_dir().ok_or_else(|| anyhow!("Could not locate a cache directory"))?;
    dir.push("real-income");
    Ok(dir)
}

// ----------------------- Formatting & Report -----------------------
fn fmt_money(x: f64) -> String {
    let sign = if x < 0.0 { "-" } else { "" };
    let v = x.abs();
    let whole = v.trunc() as i64;
    let cents = ((v - v.trunc()) * 100.0).round() as i64;

    let (whole2, cents2) = if cents == 100 { (whole + 1, 0) } else { (whole, cents) };

    format!(
        "{}{}.{:02}",
        sign,
        whole2.to_formatted_string(&Locale::en),
        cents2
    )
}

fn sdmx_period_to_ym(p: &str) -> String {
    // "2025-M11" -> "2025-11"
    // If parsing fails, return original string.
    if p.len() == 8 && p.as_bytes()[4] == b'-' && p.as_bytes()[5] == b'M' {
        let year = &p[0..4];
        let mm = &p[6..8];
        if mm.chars().all(|c| c.is_ascii_digit()) {
            return format!("{}-{}", year, mm);
        }
    }
    p.to_string()
}

fn print_header(
    mode: Mode,
    country_name: &str,
    source_label: &str,
    indicator: &str,
    start_label: &str,
    latest_label: &str,
) {
    println!("================= Real Income (Inflation-Adjusted) =================");
    println!("Mode: {:?}", mode);
    println!("Country: {}", country_name);
    println!("Source: {}", source_label);
    println!("Indicator: {}", indicator);
    println!("Start: {}", start_label);
    println!("Latest: {}", latest_label);
    println!("=====================================================================");
}

fn print_results(nominal: f64, real_now: f64, loss: f64, loss_pct: f64) {
    println!("Nominal amount: {}", fmt_money(nominal));
    println!("Real value now: {}", fmt_money(real_now));
    println!("Purchasing-power loss: {} ({:.2}%)", fmt_money(loss), loss_pct);
}

fn print_formula_datamapper() {
    println!();
    println!("Formula (DataMapper / PCPIPCH annual %):");
    println!("  deflator = Π_y (1 + PCPIPCH_y / 100)");
    println!("  real_value = nominal / deflator");
}

// ----------------------- Jokes -----------------------
fn random_joke(loss_pct: f64) -> String {
    let mut rng = rand::thread_rng();

    let mild = vec![
        "Inflation never sleeps. Unfortunately, it also never pays rent.",
        "Your money has been doing static stretching: very still, slightly less useful.",
        "Plot twist: it's not you overspending - prices just got confident.",
        "CPI called. It said: 'Nice purchasing power you had there.'",
    ];

    let spicy = vec![
        "This might be a good time to ask for a raise. Or to negotiate directly with the CPI.",
        "Your salary time-traveled... without the cost-of-living adjustment.",
        "If your boss says 'no budget', reply: 'Cool, then let's cut inflation.'",
        "Inflation: 1 - Frozen salary: 0. Rematch at the next review!",
    ];

    let pool = if loss_pct >= 10.0 { spicy } else { mild };
    pool.choose(&mut rng).unwrap().to_string()
}

// ----------------------- SDMX runner -----------------------
async fn run_sdmx(
    client: &Client,
    cache_dir: &Path,
    use_cache: bool,
    verbose: bool,
    theme: &ColorfulTheme,
    country_arg: Option<String>,
    start_input: String,
    amount: f64,
    no_jokes: bool,
    end_input: Option<String>,
) -> Result<()> {
    let start_ym = parse_ym(&start_input).context("Start must be YYYY-MM for SDMX mode")?;

    // ---- Country selection ----
    // If user passed --country, don't depend on any metadata/codelist endpoint.
    // Otherwise load ISO3 country list from SDMX Central and show fuzzy picker.
    let (country_code, country_name) = match country_arg {
        Some(code) => {
            let code_up = code.trim().to_uppercase();
            (code_up.clone(), code_up) // name fallback = code
        }
        None => {
            let countries = sdmx_load_or_fetch_countries_iso3(client, cache_dir, use_cache).await?;
            prompt_fuzzy_pick(theme, "Select country (SDMX ISO3)", &countries)?
        }
    };

    // ---- Date range ----
    let today = chrono::Utc::now().date_naive();
    let current_ym = format!("{:04}-{:02}", today.year(), today.month());

    let mut end_ym = match end_input {
        Some(ref s) => parse_ym(s).context("End must be YYYY-MM for SDMX mode")?,
        None => current_ym.clone(),
    };

    if end_ym > current_ym {
        end_ym = current_ym.clone();
    }
    if end_ym < start_ym {
        return Err(anyhow!("--end must be >= start"));
    }

    // Convert YYYY-MM -> SDMX monthly periods like 2024-M01
    let start_period = ym_to_sdmx_period(&start_ym)?;
    let end_period = ym_to_sdmx_period(&end_ym)?;

    // CPI key (per your working example):
    // /data/CPI/POL.CPI._T.IX.M?startPeriod=2024-M01
    let series_key = format!(
        "{}.{}.{}.{}.{}",
        country_code, SDMX_CPI_INDEX_TYPE, SDMX_CPI_COICOP, SDMX_CPI_TRANSFORMATION, SDMX_CPI_FREQ
    );

    if verbose {
        eprintln!("Mode: SDMX");
        eprintln!("Country: {} ({})", country_name, country_code);
        eprintln!("Dataset: {}", SDMX_CPI_DATASET);
        eprintln!("Series key: {}", series_key);
        eprintln!("Range: {} → {}", start_period, end_period);
    }

    // Fetch CPI values from /data (SDMX-ML XML)
    let (start_period_used, cpi_start, latest_period, cpi_latest) =
        sdmx_fetch_cpi_start_and_latest(
            client,
            cache_dir,
            use_cache,
            &series_key,
            &start_period,
            &end_period,
        )
        .await?;

    let start_label = sdmx_period_to_ym(&start_period_used);
    let latest_label = sdmx_period_to_ym(&latest_period);
    let ratio = cpi_start / cpi_latest;
    let real_now = amount * ratio;
    let loss = amount - real_now;
    let loss_pct = (1.0 - ratio) * 100.0;

    print_header(
        Mode::Sdmx,
        &country_name,
        "IMF SDMX",
        "CPI index level",
        &start_label,
        &latest_label,
    );
    print_results(amount, real_now, loss, loss_pct);

    println!();
    println!("CPI index levels used (SDMX):");
    println!("  {}: {:.2}", start_label, cpi_start);
    println!("  {}: {:.2}", latest_label, cpi_latest);
    println!("  Inflation factor: {:.4}", cpi_latest / cpi_start);

    println!();
    println!("Formula (SDMX / CPI index level):");
    println!("  real_value = nominal * (CPI_start / CPI_latest)");

    if !no_jokes {
        println!();
        println!("{}", random_joke(loss_pct));
    }

    Ok(())
}

// ----------------------- DataMapper runner -----------------------
async fn run_datamapper(
    client: &Client,
    cache_dir: &Path,
    use_cache: bool,
    verbose: bool,
    theme: &ColorfulTheme,
    country_arg: Option<String>,
    start_input: String,
    amount: f64,
    no_jokes: bool,
    end_input: Option<String>,
) -> Result<()> {
    let start_year =
        parse_year_loose(&start_input).context("Start must be YYYY (or YYYY-MM) for DataMapper mode")?;

    let countries = datamapper_list_countries(client, cache_dir, use_cache).await?;
    let (country_code, country_name) = match country_arg {
        Some(code) => {
            let code_up = code.trim().to_uppercase();
            let name = countries
                .iter()
                .find(|x| x.code == code_up)
                .map(|x| x.name.clone())
                .ok_or_else(|| anyhow!("Country code '{}' not found in DataMapper countries list", code_up))?;
            (code_up, name)
        }
        None => prompt_fuzzy_pick(theme, "Select country (DataMapper ISO3)", &countries)?,
    };

    let current_year = chrono::Utc::now().date_naive().year();
    let mut end_year_used = match end_input {
        Some(ref s) => parse_year_loose(s).context("End must be YYYY (or YYYY-MM) for DataMapper mode")?,
        None => current_year,
    };

    if end_year_used > current_year {
        end_year_used = current_year;
    }
    if end_year_used < start_year {
        return Err(anyhow!("--end must be >= start year"));
    }

    if verbose {
        eprintln!("Mode: DataMapper");
        eprintln!("Country: {} ({})", country_name, country_code);
        eprintln!("Indicator: {}", DATAMAPPER_INDICATOR);
        eprintln!("Years: {} → {}", start_year, end_year_used);
    }

    let (deflator, latest_year, yearly) = datamapper_deflator_and_yearly_pcpipch(
        client,
        cache_dir,
        use_cache,
        &country_code,
        start_year,
        end_year_used,
    )
    .await?;

    let real_now = amount / deflator;
    let loss = amount - real_now;
    let loss_pct = (1.0 - (1.0 / deflator)) * 100.0;

    print_header(
        Mode::Datamapper,
        &country_name,
        "IMF DataMapper",
        DATAMAPPER_INDICATOR,
        &start_year.to_string(),
        &latest_year.to_string(),
    );
    print_results(amount, real_now, loss, loss_pct);

    println!();
    println!("Annual inflation rates used (PCPIPCH):");
    for yi in &yearly {
        println!("  {}: {:+.2}%", yi.year, yi.pct);
    }

    print_formula_datamapper();

    println!();
    println!("Note: DataMapper mode uses annual inflation rates (not monthly CPI index). SDMX mode is more precise.");

    if !no_jokes {
        println!();
        println!("{}", random_joke(loss_pct));
    }

    Ok(())
}

// ----------------------- SDMX: fetch ISO3 list (with retries + cache) -----------------------
async fn sdmx_load_or_fetch_countries_iso3(
    client: &Client,
    cache_dir: &Path,
    use_cache: bool,
) -> Result<Vec<Item>> {
    let cache_file = cache_dir.join("sdmx_countries_iso3.xml");

    let bytes = if use_cache { fs::read(&cache_file).ok() } else { None };

    let xml_bytes = match bytes {
        Some(b) => b,
        None => {
            // Fetch ONLY the ISO3 country codelist
            // (This matches the XML you pasted and includes POL, RUS, CHE, USA, etc.)
            let url = format!(
                "{}/codelist/IMF/{}/latest",
                IMF_SDMX_STRUCTURE_BASE,
                SDMX_CL_AREA_CPI
            );

            let resp = client
                .get(url)
                .send()
                .await
                .context("HTTP error fetching SDMX Central country codelist")?
                .error_for_status()
                .context("SDMX Central country codelist returned non-OK status")?;

            let b = resp.bytes().await?.to_vec();
            if use_cache {
                let _ = fs::write(&cache_file, &b);
            }
            b
        }
    };

    // Parse:
    // <str:Code id="POL"><com:Name xml:lang="en">Poland</com:Name></str:Code>
    let mut reader = Reader::from_reader(xml_bytes.as_slice());
    reader.trim_text(true);

    let mut buf = Vec::new();
    let mut out: Vec<Item> = Vec::new();

    let mut in_code = false;
    let mut current_id: Option<String> = None;
    let mut current_name: Option<String> = None;
    let mut capture_name_text = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                // quick-xml 0.31: avoid borrowing a temporary from e.name().as_ref()
                let name = e.name().as_ref().to_vec();

                if name.ends_with(b"Code") {
                    in_code = true;
                    current_id = None;
                    current_name = None;

                    for a in e.attributes().with_checks(false) {
                        let a = a?;
                        if a.key.as_ref().ends_with(b"id") {
                            current_id = Some(a.unescape_value()?.to_string());
                        }
                    }
                } else if in_code && name.ends_with(b"Name") {
                    // Prefer xml:lang="en"
                    let mut is_en = false;
                    for a in e.attributes().with_checks(false) {
                        let a = a?;
                        if a.key.as_ref().ends_with(b"lang") {
                            let v = a.unescape_value()?;
                            if v.eq_ignore_ascii_case("en") {
                                is_en = true;
                            }
                        }
                    }

                    // Capture if English OR if we don't have a name yet
                    capture_name_text = is_en || current_name.is_none();
                }
            }

            Ok(Event::Text(t)) => {
                if in_code && capture_name_text {
                    current_name = Some(t.unescape()?.to_string());
                }
            }

            Ok(Event::End(ref e)) => {
                let name = e.name().as_ref().to_vec();

                if name.ends_with(b"Name") {
                    capture_name_text = false;
                }

                if name.ends_with(b"Code") && in_code {
                    if let Some(id) = current_id.take() {
                        let name = current_name.take().unwrap_or_else(|| id.clone());
                        out.push(Item { code: id, name });
                    }
                    in_code = false;
                }
            }

            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!(e).context("Invalid SDMX Central codelist XML")),
            _ => {}
        }

        buf.clear();
    }

    if out.is_empty() {
        return Err(anyhow!("Parsed 0 country codes from SDMX Central"));
    }

    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

// ----------------------- SDMX: fetch CPI start + latest (NEW /data + SDMX-ML XML) -----------------------
async fn sdmx_fetch_cpi_start_and_latest(
    client: &Client,
    cache_dir: &Path,
    use_cache: bool,
    series_key: &str,
    start_period: &str,
    end_period: &str,
) -> Result<(String, f64, String, f64)> {
    let cache_key = format!(
        "sdmx_cpi_xml_{}_{}_{}.xml",
        series_key.replace('.', "_"),
        start_period.replace('-', ""),
        end_period.replace('-', "")
    );
    let cache_file = cache_dir.join(cache_key);

    let bytes = if use_cache { fs::read(&cache_file).ok() } else { None };

    let xml_bytes = match bytes {
        Some(b) => b,
        None => {
            // NEW ENDPOINT:
            //   /data/CPI/{series_key}?startPeriod=YYYY-MMM&endPeriod=YYYY-MMM
            let url = format!(
                "{}/data/{}/{}?startPeriod={}&endPeriod={}",
                IMF_SDMX_BASE, SDMX_CPI_DATASET, series_key, start_period, end_period
            );

            let resp = client
                .get(url)
                .send()
                .await
                .context("HTTP error fetching SDMX data")?
                .error_for_status()
                .context("SDMX data returned non-OK status")?;

            let b = resp.bytes().await?.to_vec();
            if use_cache {
                let _ = fs::write(&cache_file, &b);
            }
            b
        }
    };

    // Parse <Obs TIME_PERIOD="2020-M01" OBS_VALUE="..." .../>
    let mut reader = Reader::from_reader(xml_bytes.as_slice());
    reader.trim_text(true);

    let mut buf = Vec::new();
    let mut obs: Vec<(String, f64)> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                // In this feed, Obs is typically <Obs .../>
                if e.name().as_ref().ends_with(b"Obs") {
                    let mut tp: Option<String> = None;
                    let mut val: Option<f64> = None;

                    for a in e.attributes().with_checks(false) {
                        let a = a?;
                        let k = a.key.as_ref();

                        if k.ends_with(b"TIME_PERIOD") {
                            tp = Some(a.unescape_value()?.to_string());
                        } else if k.ends_with(b"OBS_VALUE") {
                            let s = a.unescape_value()?;
                            val = Some(s.parse::<f64>().context("OBS_VALUE not numeric")?);
                        }
                    }

                    if let (Some(t), Some(v)) = (tp, val) {
                        if v > 0.0 {
                            obs.push((t, v));
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!(e).context("Invalid SDMX XML")),
            _ => {}
        }
        buf.clear();
    }

    if obs.is_empty() {
        return Err(anyhow!("No observations found in SDMX XML response"));
    }

    // TIME_PERIOD sorts lexicographically for "YYYY-MMM" format
    obs.sort_by(|a, b| a.0.cmp(&b.0));

    let start_obs = obs
        .iter()
        .find(|(t, _)| t.as_str() >= start_period)
        .ok_or_else(|| anyhow!("No CPI data found at/after start date (start too early?)"))?;

    let latest_obs = obs.last().ok_or_else(|| anyhow!("No CPI data found"))?;

    let cpi_start = start_obs.1;
    let cpi_latest = latest_obs.1;

    if cpi_start <= 0.0 || cpi_latest <= 0.0 {
        return Err(anyhow!("Invalid CPI values (<= 0)"));
    }

    Ok((
        start_obs.0.clone(),
        cpi_start,
        latest_obs.0.clone(),
        cpi_latest,
    ))
}

// ----------------------- DataMapper: anti-403 client -----------------------
fn build_datamapper_client() -> Result<Client> {
    Client::builder()
        .http1_only()
        .cookie_store(true)
        .user_agent("curl/8.5.0")
        .default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert(ACCEPT, "application/json,text/plain,*/*".parse().unwrap());
            h.insert(ACCEPT_LANGUAGE, "en-US,en;q=0.9".parse().unwrap());
            h.insert(REFERER, "https://www.imf.org/external/datamapper/".parse().unwrap());
            h
        })
        .build()
        .context("Failed to build DataMapper HTTP client")
}

// ----------------------- DataMapper: list countries -----------------------
async fn datamapper_list_countries(client: &Client, cache_dir: &Path, use_cache: bool) -> Result<Vec<Item>> {
    let cache_file = cache_dir.join("dm_countries.json");

    if use_cache {
        if let Ok(b) = fs::read(&cache_file) {
            if let Ok(v) = serde_json::from_slice::<Vec<Item>>(&b) {
                if !v.is_empty() {
                    return Ok(v);
                }
            }
        }
    }

    let url = format!("{}/countries", IMF_DATAMAPPER_BASE);
    let resp = client
        .get(url)
        .send()
        .await
        .context("HTTP error fetching DataMapper countries")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("DataMapper countries returned {}.\nBody:\n{}", status, body));
    }

    let json: Value = resp.json().await.context("Invalid JSON from DataMapper countries")?;
    let obj = json.get("countries").cloned().unwrap_or(json);
    let map = obj
        .as_object()
        .ok_or_else(|| anyhow!("Unexpected DataMapper countries JSON shape"))?;

    let mut out = Vec::with_capacity(map.len());
    for (code, info) in map {
        let name = info
            .get("label")
            .and_then(|x| x.as_str())
            .unwrap_or(code)
            .to_string();
        out.push(Item { code: code.to_string(), name });
    }

    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    if use_cache {
        let _ = fs::write(&cache_file, serde_json::to_vec_pretty(&out)?);
    }

    Ok(out)
}

// ----------------------- DataMapper: fetch PCPIPCH, return (deflator, latest_year, yearly_values) -----------------------
async fn datamapper_deflator_and_yearly_pcpipch(
    client: &Client,
    cache_dir: &Path,
    use_cache: bool,
    country_iso3: &str,
    start_year: i32,
    end_year: i32,
) -> Result<(f64, i32, Vec<YearInflation>)> {
    let years: Vec<i32> = (start_year..=end_year).collect();
    let periods = years
        .iter()
        .map(|y| y.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let cache_key = format!("dm_{}_{}_{}_{}.json", DATAMAPPER_INDICATOR, country_iso3, start_year, end_year);
    let cache_file = cache_dir.join(cache_key);

    let bytes = if use_cache { fs::read(&cache_file).ok() } else { None };

    let json_bytes = match bytes {
        Some(b) => b,
        None => {
            let url = format!(
                "{}/{}/{}?periods={}",
                IMF_DATAMAPPER_BASE, DATAMAPPER_INDICATOR, country_iso3, periods
            );
            let resp = client
                .get(url)
                .send()
                .await
                .context("HTTP error fetching DataMapper PCPIPCH values")?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(anyhow!("DataMapper values returned {}.\nBody:\n{}", status, body));
            }

            let b = resp.bytes().await?.to_vec();
            if use_cache {
                let _ = fs::write(&cache_file, &b);
            }
            b
        }
    };

    let json: Value = serde_json::from_slice(&json_bytes).context("Invalid JSON from DataMapper values")?;

    let values = json
        .get("values")
        .ok_or_else(|| anyhow!("Unexpected DataMapper response (missing 'values')"))?;

    let series = values
        .get(DATAMAPPER_INDICATOR)
        .and_then(|v| v.get(country_iso3))
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("No data for {} / {}", DATAMAPPER_INDICATOR, country_iso3))?;

    let mut deflator = 1.0_f64;
    let mut latest_year: Option<i32> = None;
    let mut yearly: Vec<YearInflation> = Vec::new();

    for y in years {
        let key = y.to_string();
        if let Some(val) = series.get(&key) {
            if let Some(pi) = val.as_f64() {
                yearly.push(YearInflation { year: y, pct: pi });
                deflator *= 1.0 + (pi / 100.0);
                latest_year = Some(y);
            }
        }
    }

    let latest_year = latest_year.ok_or_else(|| anyhow!("No numeric observations found"))?;
    Ok((deflator, latest_year, yearly))
}
