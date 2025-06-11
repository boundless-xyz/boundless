import os
import argparse
import csv
from web3 import Web3
import matplotlib.pyplot as plt
from datetime import datetime


def fetch_balances(rpc_url, address, duration, interval):
    web3 = Web3(Web3.HTTPProvider(rpc_url))
    address = web3.to_checksum_address(address)
    latest_block_number = web3.eth.block_number
    latest_block = web3.eth.get_block(latest_block_number)
    latest_block_timestamp = latest_block.timestamp

    chain_id = web3.eth.chain_id

    past_block_number = latest_block_number - 1000
    past_block = web3.eth.get_block(past_block_number)
    avg_block_time = (latest_block_timestamp - past_block.timestamp) / 1000

    balances = []

    for minutes_ago in range(duration, -1, -interval):
        target_timestamp = latest_block_timestamp - (minutes_ago * 60)
        block_offset = int((latest_block_timestamp - target_timestamp) / avg_block_time)
        estimated_block_number = max(latest_block_number - block_offset, 0)
        block = web3.eth.get_block(estimated_block_number)
        balance_wei = web3.eth.get_balance(address, block_identifier=block.number)
        balance_eth = web3.from_wei(balance_wei, 'ether')
        balances.append((block.number, block.timestamp, float(balance_eth)))
        print(f"Fetched balance at block {block.number} (timestamp {block.timestamp}): {balance_eth} ETH")

    return balances, chain_id


def write_csv(balances, address, chain_id):
    os.makedirs('out', exist_ok=True)
    filename = f"out/balance_{address}_{chain_id}.csv"
    with open(filename, 'w', newline='') as csvfile:
        writer = csv.writer(csvfile)
        writer.writerow(['block_number', 'timestamp', 'balance_eth'])
        writer.writerows(balances)

    print(f"Balances written to {filename}")


def plot_balances(balances, address, chain_id):
    timestamps = [datetime.fromtimestamp(ts) for _, ts, _ in balances]
    balances_eth = [balance for _, _, balance in balances]

    plt.figure(figsize=(10, 5))
    plt.plot(timestamps, balances_eth, marker='o', linestyle='-')
    plt.xlabel('Time')
    plt.ylabel('Balance (ETH)')
    plt.title(f'Balance over Time for Address {address} (Chain ID: {chain_id})')
    plt.grid(True)

    plt.ticklabel_format(axis='y', style='plain', useOffset=False)
    plt.gca().get_yaxis().get_major_formatter().set_scientific(False)

    plt.tight_layout()
    plt.show()


def plot_from(filepath):
    balances = []
    with open(filepath, 'r') as csvfile:
        reader = csv.reader(csvfile)
        next(reader)
        for row in reader:
            block_number, timestamp, balance_eth = int(row[0]), int(row[1]), float(row[2])
            balances.append((block_number, timestamp, balance_eth))

    parts = os.path.basename(filepath).split('_')
    address, chain_id = parts[1], parts[2].split('.')[0]
    plot_balances(balances, address, chain_id)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description="Ethereum address balance history over time.")
    parser.add_argument('address', nargs='?', help="Ethereum address to query.")
    parser.add_argument('duration', type=int, nargs='?', help="Duration in minutes to fetch balances for.")
    parser.add_argument('--interval', type=int, default=10, help="Interval in minutes between balance checks.")
    parser.add_argument('--plot', help="Plot an existing CSV file.")

    args = parser.parse_args()

    if args.plot:
        plot_from(args.plot)
    else:
        if not args.address or not args.duration:
            print("Address and duration are required if not plotting from CSV.")
            exit(1)

        rpc_url = os.getenv('RPC_URL')
        if not rpc_url:
            print("RPC_URL environment variable is required.")
            exit(1)

        balances, chain_id = fetch_balances(rpc_url, args.address, args.duration, args.interval)
        write_csv(balances, args.address, chain_id)
        plot_balances(balances, args.address, chain_id)
