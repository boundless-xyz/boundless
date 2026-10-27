#!/usr/bin/env python3
"""Rehearse upgrades using existing scripts and ephemeral localhost Anvil forks."""
import argparse, concurrent.futures, datetime, hashlib, importlib.util, json, os, socket, subprocess, time, urllib.request
from pathlib import Path
from urllib.parse import urlparse

if not __debug__:
    raise SystemExit("Run without Python optimization; validation assertions are required")
ROOT = Path(__file__).resolve().parents[2]
MARKET = ROOT
ZKC = ROOT.parent / "zkc"
MANIFEST = Path(__file__).with_name("baselines.json")
ENVIRONMENT = "rehearsal"
spec = importlib.util.spec_from_file_location("safe_batch", MARKET / "contracts/scripts/write_safe_batch.py")
writer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(writer)
SLOT = "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
ZERO = "0x" + "00" * 20
SAFE = "0xb04d1a222789a76e74168a919b43b20f66e24f0b"
TAIKO_SAFE = "0x8e537aaeddbc98a9a095397adb7c0d738458cc8b"
MULTISEND = "0x40A2aCCbd92BCA938b02010E17A5b8929b49130D"
SENDER = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
READS = {
    "eth_chainId",
    "eth_getBlockByNumber",
    "eth_getStorageAt",
    "eth_getCode",
    "eth_call",
}
CHAINS = {
    "base": (84532, "https://sepolia.base.org", ["base"]),
    "taiko": (167000, "https://rpc.taiko.xyz", ["taiko"]),
    "ethereum": (
        11155111,
        "https://ethereum-sepolia-rpc.publicnode.com",
        ["accounting", "mint", "zkc"],
    ),
}

PRODUCTION_CHAINS = {
    "base": (8453, "https://mainnet.base.org", ["base"]),
    "taiko": (167000, "https://rpc.taiko.xyz", ["taiko"]),
    "ethereum": (1, "https://eth.drpc.org", ["accounting", "mint", "zkc"]),
}
FOUNDATION = "0x3886eeaf95aa2bdddf0c924925e290291f70447f"


def configure(environment, zkc_repo):
    global ENVIRONMENT, CHAINS, ZKC
    ENVIRONMENT = "rehearsal" if environment == "staging" else "production"
    CHAINS = dict(PRODUCTION_CHAINS if ENVIRONMENT == "production" else STAGING_CHAINS)
    ZKC = Path(zkc_repo).resolve()
    for label, (chain_id, url, keys) in list(CHAINS.items()):
        variable = {
            "base": ("BASE_RPC_URL" if ENVIRONMENT == "production" else "BASE_SEPOLIA_RPC_URL"),
            "taiko": "TAIKO_RPC_URL",
            "ethereum": ("ETHEREUM_RPC_URL" if ENVIRONMENT == "production" else "ETHEREUM_SEPOLIA_RPC_URL"),
        }[label]
        CHAINS[label] = (chain_id, os.environ.get(variable, url), keys)


def repo_for(key):
    return ZKC if key == "zkc" else MARKET


def artifact_for(manifest, key):
    entry = manifest["artifacts"][key]
    path = repo_for(key) / entry["path"]
    artifact = json.loads(path.read_text())
    assert (
        keccak(artifact["bytecode"]["object"]) == entry["creation_bytecode_keccak256"]
    ), "artifact creation bytecode changed"
    assert (
        keccak(artifact["deployedBytecode"]["object"]) == entry["runtime_template_keccak256"]
    ), "artifact runtime template changed"
    return path, artifact


STAGING_CHAINS = dict(CHAINS)


def cmd(*args):
    return subprocess.check_output(args, text=True).strip()


def encode(signature, *args):
    return cmd("cast", "calldata", signature, *map(str, args))


def keccak(data):
    return subprocess.check_output(["cast", "keccak"], input=data.encode()).decode().strip()


