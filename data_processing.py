"""Data loading and OTP calculations for the Atlanta flights dashboard.

OTP definition (FAA-style threshold used here):
    A flight is on-time if ARRIVAL_DELAY < 15 minutes.
    OTP% = (on-time flights / total flights) * 100

Flight session buckets (configurable; not an official FAA standard):
    AM      05:00–11:59
    PM      12:00–16:59
    Night   17:00–20:59
    Red-Eye 21:00–04:59

October 2025 is missing from the source workbook. Monthly OTP for October is
filled by linear interpolation: Oct_OTP = (Sep_OTP + Nov_OTP) / 2.
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Iterable

import pandas as pd

logger = logging.getLogger(__name__)

DEFAULT_DATA_PATH = Path(__file__).resolve().parent / "Flights_from_Atlanta_2025.xlsx"

# Sheet names from Flights_from_Atlanta_2025.xlsx
FLIGHTS_SHEET = "Flights from ATL"
AIRLINES_SHEET = "Airlines"
AIRPORTS_SHEET = "Airports"

# OTP: arrival delay strictly less than this many minutes counts as on-time.
OTP_DELAY_THRESHOLD_MIN = 15

# Default minimum flights before an airline/airport is eligible for best/worst KPIs.
DEFAULT_MIN_FLIGHTS = 50

# Session buckets by scheduled departure hour (HH from HHMM integer).
# Red-Eye wraps midnight: hours 21–23 and 0–4.
SESSION_ORDER = ("AM", "PM", "Night", "Red-Eye")

MONTH_NAMES = {
    1: "Jan",
    2: "Feb",
    3: "Mar",
    4: "Apr",
    5: "May",
    6: "Jun",
    7: "Jul",
    8: "Aug",
    9: "Sep",
    10: "Oct",
    11: "Nov",
    12: "Dec",
}

CRITICAL_COLUMNS = (
    "ARRIVAL_DELAY",
    "YEAR",
    "MONTH",
    "DAY",
    "AIRLINE",
    "DESTINATION_AIRPORT",
    "SCHEDULED_DEPARTURE",
)


def session_from_hhmm(hhmm: int | float) -> str:
    """Map SCHEDULED_DEPARTURE (HHMM integer) to a flight session bucket."""
    hour = int(hhmm) // 100
    if 5 <= hour <= 11:
        return "AM"
    if 12 <= hour <= 16:
        return "PM"
    if 17 <= hour <= 20:
        return "Night"
    return "Red-Eye"


def load_raw_tables(data_path: str | Path = DEFAULT_DATA_PATH) -> tuple[pd.DataFrame, pd.DataFrame, pd.DataFrame]:
    """Load the three workbook tabs."""
    path = Path(data_path)
    if not path.exists():
        raise FileNotFoundError(f"Flight data not found: {path}")

    flights = pd.read_excel(path, sheet_name=FLIGHTS_SHEET)
    airlines = pd.read_excel(path, sheet_name=AIRLINES_SHEET)
    airports = pd.read_excel(path, sheet_name=AIRPORTS_SHEET)
    return flights, airlines, airports


def prepare_flights(
    flights: pd.DataFrame,
    airlines: pd.DataFrame,
    airports: pd.DataFrame,
) -> pd.DataFrame:
    """Join reference tables, drop incomplete rows, and derive OTP fields."""
    n_raw = len(flights)

    missing_critical = flights[list(CRITICAL_COLUMNS)].isna().any(axis=1)
    n_dropped = int(missing_critical.sum())
    if n_dropped:
        logger.warning(
            "Dropping %s of %s rows missing critical fields (%s)",
            n_dropped,
            n_raw,
            ", ".join(CRITICAL_COLUMNS),
        )
    flights = flights.loc[~missing_critical].copy()

    airlines = airlines.rename(columns={"AIRLINE": "AIRLINE_NAME"})
    airports = airports.rename(
        columns={
            "IATA_CODE": "DESTINATION_AIRPORT",
            "AIRPORT": "DEST_AIRPORT_NAME",
            "CITY": "DEST_AIRPORT_CITY",
            "STATE": "DEST_AIRPORT_STATE",
            "COUNTRY": "DEST_AIRPORT_COUNTRY",
            "LATITUDE": "LATITUDE",
            "LONGITUDE": "LONGITUDE",
        }
    )

    df = flights.merge(
        airlines,
        left_on="AIRLINE",
        right_on="IATA_CODE",
        how="left",
        suffixes=("", "_airline"),
    )
    if "IATA_CODE" in df.columns:
        df = df.drop(columns=["IATA_CODE"])

    airport_cols = [
        "DESTINATION_AIRPORT",
        "DEST_AIRPORT_NAME",
        "DEST_AIRPORT_CITY",
        "DEST_AIRPORT_STATE",
        "DEST_AIRPORT_COUNTRY",
        "LATITUDE",
        "LONGITUDE",
    ]
    df = df.merge(airports[airport_cols], on="DESTINATION_AIRPORT", how="left")

    # OTP flag: arrival delay < 15 minutes.
    df["is_on_time"] = df["ARRIVAL_DELAY"] < OTP_DELAY_THRESHOLD_MIN
    df["flight_date"] = pd.to_datetime(
        dict(year=df["YEAR"], month=df["MONTH"], day=df["DAY"]),
        errors="coerce",
    )
    df["month"] = df["MONTH"].astype(int)
    df["flight_session"] = df["SCHEDULED_DEPARTURE"].map(session_from_hhmm)
    df["airline_label"] = df["AIRLINE_NAME"].fillna(df["AIRLINE"])
    df["airport_label"] = df["DEST_AIRPORT_NAME"].fillna(df["DESTINATION_AIRPORT"])

    logger.info("Prepared %s flights (%s excluded)", len(df), n_dropped)
    return df


def load_and_prepare(data_path: str | Path = DEFAULT_DATA_PATH) -> pd.DataFrame:
    """Convenience loader: read workbook and return analysis-ready flights."""
    flights, airlines, airports = load_raw_tables(data_path)
    return prepare_flights(flights, airlines, airports)


def _otp_summary(group: pd.DataFrame) -> pd.Series:
    total = int(len(group))
    on_time = int(group["is_on_time"].sum())
    otp_pct = (on_time / total * 100.0) if total else float("nan")
    return pd.Series({"flights": total, "on_time": on_time, "otp_pct": otp_pct})


def _cast_otp_counts(df: pd.DataFrame) -> pd.DataFrame:
    out = df.copy()
    out["flights"] = out["flights"].astype(int)
    out["on_time"] = out["on_time"].astype(int)
    return out


def otp_by_airline(df: pd.DataFrame) -> pd.DataFrame:
    """OTP% by airline (code + display name)."""
    out = (
        df.groupby(["AIRLINE", "airline_label"], dropna=False)
        .apply(_otp_summary, include_groups=False)
        .reset_index()
        .sort_values("otp_pct", ascending=False)
    )
    return _cast_otp_counts(out)


def otp_by_destination(df: pd.DataFrame) -> pd.DataFrame:
    """OTP% by destination airport, including lat/long for mapping."""
    agg_cols = [
        "DESTINATION_AIRPORT",
        "airport_label",
        "DEST_AIRPORT_CITY",
        "DEST_AIRPORT_STATE",
        "LATITUDE",
        "LONGITUDE",
    ]
    present = [c for c in agg_cols if c in df.columns]
    out = (
        df.groupby(present, dropna=False)
        .apply(_otp_summary, include_groups=False)
        .reset_index()
        .sort_values("otp_pct", ascending=False)
    )
    return _cast_otp_counts(out)


def otp_by_session(df: pd.DataFrame) -> pd.DataFrame:
    """OTP% by AM / PM / Night / Red-Eye session."""
    out = (
        df.groupby("flight_session", dropna=False)
        .apply(_otp_summary, include_groups=False)
        .reset_index()
    )
    out["flight_session"] = pd.Categorical(
        out["flight_session"], categories=list(SESSION_ORDER), ordered=True
    )
    return _cast_otp_counts(out.sort_values("flight_session"))


def otp_by_month(df: pd.DataFrame) -> pd.DataFrame:
    """OTP% by calendar month with October linearly interpolated when missing.

    Oct_OTP = (Sep_OTP + Nov_OTP) / 2 when October has no observed flights.
    The interpolated row is flagged with ``is_interpolated=True``.
    """
    observed = (
        df.groupby("month", dropna=False)
        .apply(_otp_summary, include_groups=False)
        .reset_index()
    )
    observed["is_interpolated"] = False

    all_months = pd.DataFrame({"month": list(range(1, 13))})
    monthly = all_months.merge(observed, on="month", how="left")
    # Keep a real boolean dtype — after merge this can become float/object, and
    # `~series` would then bitwise-negate ints (-1/-2) instead of filtering rows.
    monthly["is_interpolated"] = monthly["is_interpolated"].fillna(False).astype(bool)

    # Fill October via Sep/Nov average when Oct is absent (expected for this dataset).
    oct_missing = monthly.loc[monthly["month"] == 10, "flights"].isna().all()
    if oct_missing:
        sep = monthly.loc[monthly["month"] == 9, "otp_pct"]
        nov = monthly.loc[monthly["month"] == 11, "otp_pct"]
        if not sep.empty and not nov.empty and pd.notna(sep.iloc[0]) and pd.notna(nov.iloc[0]):
            monthly.loc[monthly["month"] == 10, "otp_pct"] = (sep.iloc[0] + nov.iloc[0]) / 2.0
            monthly.loc[monthly["month"] == 10, "flights"] = 0
            monthly.loc[monthly["month"] == 10, "on_time"] = 0
            monthly.loc[monthly["month"] == 10, "is_interpolated"] = True

    monthly["month_name"] = monthly["month"].map(MONTH_NAMES)
    monthly["flights"] = monthly["flights"].fillna(0).astype(int)
    monthly["on_time"] = monthly["on_time"].fillna(0).astype(int)
    monthly["is_interpolated"] = monthly["is_interpolated"].astype(bool)
    return monthly.sort_values("month").reset_index(drop=True)


def filter_flights(
    df: pd.DataFrame,
    airlines: Iterable[str] | None = None,
    date_start: pd.Timestamp | None = None,
    date_end: pd.Timestamp | None = None,
) -> pd.DataFrame:
    """Apply optional airline and date-range filters."""
    out = df
    if airlines:
        airline_set = set(airlines)
        out = out[out["AIRLINE"].isin(airline_set)]
    if date_start is not None:
        out = out[out["flight_date"] >= pd.Timestamp(date_start)]
    if date_end is not None:
        out = out[out["flight_date"] <= pd.Timestamp(date_end)]
    return out.copy()


def rank_extremes(
    summary: pd.DataFrame,
    label_col: str,
    min_flights: int = DEFAULT_MIN_FLIGHTS,
) -> tuple[pd.Series | None, pd.Series | None]:
    """Return best and worst OTP rows meeting the minimum flight threshold."""
    eligible = summary[summary["flights"] >= min_flights]
    if eligible.empty:
        return None, None
    best = eligible.loc[eligible["otp_pct"].idxmax()]
    worst = eligible.loc[eligible["otp_pct"].idxmin()]
    # Attach which column is the display label for callers.
    best = best.copy()
    worst = worst.copy()
    best["display_label"] = best[label_col]
    worst["display_label"] = worst[label_col]
    return best, worst
