# PRD: Atlanta Flights On-Time Performance (OTP) Dashboard

## 1. Overview
A Python application that reads flight, airport, and airline reference data from
`Flights_from_Atlanta_2025.xlsx` (located in the project root directory) and produces an
interactive dashboard analyzing On-Time Performance (OTP) for flights departing Atlanta (ATL)
in 2025.

**OTP Definition:** A flight is considered "on-time" if arrival delay < 15 minutes.
`OTP% = (Flights with arrival delay < 15 min / Total flights) × 100`

## 2. Data Source

**File:** `Flights_from_Atlanta_2025.xlsx` (project root)

Confirmed against the actual workbook (107,947 flight rows, all originating from ATL):

### Tab: `Flights from ATL` (26 columns)
| Column | Type | Notes |
|---|---|---|
| `YEAR`, `MONTH`, `DAY`, `DAY_OF_WEEK` | int | All records are `YEAR = 2025`. Months present: **Jan–Sep, Nov, Dec** (Oct is entirely missing — 11 of 12 months present). |
| `AIRLINE` | str | 2-letter IATA airline code (11 distinct: AA, AS, DL, EV, F9, MQ, NK, OO, UA, US, WN). Joins to `Airlines.IATA_CODE`. |
| `FLIGHT_NUMBER`, `TAIL_NUMBER` | int/str | Not needed for OTP aggregation. |
| `ORIGIN_AIRPORT`, `OR_CITY`, `OR_STATE`, `OR_COUNTRY` | str | Always `ATL` / Atlanta — origin is constant, informational only. |
| `SCHEDULED_DEPARTURE` | int | **HHMM integer format** (e.g., `2101` = 21:01, `500` = 05:00). Range observed: 500–2330. Used to derive flight session bucket. |
| `DEPARTURE_TIME` | int | Actual departure, HHMM format. |
| `DEPARTURE_DELAY` | int | Minutes. |
| `DESTINATION_AIRPORT`, `DEST_CITY`, `DEST_STATE`, `DEST_COUNTRY` | str | 168 distinct destination airports. All codes verified present in `Airports.IATA_CODE` (no orphans). |
| `SCHEDULED_ARRIVAL`, `ARRIVAL_TIME` | int | HHMM format. |
| `ARRIVAL_DELAY` | int | Minutes, **no missing values** (0 NaNs across all 107,947 rows) — dataset appears pre-filtered to completed (non-cancelled/non-diverted) flights only. **This is the field used for OTP.** |
| `AIR_SYSTEM_DELAY`, `SECURITY_DELAY`, `AIRLINE_DELAY`, `LATE_AIRCRAFT_DELAY`, `WEATHER_DELAY` | float | Delay-cause breakdown (has NaNs when delay cause N/A). Not required for OTP% but available for future "why" drill-downs. |

### Tab: `Airlines` (2 columns, 11 rows)
`IATA_CODE` (str) → `AIRLINE` (str, full name). 1:1 match with all airline codes in the flights tab.

### Tab: `Airports` (7 columns, 217 rows)
`IATA_CODE`, `AIRPORT` (full name), `CITY`, `STATE`, `COUNTRY`, `LATITUDE` (float), `LONGITUDE` (float).
All 168 destination codes used in the flights tab are present here — clean join, no missing lookups.

**Resolved assumptions (confirmed from real data):**
- No cancellation/diversion flag exists — dataset is pre-filtered to completed flights, so **no
  exclusion logic is needed** for OTP denominator.
- `ARRIVAL_DELAY` has zero missing values — safe to use directly for the `is_on_time` flag.
- October is **fully absent** (not partially missing) — all Oct OTP%, chart values, and any
  Oct-specific KPIs must come from the Sep/Nov interpolation.
- Departure times are HHMM integers, not datetime — session bucketing must parse hours via
  `// 100` (or equivalent) rather than string time parsing.

## 3. Functional Requirements

### 3.1 Data Ingestion & Processing
- Load all 3 tabs via `pandas`/`openpyxl`.
- Join Flights → Airline (on airline code) and Flights → Airport (on destination IATA code).
- Derive fields:
  - `is_on_time` = arrival delay < 15 min (boolean)
  - `month` = month extracted from flight date
  - `flight_session` = bucketed from scheduled departure time:
    - **AM**: 05:00–11:59
    - **PM**: 12:00–16:59
    - **Night**: 17:00–20:59
    - **Red-Eye**: 21:00–04:59
    - *(Buckets configurable; documented explicitly in code comments/config since no official FAA standard exists.)*
