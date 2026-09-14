"""Atlanta Flights On-Time Performance (OTP) interactive dashboard.

Run:
    streamlit run dashboard.py

OTP definition: arrival delay < 15 minutes.
October 2025 OTP is interpolated from September and November (see data_processing).
"""

from __future__ import annotations

from pathlib import Path

import pandas as pd
import plotly.express as px
import plotly.graph_objects as go
import streamlit as st

from data_processing import (
    DEFAULT_DATA_PATH,
    DEFAULT_MIN_FLIGHTS,
    MONTH_NAMES,
    OTP_DELAY_THRESHOLD_MIN,
    filter_flights,
    load_and_prepare,
    otp_by_airline,
    otp_by_destination,
    otp_by_month,
    otp_by_session,
    rank_extremes,
)

st.set_page_config(
    page_title="ATL Flights OTP Dashboard",
    page_icon="✈️",
    layout="wide",
)


@st.cache_data(show_spinner="Loading flight data…")
def get_flights(data_path: str) -> pd.DataFrame:
    return load_and_prepare(data_path)


def _kpi_value(row: pd.Series | None, kind: str) -> tuple[str, str]:
    if row is None:
        return "N/A", "No entities meet the minimum flight threshold"
    label = str(row["display_label"])
    code = row.get("AIRLINE") or row.get("DESTINATION_AIRPORT") or ""
    title = f"{label}" + (f" ({code})" if code and code != label else "")
    delta = f"{row['otp_pct']:.1f}% OTP · {int(row['flights']):,} flights"
    return title, delta


def build_month_chart(monthly: pd.DataFrame) -> go.Figure:
    """Line chart with October visually flagged when interpolated."""
    fig = go.Figure()

    # Observed months as a solid line; Oct (if interpolated) as a distinct marker.
    is_interp = monthly["is_interpolated"].astype(bool)
    observed = monthly.loc[~is_interp]
    fig.add_trace(
        go.Scatter(
            x=observed["month_name"],
            y=observed["otp_pct"],
            mode="lines+markers",
            name="Observed OTP%",
            line=dict(width=2.5, color="#1f77b4"),
            marker=dict(size=8),
            customdata=observed[["on_time", "flights", "is_interpolated"]],
            hovertemplate=(
                "%{x}<br>OTP: %{y:.1f}%<br>"
                "On-time: %{customdata[0]:,}<br>"
                "Flights: %{customdata[1]:,}<extra></extra>"
            ),
        )
    )

    interp = monthly.loc[is_interp]
    if not interp.empty:
        # Dashed connector through Sep → Oct → Nov so the gap is visible.
        bridge = monthly.loc[monthly["month"].isin([9, 10, 11])].sort_values("month")
        fig.add_trace(
            go.Scatter(
                x=bridge["month_name"],
                y=bridge["otp_pct"],
                mode="lines",
                name="Oct interpolation bridge",
                line=dict(width=2, color="#d62728", dash="dash"),
                hoverinfo="skip",
                showlegend=True,
            )
        )
        fig.add_trace(
            go.Scatter(
                x=interp["month_name"],
                y=interp["otp_pct"],
                mode="markers",
                name="Oct (interpolated)",
                marker=dict(size=12, color="#d62728", symbol="diamond"),
                customdata=interp[["on_time", "flights"]],
                hovertemplate=(
                    "%{x}<br>OTP: %{y:.1f}% (interpolated)<br>"
                    "Method: (Sep + Nov) / 2<extra></extra>"
                ),
            )
        )

    # Force calendar order so Oct (added in a later trace) sits between Sep and Nov.
    month_order = [MONTH_NAMES[m] for m in range(1, 13)]
    fig.update_layout(
        title="OTP% by Month (2025)",
        xaxis_title="Month",
        xaxis=dict(categoryorder="array", categoryarray=month_order),
        yaxis_title="OTP %",
        yaxis=dict(range=[0, 100]),
        legend=dict(orientation="h", yanchor="bottom", y=1.02),
        margin=dict(t=60, b=40),
    )
    return fig


