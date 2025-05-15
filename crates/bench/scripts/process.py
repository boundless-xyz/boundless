import argparse
import pandas as pd
import numpy as np
import matplotlib.pyplot as plt
import sys
from pathlib import Path

def load_data(path: Path) -> pd.DataFrame:
    """Load benchmark data from CSV or JSON file."""
    suffix = path.suffix.lower()
    if suffix == ".csv":
        return pd.read_csv(path)
    elif suffix == ".json":
        return pd.read_json(path)
    else:
        raise ValueError(f"Unsupported file extension: {suffix}. Use .csv or .json")

def choose_unit_and_scale(values: np.ndarray):
    """
    Choose a human-friendly unit (Hz, kHz, MHz) based on the max value,
    and return (scale_factor, unit_label).
    """
    max_val = np.nanmax(values)
    if max_val >= 1e6:
        return 1e6, "MHz"
    elif max_val >= 1e3:
        return 1e3, "kHz"
    else:
        return 1.0, "Hz"

def analyze_latency(df: pd.DataFrame):
    """Perform latency and frequency analysis with dynamic unit scaling."""
    # Ensure required columns exist
    for col in ("latency", "cycle_count", "bid_start", "fulfilled_at"):
        if col not in df.columns:
            raise KeyError(f"Input data must contain '{col}' column")

    # Calculate frequency in Hz (cycles per second). Latency is in seconds.
    df['hz'] = df['cycle_count'] / df['latency']

    # Determine scale and unit
    scale, unit = choose_unit_and_scale(df['hz'].values)
    df['freq_scaled'] = df['hz'] / scale

    # Compute total time span
    first_start = df['bid_start'].min()
    last_fulfilled = df['fulfilled_at'].dropna().max()
    start_time = pd.to_datetime(first_start, unit="s")
    end_time = pd.to_datetime(last_fulfilled, unit="s")
    duration = end_time - start_time

    # Summary
    print("\nTime Span:")
    print(f"  First request at:   {start_time} (epoch {first_start})")
    print(f"  Last fulfillment at: {end_time} (epoch {last_fulfilled})")
    print(f"  Total duration:      {duration}\n")

    print("Latency Statistics (s):")
    print(df['latency'].describe(), "\n")
    print(f"Frequency Statistics ({unit}):")
    print(df['freq_scaled'].describe(), "\n")

    # 1) Latency histogram
    plt.figure()
    plt.hist(df['latency'].dropna(), bins=20)
    plt.xlabel("Latency (s)")
    plt.ylabel("Count")
    plt.title("Latency Distribution")
    plt.tight_layout()
    plt.show()

    # 2) Latency CDF
    sorted_lat = np.sort(df['latency'].dropna())
    cdf_lat = np.arange(1, len(sorted_lat) + 1) / len(sorted_lat)
    plt.figure()
    plt.plot(sorted_lat, cdf_lat, marker=".", linestyle="none")
    plt.xlabel("Latency (s)")
    plt.ylabel("Empirical CDF")
    plt.title("Latency CDF")
    plt.tight_layout()
    plt.show()

    # 3) Latency vs. time scatter
    times = pd.to_datetime(df['bid_start'], unit="s")
    plt.figure()
    plt.scatter(times, df['latency'])
    plt.xlabel("Request Time")
    plt.ylabel("Latency (s)")
    plt.title("Latency vs. Request Time")
    plt.tight_layout()
    plt.show()

    # 4) Frequency histogram
    plt.figure()
    plt.hist(df['freq_scaled'].dropna(), bins=20)
    plt.xlabel(f"Frequency ({unit})")
    plt.ylabel("Count")
    plt.title("Frequency Distribution")
    plt.tight_layout()
    plt.show()

    # 5) Frequency vs. time scatter
    plt.figure()
    plt.scatter(times, df['freq_scaled'])
    plt.xlabel("Request Time")
    plt.ylabel(f"Frequency ({unit})")
    plt.title("Frequency vs. Request Time")
    plt.tight_layout()
    plt.show()

def main():
    parser = argparse.ArgumentParser(description="Latency & Frequency analysis (CSV/JSON).")
    parser.add_argument("file", type=Path, help="Path to .csv or .json benchmark file")
    args = parser.parse_args()

    try:
        df = load_data(args.file)
    except Exception as e:
        print(f"Error loading data: {e}", file=sys.stderr)
        sys.exit(1)

    try:
        analyze_latency(df)
    except Exception as e:
        print(f"Error during analysis: {e}", file=sys.stderr)
        sys.exit(1)

if __name__ == "__main__":
    main()