- Data quality handling: drop/flag records with missing critical fields (delay, date, airline, destination); log counts of excluded records.

### 3.2 OTP Calculations
- **By Airline:** OTP% per airline across all flights.
- **By Destination Airport:** OTP% per destination airport.
- **By Month:** OTP% per calendar month (Jan–Dec).
  - **October gap-filling:** Oct 2025 data is missing from the source file. Compute Oct OTP% via
    **linear interpolation** between Sep and Nov OTP% values: `Oct_OTP = (Sep_OTP + Nov_OTP) / 2`.
    This estimated value must be **visually flagged** in the chart (e.g., dashed line segment,
    distinct marker, or annotation "Interpolated") to distinguish it from actual observed data.
- **By Flight Session:** OTP% per AM/PM/Night/Red-Eye bucket.

### 3.3 Dashboard Components

1. **KPI Cards**
   - Best-performing airline (highest OTP%) + its OTP%
   - Worst-performing airline (lowest OTP%) + its OTP%
   - Best-performing destination airport (highest OTP%) + its OTP%
   - Worst-performing destination airport (lowest OTP%) + its OTP%
   - Minimum flight-count threshold applied before ranking (e.g., exclude airlines/airports with
     too few flights to avoid misleading small-sample OTP%; threshold configurable)

2. **OTP% by Month — Line/Bar Chart**
   - X-axis: Jan–Dec 2025
   - Y-axis: OTP%
   - Oct data point visually distinguished as interpolated (dashed segment + label/tooltip note)

3. **OTP% by Flight Session — Bar Chart**
   - Categories: AM, PM, Night, Red-Eye
   - Y-axis: OTP%
   - Optional: overlay flight volume per session for context

4. **OTP% by Destination Airport — Map**
   - Geographic map (using Airport tab lat/long) with markers/choropleth-style coloring by OTP%
   - Marker size or color intensity encodes OTP% (or flight volume, or both via size+color)
   - Tooltip/hover: airport name, IATA code, OTP%, total flights

### 3.4 Interactivity (recommended, scope to confirm)
- Filters: date range, airline, minimum flight-count threshold
- Hover tooltips on all charts showing underlying counts (numerator/denominator), not just %

## 4. Non-Functional Requirements
- **Tech stack:** Python; `pandas` + `openpyxl` for data; `plotly`/`dash` or `streamlit` for the
  dashboard (recommend **Streamlit + Plotly** for fast build and interactive map support).
- **Performance:** Dashboard should load/render in a few seconds for a single-year, single-origin
  (ATL) dataset.
- **Reproducibility:** Data source path configurable (default: project root); no hardcoded absolute paths.
- **Documentation:** Inline comments explaining OTP definition, session bucket boundaries, and the
  October interpolation method.

## 5. Out of Scope
- Real-time or live flight data (static file-based analysis only)
- Predictive delay modeling
- Multi-origin-airport support (ATL-specific for this version)

## 6. Open Questions (to confirm before/at build start)
1. ~~Exact column names/schema for each of the 3 tabs~~ — **Resolved**, see Section 2.
2. ~~How cancelled/diverted flights should be treated~~ — **Resolved**: no such flights in dataset.
3. Definition boundaries for AM/PM/Night/Red-Eye — confirm proposed times above are acceptable
   (departure range in data is 05:00–23:30, so Red-Eye is the smaller/tail bucket).
4. Preferred dashboard framework: Streamlit vs. Dash vs. Jupyter-based. **Recommend Streamlit +
   Plotly** (fast to build, native `st.map`/Plotly map support, easy KPI cards).
5. Minimum flight-count threshold for "best/worst" KPI eligibility (with 168 destinations, some
   likely have very few flights/year — recommend a default threshold, e.g. ≥ 50 flights, configurable).

## 7. Deliverables
- `app.py` (or `dashboard.py`) — main dashboard application
- `data_processing.py` — data loading, joining, OTP calculation, session bucketing, Oct interpolation logic
- `PRD.md` — this document
- `README.md` — setup/run instructions