def build_session_chart(session_df: pd.DataFrame) -> go.Figure:
    fig = go.Figure()
    fig.add_trace(
        go.Bar(
            x=session_df["flight_session"].astype(str),
            y=session_df["otp_pct"],
            name="OTP%",
            marker_color="#2ca02c",
            customdata=session_df[["on_time", "flights"]],
            hovertemplate=(
                "%{x}<br>OTP: %{y:.1f}%<br>"
                "On-time: %{customdata[0]:,}<br>"
                "Flights: %{customdata[1]:,}<extra></extra>"
            ),
            yaxis="y",
        )
    )
    fig.add_trace(
        go.Scatter(
            x=session_df["flight_session"].astype(str),
            y=session_df["flights"],
            name="Flight volume",
            mode="lines+markers",
            marker=dict(size=8, color="#ff7f0e"),
            line=dict(width=2, color="#ff7f0e"),
            yaxis="y2",
            hovertemplate="%{x}<br>Flights: %{y:,}<extra></extra>",
        )
    )
    fig.update_layout(
        title="OTP% by Flight Session",
        xaxis_title="Session",
        yaxis=dict(title="OTP %", range=[0, 100]),
        yaxis2=dict(title="Flights", overlaying="y", side="right", showgrid=False),
        legend=dict(orientation="h", yanchor="bottom", y=1.02),
        margin=dict(t=60, b=40),
        barmode="group",
    )
    return fig


def build_map(dest_df: pd.DataFrame, min_flights: int) -> go.Figure:
    map_df = dest_df.dropna(subset=["LATITUDE", "LONGITUDE"]).copy()
    map_df = map_df[map_df["flights"] >= min_flights]
    if map_df.empty:
        fig = go.Figure()
        fig.update_layout(title="No destinations meet the current filters")
        return fig

    fig = px.scatter_geo(
        map_df,
        lat="LATITUDE",
        lon="LONGITUDE",
        color="otp_pct",
        size="flights",
        hover_name="airport_label",
        custom_data=["DESTINATION_AIRPORT", "otp_pct", "flights", "on_time"],
        color_continuous_scale="RdYlGn",
        range_color=[map_df["otp_pct"].min(), map_df["otp_pct"].max()],
        size_max=28,
        title="OTP% by Destination Airport",
    )
    fig.update_traces(
        hovertemplate=(
            "%{hovertext} (%{customdata[0]})<br>"
            "OTP: %{customdata[1]:.1f}%<br>"
            "On-time: %{customdata[3]:,}<br>"
            "Flights: %{customdata[2]:,}<extra></extra>"
        )
    )
    fig.update_geos(
        scope="north america",
        showland=True,
        landcolor="rgb(240, 240, 240)",
        showcountries=True,
        showlakes=True,
        fitbounds="locations",
    )
    fig.update_layout(
        coloraxis_colorbar=dict(title="OTP %"),
        margin=dict(t=60, b=10),
    )
    return fig


OTP_GOOD = 80.0
OTP_WARN = 70.0
OTP_COLOR_GOOD = "#1a7f37"
OTP_COLOR_WARN = "#b54708"
OTP_COLOR_BAD = "#cf222e"


def otp_text_color(value: float) -> str:
    """Text color for OTP%: green ≥80, yellow ≥70, red <70."""
    if pd.isna(value):
        return ""
    if value >= OTP_GOOD:
        return f"color: {OTP_COLOR_GOOD}; font-weight: 600"
    if value >= OTP_WARN:
        return f"color: {OTP_COLOR_WARN}; font-weight: 600"
    return f"color: {OTP_COLOR_BAD}; font-weight: 600"


def render_otp_color_legend() -> None:
    st.markdown(
        f"""
        <div style="display:flex; gap:1.25rem; flex-wrap:wrap; font-size:0.9rem; margin-bottom:0.35rem;">
          <span><span style="color:{OTP_COLOR_GOOD}; font-weight:600;">●</span> OTP ≥ {OTP_GOOD:.0f}%</span>
          <span><span style="color:{OTP_COLOR_WARN}; font-weight:600;">●</span> {OTP_WARN:.0f}% ≤ OTP &lt; {OTP_GOOD:.0f}%</span>
          <span><span style="color:{OTP_COLOR_BAD}; font-weight:600;">●</span> OTP &lt; {OTP_WARN:.0f}%</span>
        </div>
        """,
        unsafe_allow_html=True,
    )


def style_otp_table(df: pd.DataFrame):
    """Format detail table and color OTP % text by threshold."""
    return df.style.format({"OTP %": "{:.1f}"}).map(otp_text_color, subset=["OTP %"])


