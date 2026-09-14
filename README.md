# Atlanta Flights OTP Dashboard

Interactive Streamlit + Plotly dashboard for On-Time Performance (OTP) of flights
departing Atlanta (ATL) in 2025, built from `prd.md`.

## OTP definition

A flight is **on-time** if `ARRIVAL_DELAY < 15` minutes.

`OTP% = (on-time flights / total flights) × 100`

October 2025 is missing from the source file. Monthly Oct OTP is filled by linear
interpolation: `(Sep OTP + Nov OTP) / 2`, and is visually flagged on the chart.

## Requirements

- Python 3.10+
- Packages: `pandas`, `openpyxl`, `plotly`, `streamlit`

```bash
python -m venv .venv
source .venv/bin/activate
pip install pandas openpyxl plotly streamlit
```

## Data

Place the workbook at the project root (default path):

`Flights_from_Atlanta_2025.xlsx`

Or pass another path in the sidebar **Data file path** control.

Expected sheets: `Flights from ATL`, `Airlines`, `Airports`.

## Run

```bash
source .venv/bin/activate
streamlit run dashboard.py
```

## Project files

| File | Role |
|---|---|
| `data_processing.py` | Load/join Excel tabs, derive OTP + session buckets, Oct interpolation |
| `dashboard.py` | Streamlit UI: KPI cards, month/session charts, destination map, filters |
| `prd.md` | Product requirements blueprint |

## Dashboard features

- KPI cards: best/worst airline and destination (min-flight threshold applied)
- OTP% by month (Oct interpolated + dashed bridge)
- OTP% by flight session (AM / PM / Night / Red-Eye) with flight-volume overlay
- Map of destination OTP% (color = OTP, size = flights)
- Filters: airline, date range, minimum flight count
