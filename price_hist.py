"""price_hist.py

Fetch daily closing prices for a stock ticker using the Yahoo Finance data API
(via the yfinance library). Supports specific date ranges or predefined periods.

Usage:
    python price_hist.py [TICKER] [--start YYYY-MM-DD --end YYYY-MM-DD] [--period PERIOD]
    Example: python price_hist.py AAPL --start 2023-01-01 --end 2023-12-31
"""

import argparse
from datetime import datetime
from typing import Optional

import matplotlib.pyplot as plt
import pandas as pd
import yfinance as yf


def get_price_history(
    ticker: str, start: str | None = None, end: str | None = None, period: str = "5d"
) -> pd.DataFrame:
    """Return a DataFrame of daily closing prices for ``ticker``.

    If both start and end are provided, it fetches data for that range.
    Otherwise, it fetches data for the specified period.
    """
    # Use start/end if provided, otherwise use period
    download_kwargs = {
        "interval": "1d",
        "progress": False,
        "auto_adjust": True,
    }
    if start and end:
        download_kwargs["start"] = start
        download_kwargs["end"] = end
    else:
        download_kwargs["period"] = period

    data = yf.download(ticker, **download_kwargs)

    if data.empty:
        return data

    # yf.download can return a MultiIndex column even for a single symbol;
    # flatten it to a simple column layout.
    if isinstance(data.columns, pd.MultiIndex):
        data.columns = data.columns.get_level_values(0)

    closes = data[["Close"]] if "Close" in data.columns else data.iloc[:, :1]
    return closes.sort_index()


def get_option_walls(ticker: str) -> tuple[float | None, float | None]:
    """Fetch the strike prices with the highest Open Interest for calls and puts.

    Uses the nearest available expiration date.
    """
    try:
        t = yf.Ticker(ticker)
        expirations = t.options
        if not expirations:
            return None, None

        # Use the nearest expiration
        expiry = expirations[0]
        chain = t.option_chain(expiry)

        calls = chain.calls
        puts = chain.puts

        call_wall = None
        if not calls.empty:
            call_wall = float(calls.loc[calls["openInterest"].idxmax(), "strike"])

        put_wall = None
        if not puts.empty:
            put_wall = float(puts.loc[puts["openInterest"].idxmax(), "strike"])

        return call_wall, put_wall
    except Exception as e:
        print(f"Warning: Could not fetch option walls for {ticker}: {e}")
        return None, None


def plot_price_history(
    closes: pd.DataFrame,
    ticker: str,
    call_wall: float | None = None,
    put_wall: float | None = None,
    output_file: str = "price_chart.png",
) -> None:
    """Create a line plot of closing prices and save it to a file."""
    plt.figure(figsize=(10, 6))
    plt.plot(closes.index, closes["Close"], label="Closing Price", color="#1f77b4", lw=2)

    if call_wall is not None:
        plt.axhline(y=call_wall, color="green", linestyle="--", lw=1.5, label=f"Call Wall ({call_wall:.2f})")
    if put_wall is not None:
        plt.axhline(y=put_wall, color="red", linestyle="--", lw=1.5, label=f"Put Wall ({put_wall:.2f})")

    # Add walls to y-axis ticks
    current_ticks = list(plt.gca().get_yticks())
    extra_ticks = [t for t in [call_wall, put_wall] if t is not None]
    plt.yticks(sorted(set(current_ticks + extra_ticks)))

    plt.title(f"Closing Price History for {ticker}", fontsize=14, fontweight="bold")
    plt.xlabel("Date", fontsize=12)
    plt.ylabel("Price (USD)", fontsize=12)
    plt.legend()
    plt.grid(True, alpha=0.3)
    plt.tight_layout()

    plt.savefig(output_file)
    plt.close()
    print(f"\nPrice chart saved to {output_file}")


def main() -> int:
    parser = argparse.ArgumentParser(description="Fetch and plot stock price history.")
    parser.add_argument("ticker", nargs="?", default="SPCX", help="Stock ticker symbol")
    parser.add_argument("--start", help="Start date (YYYY-MM-DD)")
    parser.add_argument("--end", help="End date (YYYY-MM-DD)")
    parser.add_argument("--period", default="5d", help="Period to fetch (e.g., 1mo, 1y). Default: 5d")

    args = parser.parse_args()
    ticker = args.ticker.upper()

    # Basic date validation if both are provided
    if args.start and args.end:
        try:
            start_dt = datetime.strptime(args.start, "%Y-%m-%d")
            end_dt = datetime.strptime(args.end, "%Y-%m-%d")
            if start_dt > end_dt:
                print("Error: Start date must be before end date.")
                return 1
        except ValueError:
            print("Error: Dates must be in YYYY-MM-DD format.")
            return 1

    print(f"Fetching daily closing prices for ticker: {ticker}")
    if args.start and args.end:
        print(f"Range: {args.start} to {args.end}")
    else:
        print(f"Period: {args.period}")
    print(f"Request time: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}\n")

    closes = get_price_history(ticker, start=args.start, end=args.end, period=args.period)

    if closes.empty:
        print(f"No market data found for '{ticker}'.")
        print(
            "This usually means the ticker is not publicly listed or is "
            "misspelled. Double-check the symbol and try again."
        )
        return 1

    print(f"Ticker       : {ticker}")
    print(f"Trading days returned: {len(closes)}\n")
    print(f"{'Date':<12} {'Close'}")
    print("-" * 22)
    for date, row in closes.iterrows():
        print(f"{date.strftime('%Y-%m-%d'):<12} {row['Close']:.2f}")

    last_close = closes["Close"].iloc[-1]
    print(f"\nLast available closing price ({closes.index[-1].date()}): {last_close:.2f}")

    # Fetch option walls
    call_wall, put_wall = get_option_walls(ticker)
    if call_wall:
        print(f"Call Wall (Max OI): {call_wall:.2f}")
    if put_wall:
        print(f"Put Wall (Max OI): {put_wall:.2f}")

    plot_price_history(closes, ticker, call_wall=call_wall, put_wall=put_wall)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
