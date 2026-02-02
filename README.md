# RIP

**RIP** is a command-line tool that computes the inflation-adjusted (real) value of income using official IMF data.

It answers a simple but important question: "What is my income worth today, in purchasing-power terms?"

RIP supports two calculation methods:
* **Monthly CPI index (SDMX)** - precise, price-level based
* **Annual inflation rates (DataMapper)** - approximate, yearly

## Features

* 📈 Accurate inflation adjustment
* 🌍 Official IMF data sources
* 📅 Monthly or annual resolution
* 🧮 Transparent formulas
* 🧠 Economically correct methodology
* 🖥️ Interactive or fully scripted CLI
* 😄 *Optional jokes about inflation*

## Data Sources

RIP uses authoritative IMF APIs:

**SDMX mode**
* IMF SDMX Data API
* Dataset: CPI (Consumer Price Index)
* Monthly CPI index levels
* Most precise for time-based comparisons

**DataMapper mode**
* IMF DataMapper API
* Indicator: PCPIPCH
* Annual inflation rates
* Useful fallback when monthly CPI is unavailable

## Economic Meaning

**What does “Real Income in Purchasing-power terms” mean?**

It means expressing income in constant prices, adjusting for inflation so that values are comparable over time.

In other words: "How much can my income actually buy, after prices have changed?"

## Formulas

### SDMX mode (CPI index level)

```math
RealIncome = NominalIncome × (CPI_\text{start} / CPI_\text{latest})
```

Where:
* `CPI_start` is the CPI index value at the start period (e.g. `2024-01`)
* `CPI_latest` is the CPI index value at the latest available period
* CPI values are index levels (not percentage changes)

This method:
* Uses actual price levels
* Requires no averaging or compounding

### DataMapper mode (annual inflation rates)

```math
Deflator = \prod_{\gamma} \left(1 + \frac{PCPIPCH_{\gamma}}{100}\right)
```

```math
RealIncome = NominalIncome / Deflator
```

Where:
* `PCPIPCHᵧ` is the annual inflation rate (%) for year y
* The product runs over all years between `start` and `end`

This is an approximation, because:
* Inflation is averaged annually
* Intra-year price dynamics are ignored

## Installation

Requirements
* Rust (stable)
* Cargo

### Build
```shell
cargo build --release
```

### Run
```shell
cargo run --release -- [OPTIONS]
```

### Usage

SDMX mode (monthly CPI)
```shell
cargo run --release -- \
  --mode sdmx \
  --country ITA \
  --start 2024-01 \
  --end 2025-12 \
  --amount 10000
```

DataMapper mode (annual inflation)
```shell
cargo run --release -- \
  --mode datamapper \
  --country ITA \
  --start 2024 \
  --end 2025 \
  --amount 10000
```

### Options

| Flag         | Description                             |
| ------------ | --------------------------------------- |
| `--mode`     | `sdmx` or `datamapper`                  |
| `--country`  | ISO-3 country code (e.g. ITA, USA, DEU) |
| `--start`    | Start date (`YYYY-MM` or `YYYY`)        |
| `--end`      | End date (`YYYY-MM` or `YYYY`)          |
| `--amount`   | Nominal income amount                   |
| `--cache`    | Enable on-disk caching (default: true)  |
| `--no-jokes` | Disable inflation jokes                 |
| `--verbose`  | Print debug info                        |

If `--country`, `--start`, or `--amount` are omitted, RIP will prompt interactively.


## Use Cases

Here some example of use cases.

| Use case                      | Mode       |
| ----------------------------- | ---------- |
| Salary erosion over time      | SDMX       |
| Monthly precision             | SDMX       |
| Long-term historical estimate | DataMapper |
| Missing CPI data              | DataMapper |


## Notes & Caveats
* RIP compares time periods within the same country
* It does not perform cross-country PPP comparisons
* "Purchasing-power terms" here refers to domestic price inflation, not PPP exchange rates
* For cross-country analysis, PPP data would be required (not currently implemented)