def main() -> None:
    st.title("Atlanta (ATL) Flights — On-Time Performance")
    st.caption(
        f"OTP = arrival delay < {OTP_DELAY_THRESHOLD_MIN} minutes · "
        "Source: Flights_from_Atlanta_2025.xlsx · Oct OTP interpolated when missing"
    )

    with st.sidebar:
        st.header("Filters")
        data_path = st.text_input("Data file path", value=str(DEFAULT_DATA_PATH))
        path = Path(data_path)
        if not path.exists():
            st.error(f"File not found: {path}")
            st.stop()

        flights = get_flights(str(path))

        airline_options = sorted(flights["AIRLINE"].dropna().unique().tolist())
        airline_labels = (
            flights[["AIRLINE", "airline_label"]]
            .drop_duplicates()
            .set_index("AIRLINE")["airline_label"]
            .to_dict()
        )
        selected_airlines = st.multiselect(
            "Airlines",
            options=airline_options,
            default=airline_options,
            format_func=lambda code: f"{airline_labels.get(code, code)} ({code})",
        )

        min_date = flights["flight_date"].min().date()
        max_date = flights["flight_date"].max().date()
        date_range = st.date_input(
            "Date range",
            value=(min_date, max_date),
            min_value=min_date,
            max_value=max_date,
        )

        min_flights = st.slider(
            "Min flights for KPI / map ranking",
            min_value=1,
            max_value=500,
            value=DEFAULT_MIN_FLIGHTS,
            step=1,
            help="Entities below this flight count are excluded from best/worst KPIs and the map.",
        )

        st.markdown("---")
        st.markdown(
            "**Sessions:** AM 05–11 · PM 12–16 · Night 17–20 · Red-Eye 21–04"
        )

    if isinstance(date_range, tuple) and len(date_range) == 2:
        start_date, end_date = date_range
    else:
        start_date, end_date = min_date, max_date

    filtered = filter_flights(
        flights,
        airlines=selected_airlines or None,
        date_start=pd.Timestamp(start_date),
        date_end=pd.Timestamp(end_date),
    )

    if filtered.empty:
        st.warning("No flights match the current filters.")
        st.stop()

    airline_otp = otp_by_airline(filtered)
    dest_otp = otp_by_destination(filtered)
    month_otp = otp_by_month(filtered)
    session_otp = otp_by_session(filtered)

    best_airline, worst_airline = rank_extremes(airline_otp, "airline_label", min_flights)
    best_dest, worst_dest = rank_extremes(dest_otp, "airport_label", min_flights)

    overall_otp = filtered["is_on_time"].mean() * 100.0
    st.metric("Overall OTP%", f"{overall_otp:.1f}%", help=f"{int(filtered['is_on_time'].sum()):,} / {len(filtered):,} flights")

    k1, k2, k3, k4 = st.columns(4)
    with k1:
        title, detail = _kpi_value(best_airline, "best")
        st.metric("Best airline", title, detail)
    with k2:
        title, detail = _kpi_value(worst_airline, "worst")
        st.metric("Worst airline", title, detail)
    with k3:
        title, detail = _kpi_value(best_dest, "best")
        st.metric("Best destination", title, detail)
    with k4:
        title, detail = _kpi_value(worst_dest, "worst")
        st.metric("Worst destination", title, detail)

    left, right = st.columns(2)
    with left:
        st.plotly_chart(build_month_chart(month_otp), use_container_width=True)
        if month_otp["is_interpolated"].any():
            st.caption(
                "October OTP is linearly interpolated: (Sep OTP + Nov OTP) / 2 "
                "because October is missing from the source file."
            )
    with right:
        st.plotly_chart(build_session_chart(session_otp), use_container_width=True)

    st.plotly_chart(build_map(dest_otp, min_flights), use_container_width=True)

    with st.expander("Airline OTP detail"):
        render_otp_color_legend()
        show = airline_otp.rename(
            columns={
                "AIRLINE": "Code",
                "airline_label": "Airline",
                "flights": "Flights",
                "on_time": "On-time",
                "otp_pct": "OTP %",
            }
        )
        st.dataframe(
            style_otp_table(show),
            use_container_width=True,
            hide_index=True,
        )

    with st.expander("Destination OTP detail"):
        render_otp_color_legend()
        show = dest_otp[
            [
                "DESTINATION_AIRPORT",
                "airport_label",
                "DEST_AIRPORT_CITY",
                "DEST_AIRPORT_STATE",
                "flights",
                "on_time",
                "otp_pct",
            ]
        ].rename(
            columns={
                "DESTINATION_AIRPORT": "Code",
                "airport_label": "Airport",
                "DEST_AIRPORT_CITY": "City",
                "DEST_AIRPORT_STATE": "State",
                "flights": "Flights",
                "on_time": "On-time",
                "otp_pct": "OTP %",
            }
        )
        st.dataframe(
            style_otp_table(show),
            use_container_width=True,
            hide_index=True,
        )


if __name__ == "__main__":
    main()
