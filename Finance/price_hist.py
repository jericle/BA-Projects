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

import matplotlib.ticker as mtick
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
    """Create a professional line plot of closing prices and save it to a file."""
    fig, ax = plt.subplots(figsize=(14, 7))

    # --- colour palette (dark-dashboard style) ---
    BG_COLOUR = "#1a1e24"
    GRID_COLOUR = "#585858"
    TICK_COLOR = "#9e9e9e"
    TEXT_COLOR = "#e0e0e0"
    LINE_COLOUR = "#4fc3f7"
    FILL_ALPHA = 0.08
    CALL_WALL_COLOR = "#66bb6a"
    PUT_WALL_COLOR = "#ef5350"

    fig.patch.set_facecolor(BG_COLOUR)
    ax.set_facecolor(BG_COLOUR)

    # --- price line with gradient fill ---
    ax.plot(
        closes.index,
        closes["Close"],
        color=LINE_COLOUR,
        linewidth=2,
        label="Closing Price",
        zorder=5,
    )
    ax.fill_between(
        closes.index,
        closes["Close"],
        alpha=FILL_ALPHA,
        color=LINE_COLOUR,
        zorder=3,
    )

    # --- optional moving averages ---
    windows = [20, 50]
    ma_colors = ["#ffa726", "#ab47bc"]
    for window, ma_color in zip(windows, ma_colors):
        if len(closes) >= window:
            ma = closes["Close"].rolling(window).mean().dropna()
            if not ma.empty:
                ax.plot(
                    ma.index,
                    ma.values,
                    color=ma_color,
                    linewidth=1.2,
                    alpha=0.85,
                    label=f"MA-{window}",
                    zorder=4,
                )

    # --- option walls (shaded region + dashed lines) ---
    wall_lines_drawn = []

    if call_wall is not None:
        ax.axhline(
            y=call_wall, color=CALL_WALL_COLOR, linestyle="--", lw=1.5, label=f"Call Wall ({call_wall:.2f})"
        )
        wall_lines_drawn.append(call_wall)

    if put_wall is not None:
        ax.axhline(
            y=put_wall, color=PUT_WALL_COLOR, linestyle="--", lw=1.5, label=f"Put Wall ({put_wall:.2f})"
        )
        wall_lines_drawn.append(put_wall)

    # shade the region between put/call walls (when both exist)
    if call_wall is not None and put_wall is not None:
        lo, hi = sorted([put_wall, call_wall])
        ax.axhspan(
            lo, hi, alpha=0.12, color="gold", zorder=0,
            label=f"Put-Call Zone (${lo:.2f}–${hi:.2f})",
        )

    # --- aesthetics ---
    ax.set_title(
        f"{ticker}  —  Price History",
        fontsize=16,
        fontweight="semibold",
        color=TEXT_COLOR,
        pad=18,
    )
    ax.set_xlabel("Date", fontsize=12, color=TEXT_COLOR)
    ax.set_ylabel("Price (USD)", fontsize=12, color=TEXT_COLOR)

    # tick formatting
    ax.tick_params(axis="x", colors=TICK_COLOR, rotation=35)
    ax.tick_params(axis="y", colors=TICK_COLOR)

    ax.yaxis.set_major_formatter(mtick.FuncFormatter(lambda v, _: f"${v:,.2f}"))

    # grid
    ax.grid(True, color=GRID_COLOUR, alpha=0.35, linestyle="-", linewidth=0.5)
    ax.set_axisbelow(True)

    # legend (top-left, no frame)
    ax.legend(loc="upper left", frameon=False, fontsize=10, handlelength=2)

    # last-price annotation
    last_date = closes.index[-1]
    last_price = closes["Close"].iloc[-1]
    ax.annotate(
        f"${last_price:,.2f}",
        xy=(last_date, last_price),
        xytext=(5, 10),
        fontsize=10,
        color=LINE_COLOUR,
        fontweight="semibold",
        arrowprops=dict(arrowstyle="->", color=LINE_COLOUR, lw=1.2),
    )

    fig.tight_layout()

    plt.savefig(output_file, dpi=150, facecolor=fig.get_facecolor())
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