class RPC:
    def __init__(self, url, local=False):
        p = urlparse(url)
        if local:
            assert p.scheme == "http" and p.hostname == "127.0.0.1" and p.port, "local writes require loopback"
        else:
            assert p.scheme == "https", "upstream is read-only HTTPS"
        self.url = url
        self.local = local

    def call(self, method, params):
        assert self.local or method in READS, "upstream write forbidden"
        payload = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
        for attempt in range(4):
            try:
                request = urllib.request.Request(
                    self.url,
                    data=payload,
                    headers={
                        "Content-Type": "application/json",
                        "User-Agent": "Boundless-local-staging-rehearsal",
                    },
                )
                with urllib.request.urlopen(request, timeout=40) as response:
                    result = json.load(response)
                if "error" in result:
                    raise RuntimeError(json.dumps(result["error"]))
                return result["result"]
            except Exception:
                if attempt == 3:
                    raise
                time.sleep(attempt + 1)

    def view(self, target, signature, *args, block="latest"):
        return self.call("eth_call", [{"to": target, "data": encode(signature, *args)}, block])

    def uint(self, target, signature, *args, block="latest"):
        return int(self.view(target, signature, *args, block=block), 16)

    def address(self, target, signature, *args):
        return "0x" + self.view(target, signature, *args)[-40:]

    def send(self, to, data, sender=SENDER):
        assert self.local and self.call("web3_clientVersion", []).lower().startswith("anvil"), "writes require Anvil"
        tx = {"from": sender, "data": data}
        if to:
            tx["to"] = to
        estimate = int(self.call("eth_estimateGas", [tx]), 16)
        tx["gas"] = hex((estimate * 6 + 4) // 5)
        txhash = self.call("eth_sendTransaction", [tx])
        for _ in range(60):
            receipt = self.call("eth_getTransactionReceipt", [txhash])
            if receipt:
                assert int(receipt["status"], 16) == 1, (
                    "local transaction failed",
                    txhash,
                )
                return receipt
            time.sleep(0.1)
        raise TimeoutError(txhash)


def abi_addresses(raw):
    data = bytes.fromhex(raw[2:])
    offset = int.from_bytes(data[:32], "big")
    n = int.from_bytes(data[offset : offset + 32], "big")
    return ["0x" + data[offset + 32 + i * 32 + 12 : offset + 64 + i * 32].hex() for i in range(n)]


def verify_runtime(rpc, target, artifact, candidate):
    expected = bytearray.fromhex(artifact["deployedBytecode"]["object"][2:])
    refs = artifact["deployedBytecode"].get("immutableReferences", {})
    names = {}

    def visit(v):
        if isinstance(v, dict):
            if str(v.get("id")) in refs and "name" in v:
                names[str(v["id"])] = v["name"]
            for child in v.values():
                visit(child)
        elif isinstance(v, list):
            for child in v:
                visit(child)

    visit(artifact.get("ast", {}))
    unknown = set(refs) - set(names)
    assert len(unknown) <= 1, "unresolved immutable names"
    for identifier, locations in refs.items():
        name = names.get(identifier, "__self")
        value = ("0x" + candidate[2:].rjust(64, "0")) if name == "__self" else rpc.view(target, name + "()")
        for ref in locations:
            expected[ref["start"] : ref["start"] + ref["length"]] = bytes.fromhex(value[2:])
    actual = rpc.call("eth_getCode", [candidate, "latest"])
    assert actual.lower() == "0x" + expected.hex(), "candidate runtime/dependency mismatch"
    return keccak(actual)


def export_transaction(rpc, label, chain_id, steps, path, block, safe=None, nonce=None, name=None):
    safe = safe or (FOUNDATION if chain_id == 1 else TAIKO_SAFE if label == "taiko" else SAFE)
    owners = abi_addresses(rpc.view(safe, "getOwners()"))
    threshold = rpc.uint(safe, "getThreshold()")
    assert threshold == 3 and len(owners) == (
        5 if chain_id == 11155111 else 6
    ), "unexpected Safe threshold or owner count"
    calls = [encode("upgradeToAndCall(address,bytes)", s["implementation"], "0x") for s in steps]
    builder = writer.build_batch(
        chain_id,
        safe,
        [s["proxy"] for s in steps],
        calls,
        [s["contract"] for s in steps],
        name or label + " upgrades",
    )
    path.with_name("safe-builder.json").write_text(json.dumps(builder, indent=2) + "\n")
    assert [(v["to"], v["data"]) for v in builder["transactions"]] == [(s["proxy"], c) for s, c in zip(steps, calls)]
    if len(steps) == 1:
        target = steps[0]["proxy"]
        data = calls[0]
        operation = 0
    else:
        assert (
            keccak(rpc.call("eth_getCode", [MULTISEND, "latest"]))
            == "0xa9865ac2d9c7a1591619b188c4d88167b50df6cc0c5327fcbd1c8c75f7c066ad"
        )
        packed = "0x" + "".join(
            "00"
            + v["to"][2:]
            + int(v["value"]).to_bytes(32, "big").hex()
            + ((len(v["data"]) - 2) // 2).to_bytes(32, "big").hex()
            + v["data"][2:]
            for v in builder["transactions"]
        )
        target = MULTISEND
        data = encode("multiSend(bytes)", packed)
        operation = 1
    if nonce is None:
        nonce = rpc.uint(safe, "nonce()")
    hash_args = [target, 0, data, operation, 0, 0, 0, ZERO, ZERO, nonce]
    txhash = rpc.view(
        safe,
        "getTransactionHash(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,uint256)",
        *hash_args
    )
    transaction = {
        "scope": "LOCAL FORK FIXTURE ONLY. Candidate addresses are not deployed on the live chain. Regenerate after actual staging deployment; do not submit this file to a live Safe.",
        "chainId": chain_id,
        "blockNumber": block,
        "safe": safe,
        "to": target,
        "value": 0,
        "data": data,
        "operation": operation,
        "safeTxGas": 0,
        "baseGas": 0,
        "gasPrice": 0,
        "gasToken": ZERO,
        "refundReceiver": ZERO,
        "nonce": nonce,
        "safeTxHash": txhash,
        "threshold": threshold,
        "owners": owners,
        "steps": steps,
        "live_submission_ready": False,
    }
    path.write_text(json.dumps(transaction, indent=2) + "\n")
    return transaction


def execute_exact(rpc, transaction):
    safe = transaction["safe"]
    owners = sorted(transaction["owners"], key=lambda a: int(a, 16))[: transaction["threshold"]]
    assert rpc.uint(safe, "nonce()") == transaction["nonce"]
    for owner in owners:
        rpc.call("anvil_impersonateAccount", [owner])
        rpc.call("anvil_setBalance", [owner, hex(10**20)])
        rpc.send(safe, encode("approveHash(bytes32)", transaction["safeTxHash"]), owner)
    signatures = "0x" + "".join(o[2:].rjust(64, "0") + "00" * 32 + "01" for o in owners)
    receipt = rpc.send(
        safe,
        encode(
            "execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)",
            transaction["to"],
            0,
            transaction["data"],
            transaction["operation"],
            0,
            0,
            0,
            ZERO,
            ZERO,
            signatures,
        ),
    )
    assert rpc.uint(safe, "nonce()") == transaction["nonce"] + 1
    for step in transaction["steps"]:
        assert (
            rpc.call("eth_getStorageAt", [step["proxy"], SLOT, "latest"])[-40:].lower()
            == step["implementation"][2:].lower()
        )
    return {
        "transaction_hash": receipt["transactionHash"],
        "gas_used": int(receipt["gasUsed"], 16),
        "real_threshold": transaction["threshold"],
        "owners_impersonated_locally": owners,
        "implementation_slots_verified": len(transaction["steps"]),
    }


def deploy_with_repo_script(rpc, label, key, proxy, folder, chain_key=None):
    assert rpc.local and urlparse(rpc.url).hostname == "127.0.0.1", "deployment requires loopback Anvil"
    assert rpc.call("web3_clientVersion", []).lower().startswith("anvil"), "deployment requires Anvil"
    repo = repo_for(key)
    config = repo / ("deployment.toml" if key == "zkc" else "contracts/deployment.toml")
    before = config.read_bytes()
    profile = "shanghai" if key == "taiko" else "povw-deploy" if key in ("accounting", "mint") else "default"
    script = {
        "base": "contracts/scripts/Manage.s.sol:UpgradeBoundlessMarket",
        "taiko": "contracts/scripts/Manage.s.sol:UpgradeBoundlessMarket",
        "accounting": "contracts/scripts/Manage.PoVW.s.sol:UpgradePoVWAccounting",
        "mint": "contracts/scripts/Manage.PoVW.s.sol:UpgradePoVWMint",
        "zkc": "script/Upgrade.s.sol:UpgradeZKC",
    }[key]
    chain_key = (
        chain_key
        or (
            {
                "base": "base-mainnet",
                "taiko": "taiko-mainnet",
                "ethereum": "ethereum-mainnet",
            }
            if ENVIRONMENT == "production"
            else {
                "base": "base-sepolia-staging",
                "taiko": "taiko-staging",
                "ethereum": "ethereum-sepolia-staging",
            }
        )[label]
    )
    env = os.environ.copy()
    env.update(
        CHAIN_KEY=chain_key,
        STACK_TAG="",
        GNOSIS_EXECUTE="true",
        SKIP_SAFETY_CHECKS="false",
        FOUNDRY_FFI="true",
        FOUNDRY_OUT="out/script-rehearsal-" + key,
        DEPLOYER_PRIVATE_KEY="0",
        DEPLOYER_ADDRESS=SENDER,
        FOUNDRY_PROFILE=profile,
        FOUNDRY_BUILD_INFO_PATH="out/script-rehearsal-" + key + "/build-info",
        FOUNDRY_BROADCAST="out/script-rehearsal-" + key + "/broadcast",
        FOUNDRY_SOLC="0.8.26",
        FOUNDRY_EVM_VERSION="shanghai" if key == "taiko" else "cancun",
        FORGE_SNAPSHOT_EMIT="false",
    )
    if key in ("base", "taiko"):
        env["BOUNDLESS_ROUTER"] = rpc.address(proxy, "ROUTER()")
        env["BOUNDLESS_LEGACY_IMPL"] = rpc.address(proxy, "LEGACY_IMPL()")
    command = [
        "forge",
        "script",
        script,
        "--skip",
        "test",
        "--offline",
        "--rpc-url",
        rpc.url,
        "--broadcast",
        "--unlocked",
        "--sender",
        SENDER,
        "--skip-simulation",
        "--force",
        "--out",
        "out/script-rehearsal-" + key,
        "--cache-path",
        "cache/script-rehearsal-" + key,
        "--json",
    ]
    completed = subprocess.run(command, cwd=repo, env=env, capture_output=True, text=True, timeout=600)
    (folder / (key + "-deployment.log")).write_text(completed.stdout + completed.stderr)
    assert config.read_bytes() == before, "Safe preparation changed deployment.toml"
    assert completed.returncode == 0, key + ": existing upgrade script failed; see deployment log"
    decoded = None
    for i, ch in enumerate(completed.stdout):
        if ch != "{":
            continue
        try:
            value, _ = json.JSONDecoder().raw_decode(completed.stdout[i:])
            if isinstance(value, dict) and "returns" in value:
                decoded = value
                break
        except json.JSONDecodeError:
            pass
    assert decoded is not None, "missing forge script return values"
    returns = decoded["returns"]
    candidate = returns.get("newImpl", returns.get("newImplementation"))["value"]
    assert rpc.call("eth_getCode", [candidate, "latest"]) != "0x", "script did not deploy the implementation locally"
    return candidate


def run_tests(command, repo, env, folder, name):
    with (folder / (name + ".log")).open("w") as log:
        completed = subprocess.run(
            command,
            cwd=repo,
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
            timeout=900,
        )
    assert completed.returncode == 0, name + ": tests failed; see " + str(folder / (name + ".log"))


def run_one(label, output, manifest):
    chain_id, url, keys = CHAINS[label]
    upstream = RPC(url)
    assert int(upstream.call("eth_chainId", []), 16) == chain_id, "wrong upstream chain"
    targets = [
        next(v for v in manifest["targets"] if v["environment"] == ENVIRONMENT and v["contract"] == key) for key in keys
    ]
    number = targets[0]["baseline_hash_block"]
    assert all(t["baseline_hash_block"] == number and t["chain_id"] == chain_id for t in targets)
    pinned = upstream.call("eth_getBlockByNumber", [hex(number), False])
    assert all(t["baseline_hash_block_hash"] == pinned["hash"] for t in targets), "snapshot hash changed"
    assert int(pinned["timestamp"], 16) < manifest["cutoff_timestamp"], "rehearsal requires a pre-cutoff snapshot"
    print(label + ": pinned block " + str(number), flush=True)
    folder = output / label
    folder.mkdir(parents=True, exist_ok=True)
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    log = (folder / "anvil.log").open("w")
    hardfork = {"base": "beryl", "taiko": "shanghai", "ethereum": "amsterdam"}[label]
    process = subprocess.Popen(
        [
            "anvil",
            "--network",
            "base" if label == "base" else "ethereum",
            "--hardfork",
            hardfork,
            "--silent",
            "--host",
            "127.0.0.1",
            "--port",
            str(port),
            "--fork-url",
            url,
            "--fork-block-number",
            str(number),
            "--chain-id",
            str(chain_id),
            "--gas-limit",
            "40000000",
        ],
        stdout=log,
        stderr=subprocess.STDOUT,
    )
    rpc = RPC("http://127.0.0.1:" + str(port), local=True)
    try:
        for _ in range(80):
            if process.poll() is not None:
                raise RuntimeError(label + ": Anvil stopped during startup; see local log")
            try:
                if rpc.call("web3_clientVersion", []).lower().startswith("anvil"):
                    break
            except Exception:
                time.sleep(0.25)
        else:
            raise RuntimeError("Anvil startup timeout")
        assert int(rpc.call("eth_chainId", []), 16) == chain_id
        assert rpc.call("eth_getBlockByNumber", [hex(number), False])["hash"] == pinned["hash"]
        # Keep newly mined blocks on the historical timeline, including after the cutoff.
        rpc.call("anvil_setBlockTimestampInterval", [1])
        steps = []
        safe = FOUNDATION if chain_id == 1 else TAIKO_SAFE if label == "taiko" else SAFE
        for entry in targets:
            key = entry["contract"]
            assert (
                rpc.call("eth_getStorageAt", [entry["proxy"], SLOT, "latest"])[-40:].lower()
                == entry["baseline_implementation"][2:].lower()
            ), "baseline changed"
            assert (
                keccak(rpc.call("eth_getCode", [entry["baseline_implementation"], "latest"]))
                == entry["baseline_code_hash"]
            ), "baseline runtime changed"
            candidate = deploy_with_repo_script(rpc, label, key, entry["proxy"], folder)
            artifact_path, artifact = artifact_for(manifest, key)
            runtime_hash = verify_runtime(rpc, entry["proxy"], artifact, candidate)
            steps.append(
                {
                    "contract": key,
                    "proxy": entry["proxy"],
                    "baseline": entry["baseline_implementation"],
                    "baselineCodeHash": entry["baseline_code_hash"],
                    "implementation": candidate,
                    "runtimeCodeHash": runtime_hash,
                    "artifact_sha256": hashlib.sha256(artifact_path.read_bytes()).hexdigest(),
                }
            )
        block = int(rpc.call("eth_blockNumber", []), 16)
        repo = repo_for("zkc" if label == "ethereum" and ENVIRONMENT == "rehearsal" else "base")
        local_bundle = repo / "out/rehearsal" / ENVIRONMENT / label / "transaction.json"
        local_bundle.parent.mkdir(parents=True, exist_ok=True)
        reference = export_transaction(rpc, label, chain_id, steps, local_bundle, block, safe)
        prepare_args = [
            "python3",
            str(Path(__file__).with_name("prepare.py")),
            "--environment",
            "staging" if ENVIRONMENT == "rehearsal" else "production",
            "--zkc-repo",
            str(ZKC),
            "--chain",
            label,
            "--rpc-url",
            rpc.url,
            "--output",
            str(local_bundle),
        ]
        for step in steps:
            prepare_args.extend(["--candidate", step["contract"] + "=" + step["implementation"]])
        prepared = subprocess.run(prepare_args, capture_output=True, text=True)
        (folder / "preparation.log").write_text(prepared.stdout + prepared.stderr)
        assert prepared.returncode == 0, (
            label + ": candidate preparation failed; see " + str(folder / "preparation.log")
        )
        tx = json.loads(local_bundle.read_text())
        assert (
            tx["data"] == reference["data"]
            and tx["safeTxHash"] == reference["safeTxHash"]
            and tx["nonce"] == reference["nonce"]
        )
        assert tx["local_fixture"] and tx["rpc_reads_only"] and not tx["live_submission_ready"]
        (folder / "transaction.json").write_text(local_bundle.read_text())
        (folder / "safe-builder.json").write_text(local_bundle.with_name("safe-builder.json").read_text())
        rollback_bundle = local_bundle.parent / "rollback/transaction.json"
        rollback = json.loads(rollback_bundle.read_text())
        (folder / "rollback").mkdir(exist_ok=True)
        for name in ["transaction.json", "safe-builder.json"]:
            (folder / "rollback" / name).write_bytes((rollback_bundle.parent / name).read_bytes())
        env = os.environ.copy()
        env.update(
            FOUNDRY_FFI="false",
            FOUNDRY_SOLC="0.8.26",
            FOUNDRY_EVM_VERSION="shanghai" if label == "taiko" else "cancun",
            FORGE_SNAPSHOT_EMIT="false",
            FOUNDRY_PROFILE="shanghai" if label == "taiko" else "default",
            FOUNDRY_BUILD_INFO_PATH="out/rehearsal-build-info-" + label,
        )
        if ENVIRONMENT == "rehearsal":
            env.update(
                FOUNDRY_TEST=("staging-fork" if label == "ethereum" else "contracts/staging-fork"),
                SUNSET_STAGING_BUNDLE=str(local_bundle),
            )
            contract = "StagingEthereumForkTest" if label == "ethereum" else "StagingMarketForkTest"
            tests = [
                "forge",
                "test",
                "--offline",
                "--network",
                "base" if label == "base" else "ethereum",
                "--hardfork",
                "base:" + hardfork if label == "base" else hardfork,
                "--fork-url",
                rpc.url,
                "--fork-block-number",
                str(block),
                "--out",
                "out/staging-fork-" + label,
                "--cache-path",
                "cache/staging-fork-" + label,
                "--match-contract",
                contract,
                "-vv",
            ]
            run_tests(tests, repo, env, folder, "tests")
        else:
            env.update(SUNSET_BLOCK_NUMBER=str(block), SUNSET_REPORT_DIR=str(folder))
            if label in ("base", "taiko"):
                env.update(SUNSET_IMPL=steps[0]["implementation"])
                env["BASE_RPC_URL" if label == "base" else "TAIKO_RPC_URL"] = rpc.url
                run_tests(
                    ["bash", "contracts/fork-test/run.sh", label],
                    MARKET,
                    env,
                    folder,
                    "tests",
                )
            else:
                env.update(
                    ETHEREUM_RPC_URL=rpc.url,
                    ACCOUNTING_SUNSET_IMPL=steps[0]["implementation"],
                    MINT_SUNSET_IMPL=steps[1]["implementation"],
                )
                run_tests(
                    ["bash", "contracts/fork-test/run.sh", "povw"],
                    MARKET,
                    env,
                    folder,
                    "povw-tests",
                )
                env.update(SUNSET_IMPL=steps[2]["implementation"])
                run_tests(["bash", "fork-test/run.sh"], ZKC, env, folder, "zkc-tests")
                env.update(
                    FOUNDRY_TEST="contracts/fork-test",
                    FOUNDRY_PROFILE="povw-deploy",
                    SUNSET_BUNDLE=str(local_bundle),
                    SUNSET_ROLLBACK_BUNDLE=str(rollback_bundle),
                )
                run_tests(
                    [
                        "forge",
                        "test",
                        "--offline",
                        "--fork-url",
                        rpc.url,
                        "--fork-block-number",
                        str(block),
                        "--out",
                        "out/sunset-fork-povw",
                        "--cache-path",
                        "cache/sunset-fork-povw",
                        "--match-contract",
                        "EthereumBatchForkTest",
                        "-vv",
                    ],
                    MARKET,
                    env,
                    folder,
                    "batch-tests",
                )
        # Foundry forks independently. Also execute the exported bytes on the parent Anvil.
        execution = execute_exact(rpc, tx)
        snapshot = rpc.call("evm_snapshot", [])
        rollbacks = []
        for after_cutoff in [False, True]:
            if after_cutoff:
                assert rpc.call("evm_revert", [snapshot])
                rpc.call("evm_setNextBlockTimestamp", [manifest["cutoff_timestamp"] + 1])
                rpc.call("evm_mine", [])
            # Refresh through the read-only CLI against the installed candidates, even after T.
            refreshed_path = folder / (
                "rollback-refreshed-after" if after_cutoff else "rollback-refreshed-before"
            ) / "transaction.json"
            refresh_args = prepare_args.copy()
            refresh_args[refresh_args.index("--output") + 1] = str(refreshed_path)
            refreshed = subprocess.run(refresh_args + ["--rollback"], capture_output=True, text=True)
            (folder / ("rollback-refresh-after.log" if after_cutoff else "rollback-refresh-before.log")).write_text(refreshed.stdout + refreshed.stderr)
            assert refreshed.returncode == 0, "rollback refresh failed"
            fresh = json.loads(refreshed_path.read_text())
            for field in ["safe", "to", "value", "data", "operation", "nonce", "safeTxHash"]:
                assert fresh[field] == rollback[field], "rollback refresh changed " + field
            rollbacks.append(dict(after_cutoff=after_cutoff, **execute_exact(rpc, rollback)))
        result = {
            "status": "passed",
            "label": label,
            "chain_id": chain_id,
            "source_block": number,
            "source_block_hash": pinned["hash"],
            "local_candidate_block": block,
            "anvil_hardfork": hardfork,
            "anvil_execution": execution,
            "anvil_rollbacks": rollbacks,
            "live_writes": False,
            "live_submission_ready": False,
            "deployment_path": "Existing repository scripts on localhost Anvil with OpenZeppelin upgrade and storage validation enabled.",
            "limitations": "Local fixture addresses and impersonated owners. Proof verifiers are mocked where required. No signer availability check.",
        }
        (folder / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(
            label + ": fork regressions and exact Anvil Safe execution passed",
            flush=True,
        )
        return result
    finally:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--chain", choices=["all", *CHAINS], default="all")
    parser.add_argument("--environment", choices=["production", "staging"], required=True)
    parser.add_argument("--zkc-repo", type=Path, default=ZKC)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    configure(args.environment, args.zkc_repo)
    manifest = json.loads(MANIFEST.read_text())
    (args.output / "baselines.json").write_bytes(MANIFEST.read_bytes())
    labels = list(CHAINS) if args.chain == "all" else [args.chain]
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
        futures = {pool.submit(run_one, label, args.output, manifest): label for label in labels}
        for future in concurrent.futures.as_completed(futures):
            try:
                results.append(future.result())
            except Exception as error:
                results.append({"label": futures[future], "status": "failed", "error": str(error)})
                print(futures[future] + ": " + str(error), flush=True)
    report = {
        "checked_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "local_only": True,
        "environment": args.environment,
        "tools": {tool: cmd(tool, "--version").splitlines()[0] for tool in ["forge", "anvil", "cast", "python3"]},
        "sources": {
            key: {
                "commit": cmd("git", "-C", str(repo), "rev-parse", "HEAD"),
                "status": cmd(
                    "git",
                    "-C",
                    str(repo),
                    "status",
                    "--porcelain",
                    "--untracked-files=no",
                ),
                "submodules": cmd("git", "-C", str(repo), "submodule", "status", "--recursive"),
            }
            for key, repo in [("market", MARKET), ("zkc", ZKC)]
        },
        "baseline_manifest_sha256": hashlib.sha256(MANIFEST.read_bytes()).hexdigest(),
        "reference_builds_sha256": {
            str(folder): {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(folder.glob("*.json"))}
            for folder in [MARKET / "contracts/build-info-reference", *sorted(ZKC.glob("build-info-reference-*"))]
            if folder.is_dir()
        },
        "results": results,
    }
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    raise SystemExit(0 if all(r["status"] == "passed" for r in results) else 1)


if __name__ == "__main__":
    main()
