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
    required_cols = ("effective_latency", "e2e_latency", "cycle_count",
                     "bid_start", "locked_at", "fulfilled_at", "prover")
    for col in required_cols:
        if col not in df.columns:
            raise KeyError(f"Input data must contain '{col}' column")

    # Compute fulfillment counts
    total_requests = len(df)
    locked_mask = df['locked_at'].notna()
    locked = int(locked_mask.sum())
    fulfilled_mask = df['fulfilled_at'].notna()
    fulfilled = int(fulfilled_mask.sum())
    unfulfilled = total_requests - fulfilled

    # Top provers (exclude missing)
    top_provers = df['prover'].dropna().value_counts().head(10)

    # Calculate frequency in Hz (cycles per second). Latency is in seconds.
    df['hz'] = df['cycle_count'] / df['effective_latency']
    scale, unit = choose_unit_and_scale(df['hz'].values)
    df['freq_scaled'] = df['hz'] / scale

    # Compute total time span
    first_start = df['bid_start'].dropna().min()
    last_fulfilled = df.loc[fulfilled_mask, 'fulfilled_at'].max()
    start_time = pd.to_datetime(first_start, unit="s")
    end_time = pd.to_datetime(last_fulfilled, unit="s") if not np.isnan(last_fulfilled) else None
    duration = (end_time - start_time) if end_time is not None else None

    # Summary
    print("\n=== Summary ===")
    print(f"Total requests:    {total_requests}")
    print(f"Locked:            {locked}")
    print(f"Fulfilled:         {fulfilled}")
    print(f"Unfulfilled:       {unfulfilled}")
    print("\nTop Provers:")
    print(top_provers.to_string(), "\n")

    print("Time Span:")
    print(f"  First request at:   {start_time} (ts {first_start})")
    if end_time is not None:
        print(f"  Last fulfillment at: {end_time} (ts {int(last_fulfilled)})")
        print(f"  Total duration:      {duration}\n")
    else:
        print("  No fulfilled requests to report time span.\n")

    print("Effective Latency Statistics (s):")
    print(df['effective_latency'].describe(), "\n")
    print("E2E Latency Statistics (s):")
    print(df['e2e_latency'].describe(), "\n")
    print(f"Frequency Statistics ({unit}):")
    print(df['freq_scaled'].describe(), "\n")

    # 1) Effective latency histogram
    plt.figure()
    plt.hist(df['effective_latency'].dropna(), bins=20)
    plt.xlabel("Effective Latency (s)")
    plt.ylabel("Count")
    plt.title("Effective Latency Distribution")
    plt.tight_layout()
    plt.show()

    # 2) E2E latency histogram
    plt.figure()
    plt.hist(df['e2e_latency'].dropna(), bins=20)
    plt.xlabel("E2E Latency (s)")
    plt.ylabel("Count")
    plt.title("E2E Latency Distribution")
    plt.tight_layout()
    plt.show()

    # 3) Effective Latency CDF
    sorted_eff = np.sort(df['effective_latency'].dropna())
    cdf_eff = np.arange(1, len(sorted_eff) + 1) / len(sorted_eff)
    plt.figure()
    plt.plot(sorted_eff, cdf_eff, marker=".", linestyle="none")
    plt.xlabel("Effective Latency (s)")
    plt.ylabel("Empirical CDF")
    plt.title("Effective Latency CDF")
    plt.tight_layout()
    plt.show()

    # 4) E2E Latency CDF
    sorted_e2e = np.sort(df['e2e_latency'].dropna())
    cdf_e2e = np.arange(1, len(sorted_e2e) + 1) / len(sorted_e2e)
    plt.figure()
    plt.plot(sorted_e2e, cdf_e2e, marker=".", linestyle="none")
    plt.xlabel("E2E Latency (s)")
    plt.ylabel("Empirical CDF")
    plt.title("E2E Latency CDF")
    plt.tight_layout()
    plt.show()

    # 5) Effective Latency vs. Lock Time scatter
    plt.figure()
    lock_times = pd.to_datetime(df['locked_at'].dropna(), unit="s")
    plt.scatter(lock_times, df.loc[df['locked_at'].notna(), 'effective_latency'])
    plt.xlabel("Lock Time")
    plt.ylabel("Effective Latency (s)")
    plt.title("Effective Latency vs. Lock Time")
    plt.tight_layout()
    plt.show()

    # 6) Latency vs. Request Time scatter
    plt.figure()
    req_times = pd.to_datetime(df['bid_start'], unit="s")
    plt.scatter(req_times, df['e2e_latency'])
    plt.xlabel("Request Time")
    plt.ylabel("E2E Latency (s)")
    plt.title("E2E Latency vs. Request Time")
    plt.tight_layout()
    plt.show()

    # 7) Frequency histogram
    plt.figure()
    plt.hist(df['freq_scaled'].dropna(), bins=20)
    plt.xlabel(f"Frequency ({unit})")
    plt.ylabel("Count")
    plt.title("Frequency Distribution")
    plt.tight_layout()
    plt.show()

    # 8) Frequency vs. Lock Time scatter
    plt.figure()
    plt.scatter(lock_times, df.loc[df['locked_at'].notna(), 'freq_scaled'])
    plt.xlabel("Lock Time")
    plt.ylabel(f"Frequency ({unit})")
    plt.title("Frequency vs. Lock Time")
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
