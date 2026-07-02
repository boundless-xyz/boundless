// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {console} from "forge-std/console.sol";
import {IAccessControl} from "@openzeppelin/contracts/access/IAccessControl.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";
import {MessageHashUtils} from "@openzeppelin/contracts/utils/cryptography/MessageHashUtils.sol";
import {Test} from "forge-std/Test.sol";
import {Vm} from "forge-std/Vm.sol";
import {
    IRiscZeroVerifier,
    ReceiptClaim,
    Receipt as RiscZeroReceipt,
    ReceiptClaimLib,
    VerificationFailed
} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import {TestUtils} from "./TestUtils.sol";
import {Client} from "./clients/Client.sol";
import {IERC1967} from "@openzeppelin/contracts/interfaces/IERC1967.sol";
import {UnsafeUpgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {HitPoints} from "../src/HitPoints.sol";

import {BoundlessMarket} from "../src/BoundlessMarket.sol";
import {BoundlessRouter} from "../src/router/BoundlessRouter.sol";
import {IBoundlessVerifier} from "../src/router/interfaces/IBoundlessVerifier.sol";
import {IBoundlessAssessor} from "../src/router/interfaces/IBoundlessAssessor.sol";
import {IBoundlessJointVerifierAssessor} from "../src/router/interfaces/IBoundlessJointVerifierAssessor.sol";
import {NullVerifier, NullAssessor, NullJoint, RevertingVerifier} from "./mocks/RouterMocks.sol";
import {R0BoundlessVerifierAdapter} from "../src/router/adapters/R0BoundlessVerifierAdapter.sol";
import {R0BoundlessAssessorAdapter} from "../src/router/adapters/R0BoundlessAssessorAdapter.sol";
import {OnChainAssessor} from "../src/router/adapters/OnChainAssessor.sol";
import {AssessorCommitment} from "../src/types/AssessorCommitment.sol";
import {AssessorJournal} from "../src/types/AssessorJournal.sol";
import {FulfillmentLibrary} from "../src/types/Fulfillment.sol";
import {Callback} from "../src/types/Callback.sol";
import {
    FulfillmentDataImageIdAndJournal,
    FulfillmentDataLibrary,
    FulfillmentDataType
} from "../src/types/FulfillmentData.sol";
import {RequestId} from "../src/types/RequestId.sol";
import {AssessorCallback} from "../src/types/AssessorCallback.sol";
import {BoundlessMarketLib} from "../src/libraries/BoundlessMarketLib.sol";
import {MerkleProofish} from "../src/libraries/MerkleProofish.sol";
import {ProofRequest} from "../src/types/ProofRequest.sol";
import {LockRequest} from "../src/types/LockRequest.sol";
import {Fulfillment, LegacyFulfillment} from "../src/types/Fulfillment.sol";
import {FulfillmentBatch} from "../src/types/FulfillmentBatch.sol";
import {ProofRequestBatch} from "../src/types/ProofRequestBatch.sol";
import {SlimRequest, SlimRequestLibrary} from "../src/types/SlimRequest.sol";
import {Offer} from "../src/types/Offer.sol";
import {Requirements} from "../src/types/Requirements.sol";
import {Predicate, PredicateLibrary, PredicateType} from "../src/types/Predicate.sol";
import {IBoundlessMarket} from "../src/IBoundlessMarket.sol";

import {RiscZeroSetVerifier} from "risc0/RiscZeroSetVerifier.sol";
import {Fulfillment} from "../src/types/Fulfillment.sol";
import {MockCallback} from "./MockCallback.sol";
import {Selector} from "../src/types/Selector.sol";

import {SmartContractClient} from "./clients/SmartContractClient.sol";
import {IERC1271} from "@openzeppelin/contracts/interfaces/IERC1271.sol";

Vm constant VM = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

bytes32 constant APP_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000001;
bytes32 constant APP_IMAGE_ID_2 = 0x0000000000000000000000000000000000000000000000000000000000000002;
bytes32 constant SET_BUILDER_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000002;
bytes32 constant ASSESSOR_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000003;
// Non-functional legacy-impl placeholder. The new market's constructor rejects the zero address,
// but the new ABI never routes through the legacy fallback, so tests point legacyImpl at a dead
// address rather than a real BoundlessMarketLegacy. Fallback coverage lives in test/legacy/.
address constant LEGACY_IMPL_STUB = 0xdEDEDEDEdEdEdEDedEDeDedEdEdeDedEdEDedEdE;

bytes constant APP_JOURNAL = bytes("GUEST JOURNAL");
bytes constant APP_JOURNAL_2 = bytes("GUEST JOURNAL 2");

contract BoundlessMarketTest is Test {
    using ReceiptClaimLib for ReceiptClaim;
    using BoundlessMarketLib for Requirements;
    using BoundlessMarketLib for ProofRequest;
    using BoundlessMarketLib for Offer;
    using TestUtils for RiscZeroSetVerifier;
    using TestUtils for Selector[];
    using TestUtils for AssessorCallback[];
    using SafeCast for uint256;
    using SafeCast for int256;

    RiscZeroMockVerifier internal verifier;
    BoundlessMarket internal boundlessMarket;
    BoundlessRouter internal router;
    NullVerifier internal nullVerifier;
    NullAssessor internal nullAssessor;
    R0BoundlessVerifierAdapter internal setVerifierAdapter;
    R0BoundlessAssessorAdapter internal r0AssessorAdapter;
    OnChainAssessor internal onChainAssessor;

    address internal boundlessMarketSource;
    address internal legacyImpl;
    address internal proxy;
    RiscZeroSetVerifier internal setVerifier;
    HitPoints internal collateralToken;

    /// @dev Router class / entry selectors used in the test setup. The
    ///      assessor seal in each `FulfillmentBatch` starts with
    ///      `ASSESSOR_NULL_SEL` so the router dispatches to `NullAssessor`.
    bytes4 internal constant VERIFIER_CLASS_ID = 0x00000010;
    bytes4 internal constant VERIFIER_ENTRY_SEL = 0x00000011;
    bytes4 internal constant ASSESSOR_CLASS_ID = 0x00000020;
    bytes4 internal constant ASSESSOR_NULL_SEL = 0x00000023;
    /// @notice Router entry selector for the production R0 proof-based assessor
    ///         adapter. Used by e2e tests that exercise journal reconstruction
    ///         + setVerifier inclusion (selector, prover-mismatch, fill tampering).
    bytes4 internal constant ASSESSOR_R0_SEL = 0x00000024;
    /// @notice Router entry selector for the native Solidity `OnChainAssessor`.
    ///         Used by e2e tests that exercise the predicate-evaluation +
    ///         prover-ECDSA path through the production `fulfill` flow.
    bytes4 internal constant ASSESSOR_ON_CHAIN_SEL = 0x00000026;
    mapping(uint256 => Client) internal clients;
    mapping(uint256 => Client) internal provers;
    mapping(uint256 => SmartContractClient) internal smartContractClients;
    Client internal testProver;
    address internal testProverAddress;
    uint256 initialBalance;
    int256 internal stakeBalanceSnapshot;
    int256 internal collateralTreasuryBalanceSnapshot;

    uint256 constant DEFAULT_BALANCE = 1000 ether;
    uint256 constant EXPECTED_DEFAULT_MAX_GAS_FOR_VERIFY = 50000;
    uint256 constant EXPECTED_SLASH_BURN_BPS = 5000;

    ReceiptClaim internal appClaim = ReceiptClaimLib.ok(APP_IMAGE_ID, sha256(APP_JOURNAL));

    Vm.Wallet internal ownerWallet = vm.createWallet("OWNER");

    MockCallback internal mockCallback;
    MockCallback internal mockHighGasCallback;

    function setUp() public virtual {
        vm.deal(ownerWallet.addr, DEFAULT_BALANCE);

        vm.startPrank(ownerWallet.addr);

        // Deploy the implementation contracts
        verifier = new RiscZeroMockVerifier(bytes4(0));
        setVerifier = new RiscZeroSetVerifier(verifier, SET_BUILDER_IMAGE_ID, "https://set-builder.dev.null");
        collateralToken = new HitPoints(ownerWallet.addr);

        // Deploy and configure the BoundlessRouter with NullVerifier + NullAssessor.
        // Market state-machine tests don't exercise real cryptographic verification;
        // these mocks short-circuit verifier + assessor dispatch so each test runs
        // against the production fulfill path without paying for a real STARK.
        nullVerifier = new NullVerifier();
        nullAssessor = new NullAssessor();

        BoundlessRouter routerImpl = new BoundlessRouter();
        router = BoundlessRouter(
            UnsafeUpgrades.deployUUPSProxy(
                address(routerImpl), abi.encodeCall(BoundlessRouter.initialize, (ownerWallet.addr))
            )
        );

        router.addClass(
            ASSESSOR_CLASS_ID,
            BoundlessRouter.ClassMetadata({
                interfaceTag: type(IBoundlessAssessor).interfaceId,
                permissionlessInstantiate: false,
                isDefault: false,
                requiredAssessorClass: bytes4(0),
                schemaArtifact: bytes32(0),
                schemaArtifactUrl: "",
                defaultGasLimit: 10_000_000,
                label: ""
            })
        );
        router.instantiate(ASSESSOR_NULL_SEL, address(nullAssessor), ASSESSOR_CLASS_ID, 0);

        // Register the production R0 proof-based assessor adapter alongside
        // NullAssessor. E2e tests that need real journal reconstruction +
        // setVerifier inclusion (prover binding, fill tampering) opt into this
        // entry via the set-builder R0 fixture.
        r0AssessorAdapter = new R0BoundlessAssessorAdapter(setVerifier, ASSESSOR_IMAGE_ID);
        router.instantiate(ASSESSOR_R0_SEL, address(r0AssessorAdapter), ASSESSOR_CLASS_ID, 0);

        // Register the native Solidity `OnChainAssessor` as a third entry.
        // Used by e2e tests that exercise the alternative-assessor path
        // (predicate eval + prover ECDSA) through the production fulfill flow.
        onChainAssessor = new OnChainAssessor();
        router.instantiate(ASSESSOR_ON_CHAIN_SEL, address(onChainAssessor), ASSESSOR_CLASS_ID, 0);

        router.addClass(
            VERIFIER_CLASS_ID,
            BoundlessRouter.ClassMetadata({
                interfaceTag: type(IBoundlessVerifier).interfaceId,
                permissionlessInstantiate: false,
                isDefault: true,
                requiredAssessorClass: ASSESSOR_CLASS_ID,
                schemaArtifact: bytes32(0),
                schemaArtifactUrl: "",
                defaultGasLimit: 100_000,
                label: ""
            })
        );
        router.instantiate(VERIFIER_ENTRY_SEL, address(nullVerifier), VERIFIER_CLASS_ID, 0);

        // Also expose the real RiscZeroSetVerifier through the router under its
        // own selector, wrapped in `R0BoundlessVerifierAdapter`. Tests that need
        // set-builder inclusion-proof seals (callbacks, `submitRoot*`) sign
        // `setVerifier.SELECTOR()` and dispatch through this entry; the seal
        // produced is also valid for the callback's internal `verifyIntegrity`.
        setVerifierAdapter = new R0BoundlessVerifierAdapter(setVerifier);
        router.instantiate(setVerifier.SELECTOR(), address(setVerifierAdapter), VERIFIER_CLASS_ID, 0);

        // The new ABI never routes through the legacy fallback, so the new market's
        // delegate-call target is a non-functional stub rather than a real
        // BoundlessMarketLegacy. Fallback compatibility is covered in test/legacy/.
        legacyImpl = LEGACY_IMPL_STUB;

        // Deploy the UUPS proxy with the implementation
        boundlessMarketSource = address(new BoundlessMarket(router, address(collateralToken), legacyImpl));
        proxy = UnsafeUpgrades.deployUUPSProxy(
            boundlessMarketSource, abi.encodeCall(BoundlessMarket.initialize, (ownerWallet.addr))
        );
        boundlessMarket = BoundlessMarket(payable(proxy));

        mockCallback = new MockCallback(setVerifier, address(boundlessMarket), APP_IMAGE_ID, 10_000);
        mockHighGasCallback = new MockCallback(setVerifier, address(boundlessMarket), APP_IMAGE_ID, 250_000);

        collateralToken.grantMinterRole(ownerWallet.addr);
        collateralToken.grantAuthorizedTransferRole(proxy);
        vm.stopPrank();

        testProver = getProver(1);
        testProverAddress = testProver.addr();
        for (uint256 i = 0; i < 5; i++) {
            getClient(i);
            getProver(i);
            getSmartContractClient(i);
        }

        initialBalance = address(boundlessMarket).balance;

        stakeBalanceSnapshot = type(int256).max;
        collateralTreasuryBalanceSnapshot = type(int256).max;

        // Verify that OWNER has the admin role
        assertTrue(
            boundlessMarket.hasRole(boundlessMarket.ADMIN_ROLE(), ownerWallet.addr),
            "OWNER address does not have admin role after deployment"
        );
    }

    function expectedSlashBurnAmount(uint256 amount) internal pure returns (uint96) {
        return uint96((uint256(amount) * EXPECTED_SLASH_BURN_BPS) / 10000);
    }

    function expectedSlashTransferAmount(uint256 amount) internal pure returns (uint96) {
        return uint96((uint256(amount) * (10000 - EXPECTED_SLASH_BURN_BPS)) / 10000);
    }

    function expectMarketBalanceUnchanged() internal view {
        uint256 finalBalance = address(boundlessMarket).balance;
        console.log("Initial balance:", initialBalance);
        console.log("Final balance:", finalBalance);
        require(finalBalance == initialBalance, "Market balance changed during the test");
    }

    function snapshotMarketCollateralBalance() public {
        stakeBalanceSnapshot = collateralToken.balanceOf(address(boundlessMarket)).toInt256();
    }

    function expectMarketCollateralBalanceChange(int256 change) public view {
        require(stakeBalanceSnapshot != type(int256).max, "market stake balance snapshot is not set");
        int256 newBalance = collateralToken.balanceOf(address(boundlessMarket)).toInt256();
        console.log("Market stake balance at block %d: %d", block.number, newBalance.toUint256());
        int256 expectedBalance = stakeBalanceSnapshot + change;
        require(expectedBalance >= 0, "expected market stake balance cannot be less than 0");
        console.log("Market expected stake balance at block %d: %d", block.number, expectedBalance.toUint256());
        require(expectedBalance == newBalance, "market stake balance is not equal to expected value");
    }

    function snapshotMarketStakeTreasuryBalance() public {
        collateralTreasuryBalanceSnapshot = boundlessMarket.balanceOfCollateral(address(boundlessMarket)).toInt256();
    }

    function expectMarketCollateralTreasuryBalanceChange(int256 change) public view {
        require(
            collateralTreasuryBalanceSnapshot != type(int256).max,
            "market collateral treasury balance snapshot is not set"
        );
        int256 newBalance = boundlessMarket.balanceOfCollateral(address(boundlessMarket)).toInt256();
        console.log("Market stake treasury balance at block %d: %d", block.number, newBalance.toUint256());
        int256 expectedBalance = collateralTreasuryBalanceSnapshot + change;
        require(expectedBalance >= 0, "expected market treasury stake balance cannot be less than 0");
        console.log("Market expected stake treasury balance at block %d: %d", block.number, expectedBalance.toUint256());
        require(expectedBalance == newBalance, "market stake treasury balance is not equal to expected value");
    }

    function expectRequestFulfilled(RequestId requestId) internal view {
        require(boundlessMarket.requestIsFulfilled(requestId), "Request should be fulfilled");
        require(!boundlessMarket.requestIsSlashed(requestId), "Request should not be slashed");
    }

    /// Reconstructs the legacy-shaped `ProofDelivered` payload from the request identity and the
    /// current `Fulfillment`, mirroring what `BoundlessMarket` emits. Used by `expectEmit` checks.
    function _legacyFill(RequestId id, bytes32 requestDigest, Fulfillment memory fill)
        internal
        pure
        returns (LegacyFulfillment memory)
    {
        return LegacyFulfillment({
            id: id,
            requestDigest: requestDigest,
            claimDigest: fill.claimDigest,
            fulfillmentDataType: fill.fulfillmentDataType,
            fulfillmentData: fill.fulfillmentData,
            seal: fill.seal
        });
    }

    function expectRequestFulfilledAndSlashed(RequestId requestId) internal view {
        require(boundlessMarket.requestIsFulfilled(requestId), "Request should be fulfilled");
        require(boundlessMarket.requestIsSlashed(requestId), "Request should be slashed");
    }

    function expectRequestNotFulfilled(RequestId requestId) internal view {
        require(!boundlessMarket.requestIsFulfilled(requestId), "Request should not be fulfilled");
    }

    function expectRequestSlashed(RequestId requestId) internal view {
        require(boundlessMarket.requestIsSlashed(requestId), "Request should be slashed");
    }

    function expectRequestNotSlashed(RequestId requestId) internal view {
        require(!boundlessMarket.requestIsSlashed(requestId), "Request should be slashed");
    }

    // Creates a client account with the given index, gives it some Ether,
    // gives it some Stake Token, and deposits both into the market.
    function getClient(uint256 index) internal returns (Client) {
        if (address(clients[index]) != address(0)) {
            return clients[index];
        }
        Client client = createClientContract(string.concat("CLIENT_", vm.toString(index)));
        fundClient(client);
        clients[index] = client;
        return client;
    }

    // Creates a client account with the given index, gives it some Ether,
    // gives it some Stake Token, and deposits both into the market.
    function getSmartContractClient(uint256 index) internal returns (SmartContractClient) {
        if (address(smartContractClients[index]) != address(0)) {
            return smartContractClients[index];
        }
        SmartContractClient client = createSmartContractClientContract(string.concat("SC_CLIENT_", vm.toString(index)));
        fundSmartContractClient(client);
        smartContractClients[index] = client;
        return client;
    }

    // Creates a prover account with the given index, gives it some Ether,
    // gives it some Stake Token, and deposits both into the market.
    function getProver(uint256 index) internal returns (Client) {
        if (address(provers[index]) != address(0)) {
            return provers[index];
        }
        Client prover = createClientContract(string.concat("PROVER_", vm.toString(index)));
        fundClient(prover);
        provers[index] = prover;
        return prover;
    }

    function fundClient(Client client) internal {
        address clientAddress = client.addr();
        // Deal the client from Ether and deposit it in the market.
        vm.deal(clientAddress, DEFAULT_BALANCE);
        vm.prank(clientAddress);
        boundlessMarket.deposit{value: DEFAULT_BALANCE}();

        // Snapshot their initial ETH balance.
        client.snapshotBalance();

        // Mint some stake tokens.
        vm.prank(ownerWallet.addr);
        collateralToken.mint(clientAddress, DEFAULT_BALANCE);

        uint256 deadline = block.timestamp + 1 hours;
        (uint8 v, bytes32 r, bytes32 s) = client.signPermit(proxy, DEFAULT_BALANCE, deadline);
        vm.prank(clientAddress);
        boundlessMarket.depositCollateralWithPermit(DEFAULT_BALANCE, deadline, v, r, s);

        // Snapshot their initial stake balance.
        client.snapshotCollateralBalance();
    }

    function fundSmartContractClient(SmartContractClient client) internal {
        address walletAddress = client.addr();
        address signerAddress = client.signerAddr();

        // Deal the SCW some Ether and deposit it in the market.
        vm.deal(walletAddress, DEFAULT_BALANCE);
        vm.prank(signerAddress);
        client.execute(
            address(boundlessMarket),
            abi.encodeWithSelector(IBoundlessMarket.deposit.selector, DEFAULT_BALANCE),
            DEFAULT_BALANCE
        );

        // Snapshot their initial ETH balance.
        client.snapshotBalance();

        // Mint some stake tokens.
        vm.prank(ownerWallet.addr);
        collateralToken.mint(walletAddress, DEFAULT_BALANCE);

        vm.prank(signerAddress);
        client.execute(
            address(collateralToken), abi.encodeWithSelector(IERC20.approve.selector, boundlessMarket, DEFAULT_BALANCE)
        );

        vm.prank(signerAddress);
        client.execute(
            address(boundlessMarket),
            abi.encodeWithSelector(IBoundlessMarket.depositCollateral.selector, DEFAULT_BALANCE)
        );

        // check balances
        assertEq(boundlessMarket.balanceOf(walletAddress), DEFAULT_BALANCE);
        assertEq(boundlessMarket.balanceOfCollateral(walletAddress), DEFAULT_BALANCE);

        // Snapshot their initial stake balance.
        client.snapshotCollateralBalance();
    }

    // Create a client, using a trick to set the address equal to the wallet address.
    function createClientContract(string memory identifier) internal returns (Client) {
        Vm.Wallet memory wallet = vm.createWallet(identifier);
        Client client = new Client(wallet);
        client.initialize(identifier, boundlessMarket, collateralToken);
        return client;
    }

    function createSmartContractClientContract(string memory identifier) internal returns (SmartContractClient) {
        Vm.Wallet memory signer = vm.createWallet(string.concat(identifier, "_SIGNER"));
        SmartContractClient client = new SmartContractClient(signer);
        client.initialize(identifier, boundlessMarket, collateralToken);
        return client;
    }

    // ─── New helpers (slim/router architecture) ──────────────────────────
    //
    // The market state-machine tests build a `FulfillmentBatch` for the
    // registered `NullAssessor`. The assessor seal is just the 4-byte
    // selector — no merkle root, no inclusion proofs, no set-builder. The
    // per-fill `seal` only needs its first 4 bytes to resolve to a
    // registered verifier entry; `NullVerifier` accepts any bytes after.
    //
    // Tests that need a set-builder root (the `submitRoot*` path) will get
    // their own helper introduced when the first such test is restored.

    /// @dev Single-request convenience wrapper around `createFulfillmentBatch`.
    function createFulfillmentBatch(ProofRequest memory request, bytes memory journal, address prover)
        internal
        view
        returns (FulfillmentBatch memory)
    {
        ProofRequest[] memory requests = new ProofRequest[](1);
        requests[0] = request;
        bytes[] memory journals = new bytes[](1);
        journals[0] = journal;
        return createFulfillmentBatch(requests, journals, prover, FulfillmentDataType.ImageIdAndJournal);
    }

    /// @dev Builds a `FulfillmentBatch` ready to feed to `fulfill(...)`. The
    ///      assessor seal carries only `ASSESSOR_NULL_SEL`; `NullAssessor`
    ///      reads nothing else. Each per-fill `seal` starts with
    ///      `VERIFIER_ENTRY_SEL` so the router dispatches to `NullVerifier`.
    function createFulfillmentBatch(ProofRequest[] memory requests, bytes[] memory journals, address prover)
        internal
        view
        returns (FulfillmentBatch memory)
    {
        return createFulfillmentBatch(requests, journals, prover, FulfillmentDataType.ImageIdAndJournal);
    }

    function createFulfillmentBatch(
        ProofRequest[] memory requests,
        bytes[] memory journals,
        address prover,
        FulfillmentDataType fillType
    ) internal view returns (FulfillmentBatch memory batch) {
        uint256 n = requests.length;
        SlimRequest[] memory slim = new SlimRequest[](n);
        Fulfillment[] memory fills = new Fulfillment[](n);
        for (uint256 i = 0; i < n; i++) {
            // Derive claimDigest from the predicate. For DigestMatch and
            // PrefixMatch the first 32 bytes of predicate.data are the imageId;
            // for ClaimDigestMatch the predicate.data IS the claimDigest.
            bytes32 claimDigest;
            bytes32 imageId;
            PredicateType ptype = requests[i].requirements.predicate.predicateType;
            if (ptype != PredicateType.ClaimDigestMatch) {
                imageId = bytesToBytes32(requests[i].requirements.predicate.data);
                claimDigest = ReceiptClaimLib.ok(imageId, sha256(journals[i])).digest();
            } else {
                imageId = APP_IMAGE_ID;
                claimDigest = bytesToBytes32(requests[i].requirements.predicate.data);
            }

            bytes memory fulfillmentData;
            if (fillType == FulfillmentDataType.ImageIdAndJournal) {
                fulfillmentData = abi.encode(FulfillmentDataImageIdAndJournal({imageId: imageId, journal: journals[i]}));
            }

            fills[i] = Fulfillment({
                claimDigest: claimDigest,
                fulfillmentDataType: fillType,
                fulfillmentData: fulfillmentData,
                seal: abi.encodePacked(VERIFIER_ENTRY_SEL, hex"deadbeef")
            });
            slim[i] = _toSlim(requests[i]);
        }
        batch = FulfillmentBatch({
            requests: slim, fills: fills, assessorSeal: abi.encodePacked(ASSESSOR_NULL_SEL), prover: prover
        });
    }

    /// @dev Build a `SlimRequest` from a `ProofRequest` for the harness.
    function _toSlim(ProofRequest memory req) internal pure returns (SlimRequest memory) {
        return SlimRequest({
            id: req.id,
            predicate: req.requirements.predicate,
            callback: req.requirements.callback,
            selector: req.requirements.selector,
            imageUrlHash: keccak256(bytes(req.imageUrl)),
            inputDigest: req.input.eip712Digest(),
            offerDigest: req.offer.eip712Digest()
        });
    }

    /// @dev Wrap a single `FulfillmentBatch` in a length-1 array (the shape
    ///      `boundlessMarket.fulfill(...)` takes).
    function _asArray(FulfillmentBatch memory batch) internal pure returns (FulfillmentBatch[] memory arr) {
        arr = new FulfillmentBatch[](1);
        arr[0] = batch;
    }

    /// @dev Wrap a single `ProofRequestBatch` in a length-1 array (the shape
    ///      `priceAndFulfill(...)` takes).
    function _asArray(ProofRequestBatch memory rb) internal pure returns (ProofRequestBatch[] memory arr) {
        arr = new ProofRequestBatch[](1);
        arr[0] = rb;
    }

    /// @dev Wrap a single `ProofRequest` in a length-1 array.
    function _asArray(ProofRequest memory request) internal pure returns (ProofRequest[] memory arr) {
        arr = new ProofRequest[](1);
        arr[0] = request;
    }

    /// @dev Wrap a single signature (`bytes`) in a length-1 array.
    function _asArray(bytes memory signature) internal pure returns (bytes[] memory arr) {
        arr = new bytes[](1);
        arr[0] = signature;
    }

    // ─── Set-builder fixture (real setVerifier seals) ────────────────────
    //
    // Used for tests where the per-fill seal must be valid against
    // `RiscZeroSetVerifier.verifyIntegrity` — currently:
    //   * callback tests (the `BoundlessMarketCallback` defense re-verify),
    //   * `submitRoot*` tests (root submission side-channel).
    //
    // The harness builds the inclusion-proof seal off-chain via `TestUtils`,
    // submits the merkle root to setVerifier, and asks each request to sign
    // `setVerifier.SELECTOR()` so the router dispatches the fill through the
    // `R0BoundlessVerifierAdapter` registered in `setUp`. The same seal flows
    // unchanged to the callback's internal `verifyIntegrity`.

    function submitRoot(bytes32 root) internal {
        boundlessMarket.submitRoot(
            address(setVerifier),
            root,
            verifier.mockProve(
                SET_BUILDER_IMAGE_ID, sha256(abi.encodePacked(SET_BUILDER_IMAGE_ID, uint256(1 << 255), root))
            )
            .seal
        );
    }

    function createFillAndSubmitRoot(ProofRequest memory request, bytes memory journal, address prover)
        internal
        returns (FulfillmentBatch memory)
    {
        return createFillAndSubmitRoot(request, journal, prover, FulfillmentDataType.ImageIdAndJournal);
    }

    function createFillAndSubmitRoot(
        ProofRequest memory request,
        bytes memory journal,
        address prover,
        FulfillmentDataType fillType
    ) internal returns (FulfillmentBatch memory) {
        return createFillsAndSubmitRoot(_asArray(request), _asArray(journal), prover, fillType);
    }

    function createFillsAndSubmitRoot(ProofRequest[] memory requests, bytes[] memory journals, address prover)
        internal
        returns (FulfillmentBatch memory)
    {
        return createFillsAndSubmitRoot(requests, journals, prover, FulfillmentDataType.ImageIdAndJournal);
    }

    function createFillsAndSubmitRoot(
        ProofRequest[] memory requests,
        bytes[] memory journals,
        address prover,
        FulfillmentDataType fillType
    ) internal returns (FulfillmentBatch memory batch) {
        bytes32 root;
        (batch, root) = createFills(requests, journals, prover, fillType);
        // submit the root to the set verifier
        submitRoot(root);
    }

    function createFills(
        ProofRequest[] memory requests,
        bytes[] memory journals,
        address prover,
        FulfillmentDataType fillType
    ) internal view returns (FulfillmentBatch memory batch, bytes32 root) {
        // Broker-side per-fill outputs. `SlimRequest` carries selector +
        // callback positionally, replacing the old per-batch aggregation
        // that fed the off-chain STARK assessor journal.
        (Fulfillment[] memory fills, SlimRequest[] memory slim,) = _buildFillsAndSlim(requests, journals, fillType);

        // compute the batchRoot of the batch Merkle Tree
        bytes32[][] memory tree;
        (root, tree) = TestUtils.mockSetBuilder(fills);

        // compute all the inclusion proofs for the fullfillments
        TestUtils.Proof[] memory proofs = TestUtils.computeProofs(tree);
        for (uint256 i = 0; i < fills.length; i++) {
            fills[i].seal = TestUtils.encodeSeal(setVerifier, proofs[i]);
        }
        batch = FulfillmentBatch({
            requests: slim, fills: fills, assessorSeal: abi.encodePacked(ASSESSOR_NULL_SEL), prover: prover
        });
    }

    function createFills(ProofRequest[] memory requests, bytes[] memory journals, address prover)
        internal
        view
        returns (FulfillmentBatch memory batch, bytes32 root)
    {
        (batch, root) = createFills(requests, journals, prover, FulfillmentDataType.ImageIdAndJournal);
    }

    // ─── R0 proof-based assessor fixture ─────────────────────────────────
    //
    // Simulates what a broker + the off-chain R0 assessor guest produce
    // before calling `fulfill`: per-fill claim digests + a journal-bound
    // STARK seal over the assessor's `(root, callbacks, selectors, prover)`
    // commitment. The market verifies that output through
    // `R0BoundlessAssessorAdapter` (assessor side) and `setVerifier`
    // (per-fill verifier side), both end-to-end.
    //
    // Merkle layout follows the broker's set-builder: a single tree pairs
    // the fill-claim merkle root with the assessor leaf, so a per-fill
    // seal carries `[…, assessorLeaf]` siblings and the assessor seal is a
    // single-sibling proof against `batchRoot`.

    function createFillAndSubmitRootR0(ProofRequest memory request, bytes memory journal, address prover)
        internal
        returns (FulfillmentBatch memory)
    {
        return createFillsAndSubmitRootR0(_asArray(request), _asArray(journal), prover);
    }

    function createFillsAndSubmitRootR0(ProofRequest[] memory requests, bytes[] memory journals, address prover)
        internal
        returns (FulfillmentBatch memory batch)
    {
        // Step 1: broker-side per-fill outputs — same pre-merkle stage
        // `createFills` uses, plus the domain-bound `requestDigests` the
        // assessor guest feeds into its journal.
        (Fulfillment[] memory fills, SlimRequest[] memory slim, bytes32[] memory requestDigests) =
            _buildFillsAndSlim(requests, journals, FulfillmentDataType.ImageIdAndJournal);
        // Step 2: stand in for the assessor guest — produce the
        // `AssessorJournal` commitment the guest would have signed.
        bytes32 journalDigest = TestUtils.r0JournalDigest(slim, fills, requestDigests, prover);
        // The assessor's STARK receipt commits to `(ASSESSOR_IMAGE_ID,
        // journalDigest)`; that claim digest is what setVerifier requires
        // to be included in the submitted set-builder root.
        bytes32 assessorClaimDigest = ReceiptClaimLib.ok(ASSESSOR_IMAGE_ID, journalDigest).digest();

        // Step 3: broker's set-builder — one tree pairs the fill-claim
        // merkle root with the assessor leaf at the top. Reuses the
        // existing `mockSetBuilder` / `fillInclusionProofs` /
        // `mockAssessorSeal` helpers so the merkle layout is identical to
        // production.
        (bytes32 batchRoot, bytes32[][] memory tree) = TestUtils.mockSetBuilder(fills);
        bytes32 assessorLeaf = TestUtils.hashLeaf(assessorClaimDigest);
        bytes32 root = MerkleProofish._hashPair(batchRoot, assessorLeaf);
        TestUtils.fillInclusionProofs(setVerifier, fills, assessorLeaf, tree);
        submitRoot(root);

        batch = FulfillmentBatch({
            requests: slim,
            fills: fills,
            assessorSeal: abi.encodePacked(ASSESSOR_R0_SEL, TestUtils.mockAssessorSeal(setVerifier, batchRoot)),
            prover: prover
        });
    }

    // ─── Native on-chain assessor fixture ────────────────────────────────
    //
    // Simulates what a broker produces when fulfilling via the native
    // Solidity `OnChainAssessor`: no STARK, no merkle root — just an
    // EIP-712 ECDSA signature by the prover over the batch's
    // `(prover, requestDigests, claimDigests)` tuple.
    //
    // Two variants, in shape parity with the existing helpers:
    //   * `createFulfillmentBatchOnChain` — NullVerifier per-fill seals; the
    //     fastest path through the market when the per-fill verifier isn't
    //     under test.
    //   * `createFillsAndSubmitRootOnChain` — real `RiscZeroSetVerifier`
    //     per-fill seals; required when a downstream consumer
    //     (`BoundlessMarketCallback`) re-verifies `(imageId, journal, seal)`.
    //
    // Both routes go through the SAME `OnChainAssessor` adapter — the only
    // difference is what verifier the per-fill seal targets.

    function createFulfillmentBatchOnChain(ProofRequest memory request, bytes memory journal, Vm.Wallet memory prover)
        internal
        view
        returns (FulfillmentBatch memory)
    {
        return createFulfillmentBatchOnChain(_asArray(request), _asArray(journal), prover);
    }

    function createFulfillmentBatchOnChain(
        ProofRequest[] memory requests,
        bytes[] memory journals,
        Vm.Wallet memory prover
    ) internal view returns (FulfillmentBatch memory batch) {
        (Fulfillment[] memory fills, SlimRequest[] memory slim, bytes32[] memory requestDigests) =
            _buildFillsAndSlim(requests, journals, FulfillmentDataType.ImageIdAndJournal);
        // Per-fill seal: NullVerifier entry — accepts any payload after the
        // 4-byte selector. Callback tests should use the setVerifier variant
        // instead.
        for (uint256 i = 0; i < fills.length; i++) {
            fills[i].seal = abi.encodePacked(VERIFIER_ENTRY_SEL, hex"deadbeef");
        }
        batch = FulfillmentBatch({
            requests: slim,
            fills: fills,
            assessorSeal: _buildOnChainAssessorSeal(prover, fills, requestDigests),
            prover: prover.addr
        });
    }

    function createFillAndSubmitRootOnChain(ProofRequest memory request, bytes memory journal, Vm.Wallet memory prover)
        internal
        returns (FulfillmentBatch memory)
    {
        return createFillsAndSubmitRootOnChain(_asArray(request), _asArray(journal), prover);
    }

    function createFillsAndSubmitRootOnChain(
        ProofRequest[] memory requests,
        bytes[] memory journals,
        Vm.Wallet memory prover
    ) internal returns (FulfillmentBatch memory batch) {
        (Fulfillment[] memory fills, SlimRequest[] memory slim, bytes32[] memory requestDigests) =
            _buildFillsAndSlim(requests, journals, FulfillmentDataType.ImageIdAndJournal);
        // Build a merkle tree over fill claim digests and submit the root to
        // setVerifier. Per-fill seals are inclusion proofs against that root,
        // signed against `setVerifier.SELECTOR()` — which is what callback
        // tests' requests sign so the router dispatches through
        // `R0BoundlessVerifierAdapter` → `RiscZeroSetVerifier`.
        (bytes32 root, bytes32[][] memory tree) = TestUtils.mockSetBuilder(fills);
        TestUtils.Proof[] memory proofs = TestUtils.computeProofs(tree);
        for (uint256 i = 0; i < fills.length; i++) {
            fills[i].seal = TestUtils.encodeSeal(setVerifier, proofs[i]);
        }
        submitRoot(root);
        batch = FulfillmentBatch({
            requests: slim,
            fills: fills,
            assessorSeal: _buildOnChainAssessorSeal(prover, fills, requestDigests),
            prover: prover.addr
        });
    }

    /// @dev Build the OnChainAssessor seal: selector || ECDSA signature by
    ///      `prover` over the EIP-712 hash of
    ///      `(prover, keccak(requestDigests), keccak(claimDigests))`.
    function _buildOnChainAssessorSeal(
        Vm.Wallet memory prover,
        Fulfillment[] memory fills,
        bytes32[] memory requestDigests
    ) internal view returns (bytes memory) {
        uint256 n = fills.length;
        bytes32[] memory claimDigests = new bytes32[](n);
        for (uint256 i = 0; i < n; i++) {
            claimDigests[i] = fills[i].claimDigest;
        }
        bytes32 structHash = keccak256(
            abi.encode(
                onChainAssessor.FULFILLMENT_BATCH_AUTH_TYPEHASH(),
                prover.addr,
                keccak256(abi.encodePacked(requestDigests)),
                keccak256(abi.encodePacked(claimDigests))
            )
        );
        bytes32 digest = MessageHashUtils.toTypedDataHash(onChainAssessor.DOMAIN_SEPARATOR(), structHash);
        // Use the (uint256 pk, bytes32 digest) overload — the Vm.Wallet form
        // bumps the wallet's nonce and so is not view-compatible.
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(prover.privateKey, digest);
        return abi.encodePacked(ASSESSOR_ON_CHAIN_SEL, r, s, v);
    }

    /// @dev Broker-side per-fill build, extracted from `createFills` so the
    ///      R0 fixture can reuse the loop. Returns fills with empty seals (set
    ///      by the caller via the appropriate merkle inclusion proof), slim
    ///      payloads, and the domain-bound `requestDigests` the assessor
    ///      guest feeds into its journal.
    function _buildFillsAndSlim(ProofRequest[] memory requests, bytes[] memory journals, FulfillmentDataType fillType)
        internal
        view
        returns (Fulfillment[] memory fills, SlimRequest[] memory slim, bytes32[] memory requestDigests)
    {
        uint256 n = requests.length;
        fills = new Fulfillment[](n);
        slim = new SlimRequest[](n);
        requestDigests = new bytes32[](n);
        bytes32 domainSeparator = boundlessMarket.eip712DomainSeparator();
        for (uint8 i = 0; i < n; i++) {
            bytes32 claimDigest;
            bytes memory fulfillmentData;
            bytes memory journal = journals[i];
            PredicateType predicateType = requests[i].requirements.predicate.predicateType;
            bytes32 imageId;
            if (predicateType != PredicateType.ClaimDigestMatch) {
                imageId = bytesToBytes32(requests[i].requirements.predicate.data);
                claimDigest = ReceiptClaimLib.ok(imageId, sha256(journal)).digest();
            } else {
                // this is hacky, but for ClaimDigestMatch, the imageId is not known,
                // so we just use the APP_IMAGE_ID as the default
                imageId = APP_IMAGE_ID;
                claimDigest = bytesToBytes32(requests[i].requirements.predicate.data);
            }
            if (fillType == FulfillmentDataType.ImageIdAndJournal) {
                fulfillmentData = abi.encode(FulfillmentDataImageIdAndJournal({imageId: imageId, journal: journal}));
            }
            fills[i] = Fulfillment({
                claimDigest: claimDigest,
                fulfillmentData: fulfillmentData,
                fulfillmentDataType: fillType,
                seal: bytes("")
            });
            slim[i] = _toSlim(requests[i]);
            requestDigests[i] = MessageHashUtils.toTypedDataHash(domainSeparator, requests[i].eip712Digest());
        }
    }

    function newBatch(uint256 batchSize) internal returns (ProofRequest[] memory requests, bytes[] memory journals) {
        requests = new ProofRequest[](batchSize);
        journals = new bytes[](batchSize);
        for (uint256 j = 0; j < 5; j++) {
            getClient(j);
        }
        for (uint256 i = 0; i < batchSize; i++) {
            Client client = clients[i % 5];
            ProofRequest memory request = client.request(uint32(i / 5));
            bytes memory clientSignature = client.sign(request);
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
            requests[i] = request;
            journals[i] = APP_JOURNAL;
        }
    }

    function newBatchWithSelector(uint256 batchSize, bytes4 selector)
        internal
        returns (ProofRequest[] memory requests, bytes[] memory journals)
    {
        requests = new ProofRequest[](batchSize);
        journals = new bytes[](batchSize);
        for (uint256 j = 0; j < 5; j++) {
            getClient(j);
        }
        for (uint256 i = 0; i < batchSize; i++) {
            Client client = clients[i % 5];
            ProofRequest memory request = client.request(uint32(i / 5));
            request.requirements.selector = selector;
            bytes memory clientSignature = client.sign(request);
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
            requests[i] = request;
            journals[i] = APP_JOURNAL;
        }
    }

    function newBatchWithCallback(uint256 batchSize)
        internal
        returns (ProofRequest[] memory requests, bytes[] memory journals)
    {
        requests = new ProofRequest[](batchSize);
        journals = new bytes[](batchSize);
        for (uint256 j = 0; j < 5; j++) {
            getClient(j);
        }
        for (uint256 i = 0; i < batchSize; i++) {
            Client client = clients[i % 5];
            ProofRequest memory request = client.request(uint32(i / 5));
            request.requirements.callback.addr = address(mockCallback);
            request.requirements.callback.gasLimit = 500_000;
            bytes memory clientSignature = client.sign(request);
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
            requests[i] = request;
            journals[i] = APP_JOURNAL;
        }
    }

    function bytesToBytes32(bytes memory b) internal pure returns (bytes32) {
        bytes32 out;
        for (uint256 i = 0; i < 32; i++) {
            out |= bytes32(b[i] & 0xFF) >> (i * 8);
        }
        return out;
    }
}

contract BoundlessMarketBasicTest is BoundlessMarketTest {
    using ReceiptClaimLib for ReceiptClaim;
    using BoundlessMarketLib for Offer;
    using BoundlessMarketLib for ProofRequest;
    using SafeCast for uint256;

    function _stringEquals(string memory a, string memory b) private pure returns (bool) {
        return keccak256(abi.encodePacked(a)) == keccak256(abi.encodePacked(b));
    }

    function testBytecodeSize() public {
        vm.snapshotValue("bytecode size proxy", address(proxy).code.length);
        vm.snapshotValue("bytecode size implementation", boundlessMarketSource.code.length);
    }

    function testDeposit() public {
        vm.deal(testProverAddress, 1 ether);
        // Deposit funds into the market
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.Deposit(testProverAddress, 1 ether);
        vm.prank(testProverAddress);
        boundlessMarket.deposit{value: 1 ether}();
        testProver.expectBalanceChange(1 ether);
    }

    function testDeposits() public {
        address newUser = address(uint160(3));
        vm.deal(newUser, 2 ether);

        // Deposit funds into the market
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.Deposit(newUser, 1 ether);
        vm.prank(newUser);
        boundlessMarket.deposit{value: 1 ether}();
        vm.snapshotGasLastCall("deposit: first ever deposit");

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.Deposit(newUser, 1 ether);
        vm.prank(newUser);
        boundlessMarket.deposit{value: 1 ether}();
        vm.snapshotGasLastCall("deposit: second deposit");
    }

    function testDepositTo() public {
        vm.deal(testProverAddress, 1 ether);
        // Deposit funds into the market
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.Deposit(testProverAddress, 1 ether);
        vm.prank(testProverAddress);
        boundlessMarket.depositTo{value: 1 ether}(testProverAddress);
        testProver.expectBalanceChange(1 ether);
    }

    function testDepositsTo() public {
        address newUser = address(uint160(3));
        vm.deal(newUser, 2 ether);

        // Deposit funds into the market
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.Deposit(newUser, 1 ether);
        vm.prank(newUser);
        boundlessMarket.depositTo{value: 1 ether}(newUser);
        vm.snapshotGasLastCall("depositTo: first ever deposit");

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.Deposit(newUser, 1 ether);
        vm.prank(newUser);
        boundlessMarket.depositTo{value: 1 ether}(newUser);
        vm.snapshotGasLastCall("depositTo: second deposit");
    }

    function testAdminRoleSetup() public view {
        assertTrue(
            boundlessMarket.hasRole(boundlessMarket.ADMIN_ROLE(), ownerWallet.addr), "Owner should have admin role"
        );
    }

    function testWithdraw() public {
        // Deposit funds into the market
        vm.deal(testProverAddress, 1 ether);
        vm.prank(testProverAddress);
        boundlessMarket.deposit{value: 1 ether}();

        // Withdraw funds from the market
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.Withdrawal(testProverAddress, 1 ether);
        vm.prank(testProverAddress);
        boundlessMarket.withdraw(1 ether);
        expectMarketBalanceUnchanged();

        // Attempt to withdraw extra funds from the market.
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InsufficientBalance.selector, testProverAddress));
        vm.prank(testProverAddress);
        boundlessMarket.withdraw(DEFAULT_BALANCE + 1);
        expectMarketBalanceUnchanged();
    }

    function testWithdrawals() public {
        // Deposit funds into the market
        vm.deal(testProverAddress, 3 ether);
        vm.prank(testProverAddress);
        boundlessMarket.deposit{value: 3 ether}();

        // Withdraw funds from the market
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.Withdrawal(testProverAddress, 1 ether);
        vm.prank(testProverAddress);
        boundlessMarket.withdraw(1 ether);
        vm.snapshotGasLastCall("withdraw: 1 ether");

        uint256 balance = boundlessMarket.balanceOf(testProverAddress);
        vm.prank(testProverAddress);
        boundlessMarket.withdraw(balance);
        vm.snapshotGasLastCall("withdraw: full balance");
        assertEq(boundlessMarket.balanceOf(testProverAddress), 0);

        // Attempt to withdraw extra funds from the market.
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InsufficientBalance.selector, testProverAddress));
        vm.prank(testProverAddress);
        boundlessMarket.withdraw(DEFAULT_BALANCE + 1);
    }

    function testCollateralDeposit() public {
        // Mint some tokens
        vm.prank(ownerWallet.addr);
        collateralToken.mint(testProverAddress, 2);

        // Approve the market to spend the testProver's collateralToken
        vm.prank(testProverAddress);
        ERC20(address(collateralToken)).approve(address(boundlessMarket), 2);
        vm.snapshotGasLastCall("ERC20 approve: required for depositCollateral");

        // Deposit stake into the market
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.CollateralDeposit(testProverAddress, 1);
        vm.prank(testProverAddress);
        boundlessMarket.depositCollateral(1);
        vm.snapshotGasLastCall("depositCollateral: 1 HP (tops up market account)");
        testProver.expectCollateralBalanceChange(1);

        // Deposit stake into the market
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.CollateralDeposit(testProverAddress, 1);
        vm.prank(testProverAddress);
        boundlessMarket.depositCollateral(1);
        vm.snapshotGasLastCall("depositCollateral: full (drains testProver account)");
        testProver.expectCollateralBalanceChange(2);
    }

    function testCollateralDepositWithPermit() public {
        // Mint some tokens
        vm.prank(ownerWallet.addr);
        collateralToken.mint(testProverAddress, 2);

        // Approve the market to spend the testProver's collateralToken
        uint256 deadline = block.timestamp + 1 hours;
        (uint8 v, bytes32 r, bytes32 s) = testProver.signPermit(address(boundlessMarket), 1, deadline);

        // Deposit stake into the market
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.CollateralDeposit(testProverAddress, 1);
        vm.prank(testProverAddress);
        boundlessMarket.depositCollateralWithPermit(1, deadline, v, r, s);
        vm.snapshotGasLastCall("depositCollateralWithPermit: 1 HP (tops up market account)");
        testProver.expectCollateralBalanceChange(1);

        // Approve the market to spend the testProver's collateralToken
        (v, r, s) = testProver.signPermit(address(boundlessMarket), 1, deadline);

        // Deposit stake into the market
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.CollateralDeposit(testProverAddress, 1);
        vm.prank(testProverAddress);
        boundlessMarket.depositCollateralWithPermit(1, deadline, v, r, s);
        vm.snapshotGasLastCall("depositCollateralWithPermit: full (drains testProver account)");
        testProver.expectCollateralBalanceChange(2);
    }

    function testCollateralDepositTo() public {
        Client sender = getClient(2);
        Client receiver = getClient(3);
        address senderAddr = sender.addr();
        address receiverAddr = receiver.addr();

        vm.prank(ownerWallet.addr);
        collateralToken.mint(senderAddr, 2);

        vm.prank(senderAddr);
        ERC20(address(collateralToken)).approve(address(boundlessMarket), 2);

        uint256 senderBalanceBefore = boundlessMarket.balanceOfCollateral(senderAddr);
        uint256 receiverBalanceBefore = boundlessMarket.balanceOfCollateral(receiverAddr);

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.CollateralDeposit(receiverAddr, 1);
        vm.prank(senderAddr);
        boundlessMarket.depositCollateralTo(receiverAddr, 1);

        assertEq(boundlessMarket.balanceOfCollateral(senderAddr), senderBalanceBefore);
        assertEq(boundlessMarket.balanceOfCollateral(receiverAddr), receiverBalanceBefore + 1);
    }

    function testCollateralDepositWithPermitTo() public {
        Client sender = getClient(2);
        Client receiver = getClient(3);
        address senderAddr = sender.addr();
        address receiverAddr = receiver.addr();

        vm.prank(ownerWallet.addr);
        collateralToken.mint(senderAddr, 2);

        uint256 deadline = block.timestamp + 1 hours;
        (uint8 v, bytes32 r, bytes32 s) = sender.signPermit(address(boundlessMarket), 1, deadline);

        uint256 senderBalanceBefore = boundlessMarket.balanceOfCollateral(senderAddr);
        uint256 receiverBalanceBefore = boundlessMarket.balanceOfCollateral(receiverAddr);

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.CollateralDeposit(receiverAddr, 1);
        vm.prank(senderAddr);
        boundlessMarket.depositCollateralWithPermitTo(receiverAddr, 1, deadline, v, r, s);

        assertEq(boundlessMarket.balanceOfCollateral(senderAddr), senderBalanceBefore);
        assertEq(boundlessMarket.balanceOfCollateral(receiverAddr), receiverBalanceBefore + 1);
    }

    function testStakeWithdraw() public {
        // Withdraw stake from the market
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.CollateralWithdrawal(testProverAddress, 1);
        vm.prank(testProverAddress);
        boundlessMarket.withdrawCollateral(1);
        vm.snapshotGasLastCall("withdrawCollateral: 1 HP balance");
        testProver.expectCollateralBalanceChange(-1);
        assertEq(collateralToken.balanceOf(testProverAddress), 1, "TestProver should have 1 hitPoint after withdrawing");

        // Withdraw full stake from the market
        uint256 remainingBalance = boundlessMarket.balanceOfCollateral(testProverAddress);
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.CollateralWithdrawal(testProverAddress, remainingBalance);
        vm.prank(testProverAddress);
        boundlessMarket.withdrawCollateral(remainingBalance);
        vm.snapshotGasLastCall("withdrawCollateral: full balance");
        testProver.expectCollateralBalanceChange(-int256(DEFAULT_BALANCE));
        assertEq(
            collateralToken.balanceOf(testProverAddress),
            DEFAULT_BALANCE,
            "TestProver should have DEFAULT_BALANCE hitPoint after withdrawing"
        );

        // Attempt to withdraw extra funds from the market.
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InsufficientBalance.selector, testProverAddress));
        vm.prank(testProverAddress);
        boundlessMarket.withdrawCollateral(1);
    }

    function testSubmitRequest() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);

        // Submit the request with no funds
        // Expect the event to be emitted
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestSubmitted(request.id, request, clientSignature);
        boundlessMarket.submitRequest(request, clientSignature);
        vm.snapshotGasLastCall("submitRequest: without ether");

        // Submit the request with funds
        // Expect the event to be emitted
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.Deposit(client.addr(), uint256(request.offer.maxPrice));
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestSubmitted(request.id, request, clientSignature);
        vm.deal(client.addr(), request.offer.maxPrice);
        address clientAddress = client.addr();
        vm.prank(clientAddress);
        boundlessMarket.submitRequest{value: request.offer.maxPrice}(request, clientSignature);
        vm.snapshotGasLastCall("submitRequest: with maxPrice ether");
    }

    function _testLockRequest(bool withSig) private returns (Client, ProofRequest memory) {
        return _testLockRequest(withSig, "");
    }

    function _testLockRequest(bool withSig, string memory snapshot) private returns (Client, ProofRequest memory) {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.signLockRequest(LockRequest({request: request}));

        // Expect the event to be emitted
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestLocked(request.id, testProverAddress, request, clientSignature);
        if (withSig) {
            boundlessMarket.lockRequestWithSignature(request, clientSignature, proverSignature);
        } else {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
        }

        if (!_stringEquals(snapshot, "")) {
            vm.snapshotGasLastCall(snapshot);
        }

        // Ensure the balances are correct
        client.expectBalanceChange(-1 ether);
        testProver.expectCollateralBalanceChange(-1 ether);

        // Verify the lock request
        assertTrue(boundlessMarket.requestIsLocked(request.id), "Request should be locked-in");

        expectMarketBalanceUnchanged();

        return (client, request);
    }

    function testLockRequest() public returns (Client, ProofRequest memory) {
        return _testLockRequest(false, "lockinRequest: base case");
    }

    function testLockRequestWithSignature() public returns (Client, ProofRequest memory) {
        return _testLockRequest(true, "lockinRequest: with prover signature");
    }

    function _testLockRequestAlreadyLocked(bool withSig) private {
        (Client client, ProofRequest memory request) = _testLockRequest(withSig);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.signLockRequest(LockRequest({request: request}));

        // Attempt to lock the request again
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsLocked.selector, request.id));
        if (withSig) {
            boundlessMarket.lockRequestWithSignature(request, clientSignature, proverSignature);
        } else {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
        }

        expectMarketBalanceUnchanged();
    }

    function testLockRequestAlreadyLocked() public {
        return _testLockRequestAlreadyLocked(true);
    }

    function testLockRequestWithSignatureAlreadyLocked() public {
        return _testLockRequestAlreadyLocked(false);
    }

    function _testLockRequestBadClientSignature(bool withSig) private {
        Client clientA = getClient(1);
        Client clientB = getClient(2);
        ProofRequest memory request1 = clientA.request(1);
        ProofRequest memory request2 = clientA.request(2);
        bytes memory proverSignature = testProver.signLockRequest(LockRequest({request: request1}));

        // case: request signed by a different client
        bytes memory badClientSignature = clientB.sign(request1);
        vm.expectRevert(IBoundlessMarket.InvalidSignature.selector);
        if (withSig) {
            boundlessMarket.lockRequestWithSignature(request1, badClientSignature, proverSignature);
        } else {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request1, badClientSignature);
        }

        // case: client signed a different request
        badClientSignature = clientA.sign(request2);
        vm.expectRevert(IBoundlessMarket.InvalidSignature.selector);
        if (withSig) {
            boundlessMarket.lockRequestWithSignature(request1, badClientSignature, proverSignature);
        } else {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request1, badClientSignature);
        }

        clientA.expectBalanceChange(0 ether);
        clientB.expectBalanceChange(0 ether);
        testProver.expectBalanceChange(0 ether);
        expectMarketBalanceUnchanged();
    }

    function testLockRequestBadClientSignature() public {
        return _testLockRequestBadClientSignature(true);
    }

    function testLockRequestWithSignatureBadClientSignature() public {
        return _testLockRequestBadClientSignature(false);
    }

    function testLockRequestWithSignatureProverSignatureIncorrectRequest() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        // Prover signs the incorrect request.
        bytes memory badProverSignature = testProver.signLockRequest(LockRequest({request: client.request(2)}));

        // Error is "InsufficientBalance" because we will recover _some_ address.
        // It should be random and never correspond to a real account. We use
        // expectPartialRevert so the recovered-signer address can change
        // freely with contract layout / deploy nonce / EIP-712 domain shifts
        // without breaking this regression test.
        vm.expectPartialRevert(IBoundlessMarket.InsufficientBalance.selector);
        boundlessMarket.lockRequestWithSignature(request, clientSignature, badProverSignature);

        client.expectBalanceChange(0 ether);
        testProver.expectBalanceChange(0 ether);
        expectMarketBalanceUnchanged();
    }

    function testLockRequestWithSignatureProverSignatureIncorrectDomain() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        // Prover signs ProofRequest struct rather than LockRequest struct.
        // NOTE: This was how the contract worked in a previous version. This is included as a regression test.
        bytes memory badProverSignature = testProver.sign(request);

        // Error is "InsufficientBalance" because we will recover _some_ address.
        // It should be random and never correspond to a real account. We use
        // expectPartialRevert so the recovered-signer address can change
        // freely with contract layout / deploy nonce / EIP-712 domain shifts
        // without breaking this regression test.
        vm.expectPartialRevert(IBoundlessMarket.InsufficientBalance.selector);
        boundlessMarket.lockRequestWithSignature(request, clientSignature, badProverSignature);

        client.expectBalanceChange(0 ether);
        testProver.expectBalanceChange(0 ether);
        expectMarketBalanceUnchanged();
    }

    function _testLockRequestNotEnoughFunds(bool withSig) private {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.signLockRequest(LockRequest({request: request}));

        address clientAddress = client.addr();
        vm.prank(clientAddress);
        boundlessMarket.withdraw(DEFAULT_BALANCE);

        // case: client does not have enough funds to cover for the lock request
        // should revert with "InsufficientBalance(address requester)"
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InsufficientBalance.selector, client.addr()));
        if (withSig) {
            boundlessMarket.lockRequestWithSignature(request, clientSignature, proverSignature);
        } else {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
        }

        vm.prank(clientAddress);
        boundlessMarket.deposit{value: DEFAULT_BALANCE}();

        vm.prank(testProverAddress);
        boundlessMarket.withdrawCollateral(DEFAULT_BALANCE);
        // case: prover does not have enough funds to cover for the lock request stake
        // should revert with "InsufficientBalance(address requester)"
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InsufficientBalance.selector, testProverAddress));
        if (withSig) {
            boundlessMarket.lockRequestWithSignature(request, clientSignature, proverSignature);
        } else {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
        }
    }

    function testLockRequestNotEnoughFunds() public {
        return _testLockRequestNotEnoughFunds(true);
    }

    function testLockRequestWithSignatureNotEnoughFunds() public {
        return _testLockRequestNotEnoughFunds(false);
    }

    function _testLockRequestExpired(bool withSig) private {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.signLockRequest(LockRequest({request: request}));

        vm.warp(request.offer.deadline() + 1);

        // Attempt to lock the request after it has expired
        // should revert with "RequestIsExpired({requestId: request.id, deadline: deadline})"
        vm.expectRevert(
            abi.encodeWithSelector(
                IBoundlessMarket.RequestLockIsExpired.selector, request.id, request.offer.lockDeadline()
            )
        );
        if (withSig) {
            boundlessMarket.lockRequestWithSignature(request, clientSignature, proverSignature);
        } else {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
        }

        expectMarketBalanceUnchanged();
    }

    function testLockRequestExpired() public {
        return _testLockRequestExpired(true);
    }

    function testLockRequestWithSignatureExpired() public {
        return _testLockRequestExpired(false);
    }

    function _testLockRequestLockExpired(bool withSig) private {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.signLockRequest(LockRequest({request: request}));

        vm.warp(request.offer.lockDeadline() + 1);

        vm.expectRevert(
            abi.encodeWithSelector(
                IBoundlessMarket.RequestLockIsExpired.selector, request.id, request.offer.lockDeadline()
            )
        );
        if (withSig) {
            boundlessMarket.lockRequestWithSignature(request, clientSignature, proverSignature);
        } else {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
        }

        expectMarketBalanceUnchanged();
    }

    function testLockRequestLockExpired() public {
        return _testLockRequestLockExpired(true);
    }

    function testLockRequestWithSignatureLockExpired() public {
        return _testLockRequestLockExpired(false);
    }

    function _testLockRequestInvalidRequest1(bool withSig) private {
        Offer memory offer = Offer({
            minPrice: 2 ether,
            maxPrice: 1 ether,
            rampUpStart: uint64(block.timestamp),
            rampUpPeriod: uint32(0),
            lockTimeout: uint32(1),
            timeout: uint32(1),
            lockCollateral: 10 ether
        });

        Client client = getClient(1);
        ProofRequest memory request = client.request(1, offer);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.signLockRequest(LockRequest({request: request}));

        // Attempt to lock a request with maxPrice smaller than minPrice
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InvalidRequest.selector));
        if (withSig) {
            boundlessMarket.lockRequestWithSignature(request, clientSignature, proverSignature);
        } else {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
        }

        expectMarketBalanceUnchanged();
    }

    function testLockRequestInvalidRequest1() public {
        return _testLockRequestInvalidRequest1(true);
    }

    function testLockRequestWithSignatureInvalidRequest1() public {
        return _testLockRequestInvalidRequest1(false);
    }

    function _testLockRequestInvalidRequest2(bool withSig) private {
        Offer memory offer = Offer({
            minPrice: 1 ether,
            maxPrice: 1 ether,
            rampUpStart: uint64(block.timestamp),
            rampUpPeriod: uint32(2),
            lockTimeout: uint32(1),
            timeout: uint32(1),
            lockCollateral: 10 ether
        });

        Client client = getClient(1);
        ProofRequest memory request = client.request(1, offer);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.signLockRequest(LockRequest({request: request}));

        // Attempt to lock a request with rampUpPeriod greater than timeout
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InvalidRequest.selector));
        if (withSig) {
            boundlessMarket.lockRequestWithSignature(request, clientSignature, proverSignature);
        } else {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
        }

        expectMarketBalanceUnchanged();
    }

    function testLockRequestInvalidRequest2() public {
        return _testLockRequestInvalidRequest2(true);
    }

    function testLockRequestWithSignatureInvalidRequest2() public {
        return _testLockRequestInvalidRequest2(false);
    }

    enum LockRequestMethod {
        LockRequest,
        LockRequestWithSig,
        None
    }

    function _testFulfillSameBlock(uint32 requestIdx, LockRequestMethod lockinMethod)
        private
        returns (Client, ProofRequest memory)
    {
        return _testFulfillSameBlock(requestIdx, lockinMethod, "");
    }

    /// @dev Base for fulfillment tests with different methods for lock,
    ///      including none. All three paths must yield the same result.
    function _testFulfillSameBlock(uint32 requestIdx, LockRequestMethod lockinMethod, string memory snapshot)
        private
        returns (Client, ProofRequest memory)
    {
        Client client = getClient(1);
        ProofRequest memory request = client.request(requestIdx);
        bytes memory clientSignature = client.sign(request);

        client.snapshotBalance();
        testProver.snapshotBalance();

        if (lockinMethod == LockRequestMethod.LockRequest) {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
        } else if (lockinMethod == LockRequestMethod.LockRequestWithSig) {
            boundlessMarket.lockRequestWithSignature(
                request, clientSignature, testProver.signLockRequest(LockRequest({request: request}))
            );
        }

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, testProverAddress);
        // `RequestFulfilled` emits the domain-bound digest the market computes
        // from the slim request via `_hashTypedDataV4`.
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );

        if (lockinMethod == LockRequestMethod.None) {
            // Build a `ProofRequestBatch` for the un-locked request so the
            // priced-path leg can verify its signature inside `priceAndFulfill`.
            boundlessMarket.priceAndFulfill(
                _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
                _asArray(batch)
            );
        } else {
            boundlessMarket.fulfill(_asArray(batch));
        }
        if (!_stringEquals(snapshot, "")) {
            vm.snapshotGasLastCall(snapshot);
        }

        expectRequestFulfilled(request.id);

        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();

        return (client, request);
    }

    /// @dev Base for fulfillmentAndWithdraw tests with different methods for
    ///      lock, including none. All three paths must yield the same result.
    function _testFulfillAndWithdrawSameBlock(uint32 requestIdx, LockRequestMethod lockinMethod, string memory snapshot)
        private
        returns (Client, ProofRequest memory)
    {
        Client client = getClient(1);
        ProofRequest memory request = client.request(requestIdx);
        bytes memory clientSignature = client.sign(request);

        client.snapshotBalance();
        testProver.snapshotBalance();

        if (lockinMethod == LockRequestMethod.LockRequest) {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
        } else if (lockinMethod == LockRequestMethod.LockRequestWithSig) {
            boundlessMarket.lockRequestWithSignature(
                request, clientSignature, testProver.signLockRequest(LockRequest({request: request}))
            );
        }

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, testProverAddress);
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        uint256 initialBalance = boundlessMarket.balanceOf(testProverAddress) + testProverAddress.balance;

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );

        if (lockinMethod == LockRequestMethod.None) {
            boundlessMarket.priceAndFulfillAndWithdraw(
                _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
                _asArray(batch)
            );
        } else {
            boundlessMarket.fulfillAndWithdraw(_asArray(batch));
        }
        if (!_stringEquals(snapshot, "")) {
            vm.snapshotGasLastCall(snapshot);
        }

        expectRequestFulfilled(request.id);

        client.expectBalanceChange(-1 ether);
        assert(boundlessMarket.balanceOf(testProverAddress) == 0);
        assert(testProverAddress.balance == initialBalance + 1 ether);

        return (client, request);
    }

    // Base for submitRoot and fulfillment tests with different methods for lock, including none. All paths should yield the same result.
    function _testSubmitRootAndFulfillSameBlock(
        uint32 requestIdx,
        LockRequestMethod lockinMethod,
        string memory snapshot
    ) private returns (Client, ProofRequest memory) {
        Client client = getClient(1);
        ProofRequest memory request = client.request(requestIdx);
        bytes memory clientSignature = client.sign(request);

        client.snapshotBalance();
        testProver.snapshotBalance();

        if (lockinMethod == LockRequestMethod.LockRequest) {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
        } else if (lockinMethod == LockRequestMethod.LockRequestWithSig) {
            boundlessMarket.lockRequestWithSignature(
                request, clientSignature, testProver.signLockRequest(LockRequest({request: request}))
            );
        }

        (FulfillmentBatch memory batch, bytes32 root) =
            createFills(_asArray(request), _asArray(APP_JOURNAL), testProverAddress);

        bytes memory seal =
            verifier.mockProve(
            SET_BUILDER_IMAGE_ID, sha256(abi.encodePacked(SET_BUILDER_IMAGE_ID, uint256(1 << 255), root))
        )
        .seal;

        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        if (lockinMethod == LockRequestMethod.None) {
            boundlessMarket.submitRootAndPriceAndFulfill(
                address(setVerifier),
                root,
                seal,
                _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
                _asArray(batch)
            );
        } else {
            boundlessMarket.submitRootAndFulfill(address(setVerifier), root, seal, _asArray(batch));
        }
        if (!_stringEquals(snapshot, "")) {
            vm.snapshotGasLastCall(snapshot);
        }

        // Check that the proof was submitted
        expectRequestFulfilled(request.id);

        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();

        return (client, request);
    }

    // Base for submitRootAndFulfillAndWithdraw tests with different methods for lock, including none. All paths should yield the same result.
    function _testSubmitRootAndFulfillAndWithdrawSameBlock(
        uint32 requestIdx,
        LockRequestMethod lockinMethod,
        string memory snapshot
    ) private returns (Client, ProofRequest memory) {
        Client client = getClient(1);
        ProofRequest memory request = client.request(requestIdx);
        bytes memory clientSignature = client.sign(request);

        client.snapshotBalance();
        testProver.snapshotBalance();

        if (lockinMethod == LockRequestMethod.LockRequest) {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, clientSignature);
        } else if (lockinMethod == LockRequestMethod.LockRequestWithSig) {
            boundlessMarket.lockRequestWithSignature(
                request, clientSignature, testProver.signLockRequest(LockRequest({request: request}))
            );
        }

        (FulfillmentBatch memory batch, bytes32 root) =
            createFills(_asArray(request), _asArray(APP_JOURNAL), testProverAddress);

        bytes memory seal =
            verifier.mockProve(
            SET_BUILDER_IMAGE_ID, sha256(abi.encodePacked(SET_BUILDER_IMAGE_ID, uint256(1 << 255), root))
        )
        .seal;

        uint256 initialBalance = boundlessMarket.balanceOf(testProverAddress) + testProverAddress.balance;

        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        if (lockinMethod == LockRequestMethod.None) {
            boundlessMarket.submitRootAndPriceAndFulfillAndWithdraw(
                address(setVerifier),
                root,
                seal,
                _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
                _asArray(batch)
            );
        } else {
            boundlessMarket.submitRootAndFulfillAndWithdraw(address(setVerifier), root, seal, _asArray(batch));
        }
        if (!_stringEquals(snapshot, "")) {
            vm.snapshotGasLastCall(snapshot);
        }

        // Check that the proof was submitted
        expectRequestFulfilled(request.id);

        client.expectBalanceChange(-1 ether);
        assert(boundlessMarket.balanceOf(testProverAddress) == 0);
        assert(testProverAddress.balance == initialBalance + 1 ether);

        return (client, request);
    }

    function testFulfillLockedRequest() public {
        _testFulfillSameBlock(1, LockRequestMethod.LockRequest, "fulfill: a locked request");
    }

    function testFulfillAndWithdrawLockedRequest() public {
        _testFulfillAndWithdrawSameBlock(1, LockRequestMethod.LockRequest, "fulfillAndWithdraw: a locked request");
    }

    function testFulfillLockedRequestWithSig() public {
        _testFulfillSameBlock(
            1, LockRequestMethod.LockRequestWithSig, "fulfill: a locked request (locked via prover signature)"
        );
    }

    function testSubmitRootAndFulfillLockedRequest() public {
        _testSubmitRootAndFulfillSameBlock(1, LockRequestMethod.LockRequest, "submitRootAndFulfill: a locked request");
    }

    function testSubmitRootAndFulfillAndWithdrawLockedRequest() public {
        _testSubmitRootAndFulfillAndWithdrawSameBlock(
            1, LockRequestMethod.LockRequest, "submitRootAndFulfillAndWithdraw: a locked request"
        );
    }

    function testSubmitRootAndFulfillLockedRequestWithSig() public {
        _testSubmitRootAndFulfillSameBlock(
            1,
            LockRequestMethod.LockRequestWithSig,
            "submitRootAndFulfill: a locked request (locked via prover signature)"
        );
    }

    // Check that a single client can create many requests, with the full range of indices, and
    // complete the flow each time.
    function testFulfillLockedRequestRangeOfRequestIdx() public {
        for (uint32 idx = 0; idx < 512; idx++) {
            _testFulfillSameBlock(idx, LockRequestMethod.LockRequest);
        }
        _testFulfillSameBlock(0xdeadbeef, LockRequestMethod.LockRequest);
        _testFulfillSameBlock(0xffffffff, LockRequestMethod.LockRequest);
    }

    function testFulfillLargeJournal() external {
        // Generate a 10kB buffer full of non-zero bytes.
        // 10kB = 320 bytes32 values (10240/32)
        bytes32[] memory buffer32 = new bytes32[](320);
        for (uint256 i = 0; i < buffer32.length; i++) {
            buffer32[i] = bytes32(uint256(0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF));
        }
        bytes memory bigJournal = abi.encodePacked(buffer32);

        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        request.requirements.predicate =
            Predicate({predicateType: PredicateType.DigestMatch, data: abi.encode(sha256(bigJournal))});
        bytes memory clientSignature = client.sign(request);

        client.snapshotBalance();
        testProver.snapshotBalance();

        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        FulfillmentBatch memory batch = createFulfillmentBatch(request, bigJournal, testProverAddress);
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        boundlessMarket.fulfill(_asArray(batch));
        vm.snapshotGasLastCall("fulfill: a locked request with 10kB journal");

        // Check that the proof was submitted
        expectRequestFulfilled(request.id);

        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();
    }

    // While a request is locked, another prover can fulfill it but will not receive a payment.
    function testFulfillLockedRequestByOtherProverNotRequirePayment()
        public
        returns (Client, Client, ProofRequest memory)
    {
        Client client = getClient(1);
        ProofRequest memory request = client.request(3);

        boundlessMarket.lockRequestWithSignature(
            request, client.sign(request), testProver.signLockRequest(LockRequest({request: request}))
        );

        Client otherProver = getProver(2);
        address otherProverAddress = otherProver.addr();
        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, otherProverAddress);

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.PaymentRequirementsFailed(abi.encodeWithSelector(
                IBoundlessMarket.RequestIsLocked.selector, request.id
            ));
        boundlessMarket.fulfill(_asArray(batch));
        vm.snapshotGasLastCall("fulfill: another prover fulfills without payment");

        expectRequestFulfilled(request.id);

        // Provers stake is still on the line.
        testProver.expectCollateralBalanceChange(-int256(uint256(request.offer.lockCollateral)));

        // No payment should have been made, as the other prover filled while the request is still locked.
        otherProver.expectBalanceChange(0);
        otherProver.expectCollateralBalanceChange(0);

        expectMarketBalanceUnchanged();

        return (client, otherProver, request);
    }

    // If a request was fulfilled and payment was already sent, we don't allow it to be fulfilled again.
    function testFulfillLockedRequestAlreadyFulfilledAndPaid() public {
        _testFulfillAlreadyFulfilled(1, LockRequestMethod.LockRequest);
        _testFulfillAlreadyFulfilled(2, LockRequestMethod.LockRequestWithSig);
    }

    // This is the only case where fulfill can be called twice successfully.
    // In some cases, a request can be fulfilled without payment being sent. This test starts with
    // one of those cases and checks that the prover can submit fulfillment again to get payment.
    function testFulfillLockedRequestAlreadyFulfilledByOtherProver() public {
        (, Client otherProver, ProofRequest memory request) = testFulfillLockedRequestByOtherProverNotRequirePayment();
        testProver.snapshotBalance();
        testProver.snapshotCollateralBalance();
        otherProver.snapshotBalance();
        otherProver.snapshotCollateralBalance();

        expectRequestFulfilled(request.id);

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, testProverAddress);
        boundlessMarket.fulfill(_asArray(batch));
        vm.snapshotGasLastCall(
            "fulfill: fulfilled by the locked prover for payment (request already fulfilled by another prover)"
        );

        expectRequestFulfilled(request.id);

        // Prover should now have received back their stake plus payment for the request.
        testProver.expectBalanceChange(1 ether);
        testProver.expectCollateralBalanceChange(1 ether);

        // No payment should have been made to the other prover that filled while the request was locked.
        otherProver.expectBalanceChange(0);
        otherProver.expectCollateralBalanceChange(0);

        expectMarketBalanceUnchanged();
    }

    function testFulfillLockedRequestProverAddressNotMatchAssessorReceipt() public {
        Client client = getClient(1);

        ProofRequest memory request = client.request(3);

        boundlessMarket.lockRequestWithSignature(
            request, client.sign(request), testProver.signLockRequest(LockRequest({request: request}))
        );
        // address(3) is just a standin for some other address.
        address mockOtherProverAddr = address(uint160(3));
        // Broker produces a batch bound to `testProverAddress` — the
        // assessor guest commits that prover into its journal.
        FulfillmentBatch memory batch = createFillAndSubmitRootR0(request, APP_JOURNAL, testProverAddress);

        // Tamper: pass a different `prover` to the market. The on-chain
        // adapter reconstructs the journal with `mockOtherProverAddr`,
        // yielding a different `journalDigest` than the broker's seal
        // committed to — setVerifier's inclusion check reverts with
        // `VerificationFailed`.
        batch.prover = mockOtherProverAddr;
        vm.expectRevert(VerificationFailed.selector);
        boundlessMarket.fulfill(_asArray(batch));

        // Prover should have their original balance less the stake amount.
        testProver.expectCollateralBalanceChange(-int256(uint256(request.offer.lockCollateral)));
        expectMarketBalanceUnchanged();
    }

    // Tests trying to fulfill a request that was locked and has now expired.
    function testFulfillLockedRequestFullyExpired() public returns (Client, ProofRequest memory) {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        client.snapshotBalance();
        testProver.snapshotBalance();

        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);
        // At this point the client should have only been charged the 1 ETH at lock time.
        client.expectBalanceChange(-1 ether);

        // Advance the chain ahead to simulate the request timeout.
        vm.warp(request.offer.deadline() + 1);

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, testProverAddress);

        // Try the priceAndFulfill path.
        bytes[] memory paymentErrors = boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );
        assert(
            keccak256(paymentErrors[0])
                == keccak256(abi.encodeWithSelector(IBoundlessMarket.RequestIsExpired.selector, request.id))
        );
        expectRequestNotFulfilled(request.id);

        // Client is out 1 eth until slash is called.
        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(0 ether);
        testProver.expectCollateralBalanceChange(-1 ether);
        expectMarketBalanceUnchanged();

        // Try the fulfill path as well. Should be the same results.
        paymentErrors = boundlessMarket.fulfill(_asArray(batch));
        assert(
            keccak256(paymentErrors[0])
                == keccak256(abi.encodeWithSelector(IBoundlessMarket.RequestIsExpired.selector, request.id))
        );
        expectRequestNotFulfilled(request.id);

        // Client is out 1 eth until slash is called.
        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(0 ether);
        testProver.expectCollateralBalanceChange(-1 ether);
        expectMarketBalanceUnchanged();

        return (client, request);
    }

    function testFulfillLockedRequestMultipleRequestsSameIndex() public {
        _testFulfillRepeatIndex(LockRequestMethod.LockRequest);
    }

    function testFulfillLockedRequestMultipleRequestsSameIndexWithSig() public {
        _testFulfillRepeatIndex(LockRequestMethod.LockRequestWithSig);
    }

    // Scenario when a prover locks a request, fails to deliver it within the lock expiry,
    // then another prover fulfills a request after the lock has expired,
    // but before the request as a whole has expired.
    function testFulfillWasLockedRequestByOtherProver() public returns (ProofRequest memory, Client, Client, Client) {
        // Create a request with a lock timeout of 50 blocks, and overall timeout of 100.
        Client client = getClient(1);
        ProofRequest memory request = client.request(
            1,
            Offer({
                minPrice: 1 ether,
                maxPrice: 2 ether,
                rampUpStart: uint64(block.timestamp),
                rampUpPeriod: uint32(50),
                lockTimeout: uint32(50),
                timeout: uint32(100),
                lockCollateral: 1 ether
            })
        );
        bytes memory clientSignature = client.sign(request);

        Client locker = getProver(1);
        Client otherProver = getProver(2);

        client.snapshotBalance();
        locker.snapshotBalance();
        otherProver.snapshotBalance();

        address lockerAddress = locker.addr();
        vm.prank(lockerAddress);
        boundlessMarket.lockRequest(request, clientSignature);
        // At this point the client should have only been charged the 1 ETH at lock time.
        client.expectBalanceChange(-1 ether);

        // Advance the chain ahead to simulate the lock timeout.
        vm.warp(request.offer.lockDeadline() + 1);

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, otherProver.addr());
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, otherProver.addr(), expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, otherProver.addr(), _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );

        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );

        // Check that the proof was submitted
        expectRequestFulfilled(request.id);

        // Client's fee should be returned on fulfill.
        client.expectBalanceChange(0 ether);
        locker.expectBalanceChange(0 ether);
        locker.expectCollateralBalanceChange(-1 ether);
        otherProver.expectBalanceChange(0 ether);
        otherProver.expectCollateralBalanceChange(0 ether);
        expectMarketBalanceUnchanged();

        return (request, client, locker, otherProver);
    }

    function testFulfillWasLockedClientWithdrawsBalance() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(
            1,
            Offer({
                minPrice: 1 ether,
                maxPrice: 2 ether,
                rampUpStart: uint64(block.timestamp),
                rampUpPeriod: uint32(50),
                lockTimeout: uint32(50),
                timeout: uint32(100),
                lockCollateral: 1 ether
            })
        );
        bytes memory clientSignature = client.sign(request);

        address clientAddress = client.addr();
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        uint256 balance = boundlessMarket.balanceOf(clientAddress);
        vm.prank(clientAddress);
        boundlessMarket.withdraw(balance);

        client.snapshotBalance();

        // Advance the chain ahead to simulate the lock timeout.
        vm.warp(request.offer.lockDeadline() + 1);

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, testProverAddress);

        // Fulfill should complete successfully.
        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );
        expectRequestFulfilled(request.id);

        // Client should get back 1 eth upon fulfill.
        client.expectBalanceChange(1 ether);
        testProver.expectBalanceChange(0 ether);
        testProver.expectCollateralBalanceChange(-1 ether);
    }

    // Scenario when a prover locks a request, fails to deliver it within the lock expiry,
    // but does deliver it before the request expires. Here they should lose their stake,
    // but receive payment for the request.
    function testFulfillWasLockedRequestByOriginalLocker() public returns (ProofRequest memory, Client) {
        // Create a request with a lock timeout of 50 blocks, and overall timeout of 100.
        Client client = getClient(1);
        ProofRequest memory request = client.request(
            1,
            Offer({
                minPrice: 1 ether,
                maxPrice: 2 ether,
                rampUpStart: uint64(block.timestamp),
                rampUpPeriod: uint32(50),
                lockTimeout: uint32(50),
                timeout: uint32(100),
                lockCollateral: 1 ether
            })
        );
        bytes memory clientSignature = client.sign(request);

        Client locker = getProver(1);

        client.snapshotBalance();
        locker.snapshotBalance();

        address lockerAddress = locker.addr();
        vm.prank(lockerAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        // Advance the chain ahead to simulate the lock timeout.
        vm.warp(request.offer.lockDeadline() + 1);

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, locker.addr());
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, lockerAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, lockerAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );

        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );

        // Check that the proof was submitted
        expectRequestFulfilled(request.id);

        client.expectBalanceChange(0 ether);
        locker.expectBalanceChange(0 ether);
        locker.expectCollateralBalanceChange(-1 ether);
        expectMarketBalanceUnchanged();
        return (request, locker);
    }

    // One request is locked, fully expires.
    // A second request with the same id is then fulfilled.
    // Slash should award stake to the fulfiller of the second request.
    function testFulfillWasLockedRequestRepeatIndexStakeRollover() public {
        Client client = getClient(1);

        Offer memory offerA = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            rampUpStart: uint64(block.timestamp),
            rampUpPeriod: uint32(10),
            lockTimeout: uint32(100),
            timeout: uint32(100),
            lockCollateral: 1 ether
        });
        Offer memory offerB = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            rampUpStart: uint64(block.timestamp) + uint64(offerA.timeout) + 1,
            rampUpPeriod: uint32(10),
            lockTimeout: uint32(100),
            timeout: 100,
            lockCollateral: 1 ether
        });

        ProofRequest memory requestA = client.request(1, offerA);
        ProofRequest memory requestB = client.request(1, offerB);
        bytes memory clientSignatureA = client.sign(requestA);
        bytes memory clientSignatureB = client.sign(requestB);
        Client locker = getProver(1);
        Client fulfiller = getProver(2);

        client.snapshotBalance();
        locker.snapshotBalance();
        fulfiller.snapshotBalance();

        // Lock-in request A.
        address lockerAddress = locker.addr();
        vm.prank(lockerAddress);
        boundlessMarket.lockRequest(requestA, clientSignatureA);

        vm.warp(uint64(block.timestamp) + uint64(offerA.timeout) + 1);
        // Attempt to fill request B.
        FulfillmentBatch memory batch = createFulfillmentBatch(requestB, APP_JOURNAL, fulfiller.addr());

        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(requestB), signatures: _asArray(clientSignatureB)})),
            _asArray(batch)
        );

        // Check that the request ID is marked as fulfilled.
        expectRequestFulfilled(requestB.id);

        boundlessMarket.slash(requestB.id);

        client.expectBalanceChange(-1 ether);
        locker.expectBalanceChange(0 ether);
        locker.expectCollateralBalanceChange(-1 ether);
        fulfiller.expectBalanceChange(1 ether);
        fulfiller.expectCollateralBalanceChange(uint256(expectedSlashTransferAmount(offerA.lockCollateral)).toInt256());
        expectMarketBalanceUnchanged();
    }

    // One request is locked, the lock expires, but the request is not yet expired.
    // A second request with the same id is then fulfilled.
    // Slash should award stake to the fulfiller of the second request.
    function testFulfillWasLockedRequestRepeatIndexStakeRolloverFirstRequestNotExpired() public {
        Client client = getClient(1);

        Offer memory offerA = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            rampUpStart: uint64(block.timestamp),
            rampUpPeriod: uint32(10),
            lockTimeout: uint32(50),
            timeout: uint32(100),
            lockCollateral: 1 ether
        });
        Offer memory offerB = Offer({
            minPrice: 2 ether,
            maxPrice: 2 ether,
            rampUpStart: uint64(block.timestamp),
            rampUpPeriod: uint32(0),
            lockTimeout: offerA.timeout + 101,
            timeout: offerA.timeout + 101,
            lockCollateral: 1 ether
        });

        ProofRequest memory requestA = client.request(1, offerA);
        ProofRequest memory requestB = client.request(1, offerB);
        bytes memory clientSignatureA = client.sign(requestA);
        bytes memory clientSignatureB = client.sign(requestB);
        Client locker = getProver(1);
        Client fulfiller = getProver(2);

        client.snapshotBalance();
        locker.snapshotBalance();
        fulfiller.snapshotBalance();

        // Lock-in request A.
        address lockerAddress = locker.addr();
        vm.prank(lockerAddress);
        boundlessMarket.lockRequest(requestA, clientSignatureA);

        vm.warp(offerA.lockDeadline() + 1);
        // Attempt to fill request B.
        FulfillmentBatch memory batch = createFulfillmentBatch(requestB, APP_JOURNAL, fulfiller.addr());

        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(requestB), signatures: _asArray(clientSignatureB)})),
            _asArray(batch)
        );

        // Check that the request ID is marked as fulfilled.
        expectRequestFulfilled(requestB.id);

        // Slash should revert as the original locked request has not yet fully expired.
        vm.expectRevert(
            abi.encodeWithSelector(
                IBoundlessMarket.RequestIsNotExpired.selector,
                requestB.id,
                uint64(block.timestamp) + uint64(offerA.timeout)
            )
        );
        boundlessMarket.slash(requestB.id);

        // Advance to where the original locked request has fully expired.
        vm.warp(uint64(block.timestamp) + uint64(offerA.timeout) + 1);

        vm.prank(lockerAddress);
        boundlessMarket.slash(requestB.id);

        client.expectBalanceChange(-2 ether);
        locker.expectBalanceChange(0 ether);
        locker.expectCollateralBalanceChange(-1 ether);
        fulfiller.expectBalanceChange(2 ether);
        fulfiller.expectCollateralBalanceChange(uint256(expectedSlashTransferAmount(offerA.lockCollateral)).toInt256());
        expectMarketBalanceUnchanged();
    }

    // One request is locked and the client is charged 2 ether. The request expires unfulfilled.
    // A second request with the same id is then fulfilled for a cost of just 1 ether.
    // The client should be refunded the difference.
    function testFulfillWasLockedRequestRepeatIndexSecondRequestCheaper() public {
        Client client = getClient(1);

        // Create two distinct requests with the same ID. It should be the case that only one can be
        // filled, and if one is locked, the other cannot be filled.
        Offer memory offerA = Offer({
            minPrice: 2 ether,
            maxPrice: 3 ether,
            rampUpStart: uint64(block.timestamp),
            rampUpPeriod: uint32(10),
            lockTimeout: uint32(50),
            timeout: uint32(100),
            lockCollateral: 1 ether
        });
        Offer memory offerB = Offer({
            minPrice: 1 ether,
            maxPrice: 1 ether,
            rampUpStart: uint64(block.timestamp),
            rampUpPeriod: uint32(0),
            lockTimeout: uint32(100),
            timeout: uint32(block.timestamp) + offerA.timeout + 101,
            lockCollateral: 1 ether
        });

        ProofRequest memory requestA = client.request(1, offerA);
        ProofRequest memory requestB = client.request(1, offerB);
        bytes memory clientSignatureA = client.sign(requestA);
        bytes memory clientSignatureB = client.sign(requestB);
        Client locker = getProver(1);
        Client fulfiller = getProver(2);

        client.snapshotBalance();
        locker.snapshotBalance();
        fulfiller.snapshotBalance();

        // Lock-in request A.
        address lockerAddress = locker.addr();
        vm.prank(lockerAddress);
        boundlessMarket.lockRequest(requestA, clientSignatureA);

        client.expectBalanceChange(-2 ether);

        vm.warp(offerA.lockDeadline() + 1);

        // Attempt to fill request B, which costs just 1 ether at the time of fulfillment.
        FulfillmentBatch memory batch = createFulfillmentBatch(requestB, APP_JOURNAL, fulfiller.addr());
        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(requestB), signatures: _asArray(clientSignatureB)})),
            _asArray(batch)
        );

        // Client should be refunded 1 ether, meaning their net balance change is -1
        client.expectBalanceChange(-1 ether);

        // Check that the request ID is marked as fulfilled.
        expectRequestFulfilled(requestB.id);

        client.expectBalanceChange(-1 ether);
        locker.expectBalanceChange(0 ether);
        locker.expectCollateralBalanceChange(-1 ether);
        fulfiller.expectBalanceChange(1 ether);
        fulfiller.expectCollateralBalanceChange(0 ether);
        expectMarketBalanceUnchanged();
    }

    // One request is locked, expires, and is slashed.
    // A second request with the same id is then fulfilled.
    function testFulfillWasLockedRequestRepeatIndexStakeRolloverSlashedBeforeFulfill() public {
        Client client = getClient(1);

        // Create two distinct requests with the same ID. It should be the case that only one can be
        // filled, and if one is locked, the other cannot be filled.
        Offer memory offerA = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            rampUpStart: uint64(block.timestamp),
            rampUpPeriod: uint32(10),
            lockTimeout: uint32(100),
            timeout: uint32(100),
            lockCollateral: 1 ether
        });
        Offer memory offerB = Offer({
            minPrice: 3 ether,
            maxPrice: 3 ether,
            rampUpStart: uint64(block.timestamp) + uint64(offerA.timeout) + 1,
            rampUpPeriod: uint32(10),
            lockTimeout: uint32(100),
            timeout: 100,
            lockCollateral: 1 ether
        });

        ProofRequest memory requestA = client.request(1, offerA);
        ProofRequest memory requestB = client.request(1, offerB);
        bytes memory clientSignatureA = client.sign(requestA);
        bytes memory clientSignatureB = client.sign(requestB);
        Client locker = getProver(1);
        Client fulfiller = getProver(2);

        client.snapshotBalance();
        locker.snapshotBalance();
        fulfiller.snapshotBalance();

        // Lock-in request A.
        address lockerAddress = locker.addr();
        vm.prank(lockerAddress);
        boundlessMarket.lockRequest(requestA, clientSignatureA);

        vm.warp(uint64(block.timestamp) + uint64(offerA.timeout) + 1);

        // Slash the request first.
        vm.prank(lockerAddress);
        boundlessMarket.slash(requestA.id);

        // Attempt to fill request B.
        FulfillmentBatch memory batch = createFulfillmentBatch(requestB, APP_JOURNAL, fulfiller.addr());

        address fulfillerAddress = fulfiller.addr();
        vm.prank(fulfillerAddress);
        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(requestB), signatures: _asArray(clientSignatureB)})),
            _asArray(batch)
        );

        // Check that the request ID is marked as fulfilled.
        expectRequestFulfilledAndSlashed(requestB.id);

        client.expectBalanceChange(-3 ether);
        locker.expectBalanceChange(0 ether);
        locker.expectCollateralBalanceChange(-1 ether);
        fulfiller.expectBalanceChange(3 ether);
        fulfiller.expectCollateralBalanceChange(0 ether);
    }

    // Scenario when a prover locks a request, fails to deliver it within the lock expiry,
    // but does deliver it before the request expires. Here they should lose most of their stake
    // (not all), and receive no payment from the client.
    function testFulfillWasLockedRequestDoubleFulfill() public {
        // Create a request with a lock timeout of 50 blocks, and overall timeout of 100.
        Client client = getClient(1);
        ProofRequest memory request = client.request(
            1,
            Offer({
                minPrice: 1 ether,
                maxPrice: 2 ether,
                rampUpStart: uint64(block.timestamp),
                rampUpPeriod: uint32(50),
                lockTimeout: uint32(50),
                timeout: uint32(100),
                lockCollateral: 1 ether
            })
        );
        bytes memory clientSignature = client.sign(request);

        Client locker = getProver(1);
        address lockerAddress = locker.addr();

        client.snapshotBalance();
        locker.snapshotBalance();

        vm.prank(lockerAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        // Advance the chain ahead to simulate the lock timeout.
        vm.warp(request.offer.lockDeadline() + 1);

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, lockerAddress);
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );

        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.PaymentRequirementsFailed(abi.encodeWithSelector(
                IBoundlessMarket.RequestIsFulfilled.selector, request.id
            ));
        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );
        vm.snapshotGasLastCall("priceAndFulfill: fulfill already fulfilled was locked request");

        // Check that the proof was submitted
        expectRequestFulfilled(request.id);

        // Check balances after the fulfillment but before slash.
        client.expectBalanceChange(0 ether);
        locker.expectBalanceChange(0 ether);
        locker.expectCollateralBalanceChange(-1 ether);

        vm.warp(request.offer.deadline() + 1);
        boundlessMarket.slash(request.id);

        // Check balances after the slash.
        client.expectBalanceChange(0 ether);
        locker.expectBalanceChange(0 ether);
        locker.expectCollateralBalanceChange(-int256(uint256(expectedSlashBurnAmount(request.offer.lockCollateral))));
    }

    // Scenario when a prover locks a request, fails to deliver it within the lock expiry,
    // another prover fulfills the request, and then the locker tries to fulfill the request
    // before the request as a whole has expired. A proof should still be delivered and no revert
    // should occur, since we support multiple proofs being delivered for a single request. No
    // balance changes should occur.
    function testFulfillWasLockedRequestLockerFulfillAfterAnotherProverFulfill() public {
        (ProofRequest memory request, Client client, Client locker,) = testFulfillWasLockedRequestByOtherProver();

        locker.snapshotBalance();
        locker.snapshotCollateralBalance();

        bytes memory clientSignature = client.sign(request);

        // The locker should have no balance change.
        // Now the locker tries to fulfill the request.
        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, locker.addr());

        // But its already been fulfilled by the other prover.
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.PaymentRequirementsFailed(abi.encodeWithSelector(
                IBoundlessMarket.RequestIsFulfilled.selector, request.id
            ));

        // The proof should still be delivered.
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id,
            locker.addr(),
            _legacyFill(
                request.id,
                MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest()),
                batch.fills[0]
            )
        );

        // The fulfillment should not revert, as we support multiple proofs being delivered for a single request.
        bytes[] memory paymentErrors = boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );
        assert(
            keccak256(paymentErrors[0])
                == keccak256(abi.encodeWithSelector(IBoundlessMarket.RequestIsFulfilled.selector, request.id))
        );

        // The locker should have no balance change.
        locker.expectBalanceChange(0 ether);
        locker.expectCollateralBalanceChange(0 ether);
        expectMarketBalanceUnchanged();
    }

    // Scenario when a prover locks a request, fails to deliver it within the lock expiry,
    // another prover fulfills the request, and then the locker tries to fulfill the request
    // _after_ the request has fully expired.
    //
    // In this case the request has fully expired, so the proof should NOT be delivered,
    // however we should not revert (as this allows partial fulfillment of other requests in the batch).
    function testFulfillWasLockedRequestLockerFulfillAfterAnotherProverFulfillAndRequestExpired() public {
        (ProofRequest memory request, Client client, Client locker,) = testFulfillWasLockedRequestByOtherProver();

        locker.snapshotBalance();
        locker.snapshotCollateralBalance();

        bytes memory clientSignature = client.sign(request);

        // Advance the chain ahead to simulate the request expiration.
        vm.warp(request.offer.deadline() + 1);

        // The locker should have no balance change.
        // Now the locker tries to fulfill the request.
        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, locker.addr());

        // In this case the request has fully expired, so the proof should NOT be delivered,
        // however we should not revert (as this allows partial fulfillment of other requests in the batch)
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.PaymentRequirementsFailed(abi.encodeWithSelector(
                IBoundlessMarket.RequestIsExpired.selector, request.id
            ));

        // The fulfillment should not revert, as we support multiple proofs being delivered for a single request.
        bytes[] memory paymentErrors = boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );
        assert(
            keccak256(paymentErrors[0])
                == keccak256(abi.encodeWithSelector(IBoundlessMarket.RequestIsExpired.selector, request.id))
        );

        // The locker should have no balance change.
        locker.expectBalanceChange(0 ether);
        locker.expectCollateralBalanceChange(0 ether);
        expectMarketBalanceUnchanged();
    }

    // A request is locked with a valid smart contract signature (signature is checked onchain at lock time)
    // and then a prover tries to fulfill it specifying an invalid smart contract signature. The signature could
    // be invalid for a number of reasons, including the smart contract wallet rotating their signers so the old signature
    // is no longer valid.
    // Since there is possibility of funds being pulled in the multiple request same id case, we ensure we check
    // the SC signature again.
    function testFulfillWasLockedRequestByInvalidSmartContractSignature() public {
        SmartContractClient client = getSmartContractClient(1);
        // Request ID indicates smart contract signature, but the signature is invalid.
        ProofRequest memory request = client.request(
            1,
            Offer({
                minPrice: 1 ether,
                maxPrice: 2 ether,
                rampUpStart: uint64(block.timestamp),
                rampUpPeriod: uint32(50),
                lockTimeout: uint32(50),
                timeout: uint32(100),
                lockCollateral: 1 ether
            })
        );
        bytes memory validClientSignature = client.sign(request);
        bytes memory invalidClientSignature = bytes("invalid");

        boundlessMarket.lockRequestWithSignature(
            request, validClientSignature, testProver.signLockRequest(LockRequest({request: request}))
        );
        vm.warp(request.offer.lockDeadline() + 1);

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, testProverAddress);

        // Fulfill should succeed even though the lock has expired when the request matches what was locked.
        boundlessMarket.fulfill(_asArray(batch));

        // Fulfill should revert during the signature check during pricing, since the signature is invalid.
        // NOTE: This should revert, even though we know the request was signed previously because
        // of signature validation during the lock operation, because the signature in this call is
        // invalid. As a principle, all data in a message must be validated, even if the data given
        // is superfluous.
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InvalidSignature.selector));
        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(invalidClientSignature)})),
            _asArray(batch)
        );

        // Fulfill should succeed if the signature is valid.
        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(validClientSignature)})),
            _asArray(batch)
        );
        expectRequestFulfilled(request.id);

        client.expectBalanceChange(0 ether);
        testProver.expectBalanceChange(0 ether);
        expectMarketBalanceUnchanged();
    }

    function testFulfillNeverLocked() public {
        _testFulfillSameBlock(1, LockRequestMethod.None, "priceAndFulfill: a single request that was not locked");
    }

    /// Fulfill without locking should still work even if the prover does not have stake.
    function testFulfillNeverLockedProverNoStake() public {
        vm.prank(testProverAddress);
        boundlessMarket.withdrawCollateral(DEFAULT_BALANCE);

        _testFulfillSameBlock(
            1,
            LockRequestMethod.None,
            "priceAndFulfill: a single request that was not locked fulfilled by prover not in allow-list"
        );
    }

    function testSubmitRootAndFulfillNeverLocked() public {
        _testSubmitRootAndFulfillSameBlock(
            1, LockRequestMethod.None, "submitRootAndPriceAndFulfill: a single request that was not locked"
        );
    }

    /// SubmitRootAndFulfill without locking should still work even if the prover does not have stake.
    function testSubmitRootAndFulfillNeverLockedProverNoStake() public {
        vm.prank(testProverAddress);
        boundlessMarket.withdrawCollateral(DEFAULT_BALANCE);

        _testSubmitRootAndFulfillSameBlock(
            1,
            LockRequestMethod.None,
            "submitRootAndPriceAndFulfill: a single request that was not locked fulfilled by prover not in allow-list"
        );
    }

    function testFulfillNeverLockedNotPriced() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, testProverAddress);

        // Attempt to fulfill a request without locking or pricing it.
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsNotLockedOrPriced.selector, request.id));
        boundlessMarket.fulfill(_asArray(batch));

        expectMarketBalanceUnchanged();
    }

    /// Every EIP-712-bound field in `SlimRequest` must, when mutated alone,
    /// cause `_verifyBinding` to revert. If any mutation slips through,
    /// `SlimRequestLibrary.reconstructRequestDigest` has drifted from
    /// `ProofRequestLibrary.eip712Digest` — silently letting a prover swap
    /// the field on a client-signed request. The `slim.id` case (last)
    /// additionally pins that the binding check compares against the
    /// stored digest, not just lock existence: a regression checking
    /// `requestLocks[id].requestDigest != 0` would accept an id swap.
    function testFulfillRevertsOnAnyMutatedSlimField() public {
        Client client = getClient(1);
        // Give the request a non-default selector and predicate so the selector
        // and predicate mutations below are real edits. Leave the callback at
        // its zero default — every Callback field is still bound by the
        // typehash, so mutating either field still changes the reconstructed
        // digest. Skipping a real callback addr also keeps the sanity-fulfill
        // below from attempting an external call.
        ProofRequest memory request = client.request(1);
        request.requirements.selector = VERIFIER_ENTRY_SEL;
        request.requirements.predicate = PredicateLibrary.createPrefixMatchPredicate(APP_IMAGE_ID, bytes("prefix"));

        bytes memory clientSignature = client.sign(request);
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        // Second locked request — different id AND different other fields so
        // its stored digest naturally differs from the first request's. Used
        // by the slim.id-swap case (9, below) to exercise the binding check
        // against a real second lock rather than a never-locked id.
        ProofRequest memory secondRequest = client.request(2);
        secondRequest.requirements.selector = VERIFIER_ENTRY_SEL;
        secondRequest.requirements.predicate =
            PredicateLibrary.createPrefixMatchPredicate(APP_IMAGE_ID, bytes("other-prefix"));
        bytes memory secondClientSignature = client.sign(secondRequest);
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(secondRequest, secondClientSignature);

        bytes memory expectedRevert =
            abi.encodeWithSelector(IBoundlessMarket.RequestIsNotLockedOrPriced.selector, request.id);
        FulfillmentBatch memory b;

        // Each iteration clones `request` via ABI roundtrip so the mutation
        // can't leak back through the shared memory pointers Solidity uses
        // for `bytes` / nested-struct fields (a `Predicate memory` literal
        // copies the pointer to `.data`, not the bytes themselves).

        // 1. selector
        b = createFulfillmentBatch(_clone(request), APP_JOURNAL, testProverAddress);
        b.requests[0].selector = bytes4(0xdeadbeef);
        vm.expectRevert(expectedRevert);
        boundlessMarket.fulfill(_asArray(b));

        // 2. callback.addr
        b = createFulfillmentBatch(_clone(request), APP_JOURNAL, testProverAddress);
        b.requests[0].callback.addr = address(0xCAFE);
        vm.expectRevert(expectedRevert);
        boundlessMarket.fulfill(_asArray(b));

        // 3. callback.gasLimit
        b = createFulfillmentBatch(_clone(request), APP_JOURNAL, testProverAddress);
        b.requests[0].callback.gasLimit = b.requests[0].callback.gasLimit + 1;
        vm.expectRevert(expectedRevert);
        boundlessMarket.fulfill(_asArray(b));

        // 4. predicate.predicateType
        b = createFulfillmentBatch(_clone(request), APP_JOURNAL, testProverAddress);
        b.requests[0].predicate.predicateType = PredicateType.DigestMatch; // baseline is PrefixMatch
        vm.expectRevert(expectedRevert);
        boundlessMarket.fulfill(_asArray(b));

        // 5. predicate.data
        b = createFulfillmentBatch(_clone(request), APP_JOURNAL, testProverAddress);
        b.requests[0].predicate.data = abi.encodePacked(b.requests[0].predicate.data, hex"00");
        vm.expectRevert(expectedRevert);
        boundlessMarket.fulfill(_asArray(b));

        // 6. imageUrlHash
        b = createFulfillmentBatch(_clone(request), APP_JOURNAL, testProverAddress);
        b.requests[0].imageUrlHash = b.requests[0].imageUrlHash ^ bytes32(uint256(1));
        vm.expectRevert(expectedRevert);
        boundlessMarket.fulfill(_asArray(b));

        // 7. inputDigest
        b = createFulfillmentBatch(_clone(request), APP_JOURNAL, testProverAddress);
        b.requests[0].inputDigest = b.requests[0].inputDigest ^ bytes32(uint256(1));
        vm.expectRevert(expectedRevert);
        boundlessMarket.fulfill(_asArray(b));

        // 8. offerDigest
        b = createFulfillmentBatch(_clone(request), APP_JOURNAL, testProverAddress);
        b.requests[0].offerDigest = b.requests[0].offerDigest ^ bytes32(uint256(1));
        vm.expectRevert(expectedRevert);
        boundlessMarket.fulfill(_asArray(b));

        // 9. id — swap to another locked request's id. The lookup finds a
        // real lock (non-zero digest), so a regression that only checked
        // existence would silently accept this. The digest reconstructed
        // from request's non-id fields under secondRequest.id doesn't
        // match secondRequest's stored digest (different other fields),
        // so the digest comparison still rejects. The revert id matches
        // the (swapped) slim id, not request.id.
        b = createFulfillmentBatch(_clone(request), APP_JOURNAL, testProverAddress);
        b.requests[0].id = secondRequest.id;
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsNotLockedOrPriced.selector, secondRequest.id));
        boundlessMarket.fulfill(_asArray(b));

        // Sanity: an un-mutated batch fulfills successfully — confirms the
        // slim payload reconstructs to the digest stored at lock time, so
        // the revert-on-mutation assertions above aren't trivially satisfied
        // by some unrelated revert in the baseline fixture.
        FulfillmentBatch memory sanity = createFulfillmentBatch(request, APP_JOURNAL, testProverAddress);
        boundlessMarket.fulfill(_asArray(sanity));
        expectRequestFulfilled(request.id);
    }

    /// @dev Deep-copy a `ProofRequest` via ABI roundtrip. Solidity's
    ///      memory-to-memory struct assignment copies reference-typed fields
    ///      (`bytes`, nested structs containing `bytes`) by pointer, so
    ///      mutating a "copy" silently mutates the original. ABI encode +
    ///      decode forces a fresh allocation for every reference at every
    ///      depth.
    function _clone(ProofRequest memory r) internal pure returns (ProofRequest memory) {
        return abi.decode(abi.encode(r), (ProofRequest));
    }

    // Should revert as you can not fulfill a request twice, except for in the case covered by:
    // `testFulfillLockedRequestAlreadyFulfilledByOtherProver`
    function testFulfillNeverLockedAlreadyFulfilledAndPaid() public {
        _testFulfillAlreadyFulfilled(3, LockRequestMethod.None);
    }

    function testFulfillNeverLockedFullyExpired() public returns (Client, ProofRequest memory) {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, testProverAddress);

        vm.warp(request.offer.deadline() + 1);

        bytes[] memory paymentErrors = boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );
        assert(
            keccak256(paymentErrors[0])
                == keccak256(abi.encodeWithSelector(IBoundlessMarket.RequestIsExpired.selector, request.id))
        );
        expectRequestNotFulfilled(request.id);

        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsNotLockedOrPriced.selector, request.id));
        boundlessMarket.fulfill(_asArray(batch));

        expectRequestNotFulfilled(request.id);
        client.expectBalanceChange(0 ether);
        testProver.expectBalanceChange(0 ether);
        testProver.expectCollateralBalanceChange(0 ether);
        expectMarketBalanceUnchanged();

        return (client, request);
    }

    function testFulfillNeverLockedClientWithdrawsBalance() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);

        address clientAddress = client.addr();

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, testProverAddress);

        uint256 balance = boundlessMarket.balanceOf(clientAddress);
        vm.prank(clientAddress);
        boundlessMarket.withdraw(balance);

        // expect emit of payment requirement failed
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.PaymentRequirementsFailed(abi.encodeWithSelector(
                IBoundlessMarket.InsufficientBalance.selector, clientAddress
            ));
        vm.prank(clientAddress);
        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );
        expectRequestFulfilled(request.id);
    }

    function testFulfillNeverLockedRequestMultipleRequestsSameIndex() public {
        _testFulfillRepeatIndex(LockRequestMethod.None);
    }

    // Fulfill a batch of locked requests
    function testFulfillLockedRequests() public {
        // Provide a batch definition as an array of clients and how many requests each submits.
        uint256[5] memory batchSizes = [uint256(1), 2, 1, 3, 1];
        uint256 batchSize = 0;
        for (uint256 i = 0; i < batchSizes.length; i++) {
            batchSize += batchSizes[i];
        }
        ProofRequest[] memory requests = new ProofRequest[](batchSize);
        bytes[] memory journals = new bytes[](batchSize);
        uint256 expectedRevenue = 0;
        uint256 idx = 0;
        for (uint256 i = 0; i < batchSizes.length; i++) {
            Client client = getClient(i);

            for (uint256 j = 0; j < batchSizes[i]; j++) {
                ProofRequest memory request = client.request(uint32(j));

                // TODO: This is a fragile part of this test. It should be improved.
                uint256 desiredPrice = uint256(1.5 ether);
                vm.warp(request.offer.timeAtPrice(desiredPrice));
                expectedRevenue += desiredPrice;

                boundlessMarket.lockRequestWithSignature(
                    request, client.sign(request), testProver.signLockRequest(LockRequest({request: request}))
                );

                requests[idx] = request;
                journals[idx] = APP_JOURNAL;
                idx++;
            }
        }

        FulfillmentBatch memory batch = createFulfillmentBatch(requests, journals, testProverAddress);

        bytes32 domainSeparator = boundlessMarket.eip712DomainSeparator();
        for (uint256 i = 0; i < batch.fills.length; i++) {
            bytes32 expectedRequestDigest =
                MessageHashUtils.toTypedDataHash(domainSeparator, requests[i].eip712Digest());
            vm.expectEmit(true, true, true, true);
            emit IBoundlessMarket.RequestFulfilled(requests[i].id, testProverAddress, expectedRequestDigest);
            vm.expectEmit(true, true, true, false);
            emit IBoundlessMarket.ProofDelivered(
                requests[i].id, testProverAddress, _legacyFill(requests[i].id, expectedRequestDigest, batch.fills[i])
            );
        }
        boundlessMarket.fulfill(_asArray(batch));
        vm.snapshotGasLastCall(string.concat("fulfill: a batch of ", vm.toString(batchSize)));

        for (uint256 i = 0; i < requests.length; i++) {
            // Check that the proof was submitted
            expectRequestFulfilled(requests[i].id);
        }

        testProver.expectBalanceChange(int256(uint256(expectedRevenue)));
        expectMarketBalanceUnchanged();
    }

    // Fulfill a batch of locked ClaimDigestMatch requests with no journal
    function testFulfillLockedRequestsNoJournal() public {
        // Provide a batch definition as an array of clients and how many requests each submits.
        uint256[5] memory batchSizes = [uint256(1), 2, 1, 3, 1];
        uint256 batchSize = 0;
        for (uint256 i = 0; i < batchSizes.length; i++) {
            batchSize += batchSizes[i];
        }
        ProofRequest[] memory requests = new ProofRequest[](batchSize);
        bytes[] memory journals = new bytes[](batchSize);
        uint256 expectedRevenue = 0;
        uint256 idx = 0;

        for (uint256 i = 0; i < batchSizes.length; i++) {
            Client client = getClient(i);

            for (uint256 j = 0; j < batchSizes[i]; j++) {
                ProofRequest memory request = client.request(uint32(j));
                bytes32 imageId = bytesToBytes32(request.requirements.predicate.data);

                request.requirements.predicate = Predicate({
                    predicateType: PredicateType.ClaimDigestMatch,
                    data: abi.encode(ReceiptClaimLib.ok(imageId, sha256(APP_JOURNAL)).digest())
                });

                // TODO: This is a fragile part of this test. It should be improved.
                uint256 desiredPrice = uint256(1.5 ether);
                vm.warp(request.offer.timeAtPrice(desiredPrice));
                expectedRevenue += desiredPrice;

                boundlessMarket.lockRequestWithSignature(
                    request, client.sign(request), testProver.signLockRequest(LockRequest({request: request}))
                );

                requests[idx] = request;
                journals[idx] = APP_JOURNAL;
                idx++;
            }
        }

        FulfillmentBatch memory batch =
            createFulfillmentBatch(requests, journals, testProverAddress, FulfillmentDataType.None);

        bytes32 domainSeparator = boundlessMarket.eip712DomainSeparator();
        for (uint256 i = 0; i < batch.fills.length; i++) {
            bytes32 expectedRequestDigest =
                MessageHashUtils.toTypedDataHash(domainSeparator, requests[i].eip712Digest());
            vm.expectEmit(true, true, true, true);
            emit IBoundlessMarket.RequestFulfilled(requests[i].id, testProverAddress, expectedRequestDigest);
            vm.expectEmit(true, true, true, false);
            emit IBoundlessMarket.ProofDelivered(
                requests[i].id, testProverAddress, _legacyFill(requests[i].id, expectedRequestDigest, batch.fills[i])
            );
        }
        boundlessMarket.fulfill(_asArray(batch));
        vm.snapshotGasLastCall(string.concat("fulfill (no journal): a batch of ", vm.toString(batchSize)));
        for (uint256 i = 0; i < requests.length; i++) {
            // Check that the proof was submitted
            expectRequestFulfilled(requests[i].id);
        }

        testProver.expectBalanceChange(int256(uint256(expectedRevenue)));
        expectMarketBalanceUnchanged();
    }

    // Testing that reordering fill claim digests + data in a batch (so they
    // no longer line up with the assessor's per-fill leaves) causes the
    // fulfill to revert. Uses the R0 proof-based assessor fixture: the
    // broker's seal commits to the original ordering, so post-fixture
    // tampering desyncs the per-fill seal from the (now-swapped)
    // claim digest and setVerifier rejects.
    function testFulfillShuffleFills() public {
        uint256 batchSize = 2;
        ProofRequest[] memory requests = new ProofRequest[](batchSize);
        bytes[] memory journals = new bytes[](batchSize);

        // First request
        Client client = getClient(0);
        ProofRequest memory request = client.request(uint32(0));
        request.requirements.selector = setVerifier.SELECTOR();
        boundlessMarket.lockRequestWithSignature(
            request, client.sign(request), testProver.signLockRequest(LockRequest({request: request}))
        );
        requests[0] = request;
        journals[0] = APP_JOURNAL;

        // Second request
        client = getClient(1);
        request = client.request(uint32(1));

        request.requirements = Requirements({
            predicate: PredicateLibrary.createDigestMatchPredicate(bytes32(APP_IMAGE_ID_2), sha256(APP_JOURNAL_2)),
            selector: setVerifier.SELECTOR(),
            callback: Callback({addr: address(0), gasLimit: 0})
        });
        boundlessMarket.lockRequestWithSignature(
            request, client.sign(request), testProver.signLockRequest(LockRequest({request: request}))
        );
        requests[1] = request;
        journals[1] = APP_JOURNAL_2;

        FulfillmentBatch memory batch = createFillsAndSubmitRootR0(requests, journals, testProverAddress);

        // Swap fulfillment data + claim digest between fill 0 and fill 1.
        // The per-fill seals stay paired with their original claim digests,
        // so the router's per-fill verifier (setVerifier) reverts because
        // the inclusion proof in `fills[i].seal` no longer matches the
        // tampered `fills[i].claimDigest`.
        bytes memory fulfillmentData0 = batch.fills[0].fulfillmentData;
        bytes32 claimDigest0 = batch.fills[0].claimDigest;

        batch.fills[0].fulfillmentData = batch.fills[1].fulfillmentData;
        batch.fills[1].fulfillmentData = fulfillmentData0;

        batch.fills[0].claimDigest = batch.fills[1].claimDigest;
        batch.fills[1].claimDigest = claimDigest0;

        vm.expectPartialRevert(BoundlessRouter.VerifierFailed.selector);
        boundlessMarket.fulfill(_asArray(batch));

        expectMarketBalanceUnchanged();
    }

    // Test that a smart contract signature can be used to price a request.
    // The smart contract signature must be validated when a request is priced. This
    // ensures that the smart contract signature is checked in the never locked path,
    // since the signature is not checked at lock time (nor in the assessor).
    function testPriceRequestSmartContractSignature() external {
        SmartContractClient client = getSmartContractClient(1);
        ProofRequest memory request = client.request(3);
        bytes memory clientSignature = client.sign(request);

        // Expect isValidSignature to be called on the smart contract wallet
        bytes32 requestHash =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());
        vm.expectCall(
            client.addr(), abi.encodeWithSelector(IERC1271.isValidSignature.selector, requestHash, clientSignature)
        );
        boundlessMarket.priceRequest(request, clientSignature);
    }

    function testPriceRequestSmartContractSignatureExceedsGasLimit() external {
        SmartContractClient client = getSmartContractClient(1);
        client.smartWallet().setGasCost(boundlessMarket.ERC1271_MAX_GAS_FOR_CHECK() + 1);
        ProofRequest memory request = client.request(3);
        bytes memory clientSignature = client.sign(request);

        // Expect isValidSignature to be called on the smart contract wallet
        bytes32 requestHash =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());
        vm.expectCall(
            client.addr(), abi.encodeWithSelector(IERC1271.isValidSignature.selector, requestHash, clientSignature)
        );
        vm.expectRevert(bytes("")); // revert due to out of gas results in empty error
        boundlessMarket.priceRequest(request, clientSignature);
    }

    // Test that a smart contract signature can be used to price and fulfill a request.
    // The smart contract signature must be validated when a request is priced. This
    // ensures that the smart contract signature is validated during the never locked path,
    // since the signature is not checked at lock time (nor in the assessor).
    function testPriceAndFulfillSmartContractSignature() external {
        SmartContractClient client = getSmartContractClient(1);
        ProofRequest memory request = client.request(3);
        bytes memory clientSignature = client.sign(request);

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, testProverAddress);
        bytes32 requestHash =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, requestHash);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, requestHash, batch.fills[0])
        );
        // Expect isValidSignature to be called on the smart contract wallet
        vm.expectCall(
            client.addr(), abi.encodeWithSelector(IERC1271.isValidSignature.selector, requestHash, clientSignature)
        );

        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );
        vm.snapshotGasLastCall("priceAndFulfill: a single request (smart contract signature)");

        expectRequestFulfilled(request.id);

        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();
    }

    // Fulfill a batch of locked requests and withdraw
    function testFulfillAndWithdrawLockedRequests() public {
        // Provide a batch definition as an array of clients and how many requests each submits.
        uint256[5] memory batchSizes = [uint256(1), 2, 1, 3, 1];
        uint256 batchSize = 0;
        for (uint256 i = 0; i < batchSizes.length; i++) {
            batchSize += batchSizes[i];
        }

        ProofRequest[] memory requests = new ProofRequest[](batchSize);
        bytes[] memory journals = new bytes[](batchSize);
        uint256 expectedRevenue = 0;
        uint256 idx = 0;
        for (uint256 i = 0; i < batchSizes.length; i++) {
            Client client = getClient(i);

            for (uint256 j = 0; j < batchSizes[i]; j++) {
                ProofRequest memory request = client.request(uint32(j));

                // TODO: This is a fragile part of this test. It should be improved.
                uint256 desiredPrice = uint256(1.5 ether);
                vm.warp(request.offer.timeAtPrice(desiredPrice));
                expectedRevenue += desiredPrice;

                boundlessMarket.lockRequestWithSignature(
                    request, client.sign(request), testProver.signLockRequest(LockRequest({request: request}))
                );

                requests[idx] = request;
                journals[idx] = APP_JOURNAL;
                idx++;
            }
        }

        FulfillmentBatch memory batch = createFulfillmentBatch(requests, journals, testProverAddress);

        uint256 initialBalance = testProverAddress.balance + boundlessMarket.balanceOf(testProverAddress);

        bytes32 domainSeparator = boundlessMarket.eip712DomainSeparator();
        for (uint256 i = 0; i < batch.fills.length; i++) {
            bytes32 expectedRequestDigest =
                MessageHashUtils.toTypedDataHash(domainSeparator, requests[i].eip712Digest());
            vm.expectEmit(true, true, true, true);
            emit IBoundlessMarket.RequestFulfilled(requests[i].id, testProverAddress, expectedRequestDigest);
            vm.expectEmit(true, true, true, false);
            emit IBoundlessMarket.ProofDelivered(
                requests[i].id, testProverAddress, _legacyFill(requests[i].id, expectedRequestDigest, batch.fills[i])
            );
        }
        boundlessMarket.fulfillAndWithdraw(_asArray(batch));
        vm.snapshotGasLastCall(string.concat("fulfillAndWithdraw: a batch of ", vm.toString(batchSize)));

        for (uint256 i = 0; i < requests.length; i++) {
            // Check that the proof was submitted
            expectRequestFulfilled(requests[i].id);
        }

        assert(boundlessMarket.balanceOf(testProverAddress) == 0);
        assert(testProverAddress.balance == initialBalance + uint256(expectedRevenue));
    }

    function testPriceAndFulfillLockedRequest() external {
        Client client = getClient(1);
        ProofRequest memory request = client.request(3);
        bytes memory clientSignature = client.sign(request);

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, testProverAddress);
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );
        vm.snapshotGasLastCall("priceAndFulfill: a single request");

        expectRequestFulfilled(request.id);

        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();
    }

    function testSubmitRootAndPriceAndFulfillLockedRequest() external {
        Client client = getClient(1);
        ProofRequest memory request = client.request(3);
        bytes memory clientSignature = client.sign(request);

        (FulfillmentBatch memory batch, bytes32 root) =
            createFills(_asArray(request), _asArray(APP_JOURNAL), testProverAddress);

        bytes memory seal =
            verifier.mockProve(
            SET_BUILDER_IMAGE_ID, sha256(abi.encodePacked(SET_BUILDER_IMAGE_ID, uint256(1 << 255), root))
        )
        .seal;

        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        boundlessMarket.submitRootAndPriceAndFulfill(
            address(setVerifier),
            root,
            seal,
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );
        vm.snapshotGasLastCall("submitRootAndPriceAndFulfill: a single request");

        expectRequestFulfilled(request.id);

        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();
    }

    function _testFulfillAlreadyFulfilled(uint32 idx, LockRequestMethod lockinMethod) private {
        (Client client, ProofRequest memory request) = _testFulfillSameBlock(idx, lockinMethod);

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, testProverAddress);

        // TODO(#704): Workaround in test for edge case described in #704
        vm.warp(request.offer.lockDeadline() + 1);

        // Attempt to fulfill a request already fulfilled
        // should return "RequestIsFulfilled({requestId: request.id})"
        bytes[] memory paymentError = boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(client.sign(request))})),
            _asArray(batch)
        );
        assert(
            keccak256(paymentError[0])
                == keccak256(abi.encodeWithSelector(IBoundlessMarket.RequestIsFulfilled.selector, request.id))
        );

        expectMarketBalanceUnchanged();
    }

    function testPriceAndFulfillWithSelector() external {
        Client client = getClient(1);
        ProofRequest memory request = client.request(3);
        request.requirements.selector = setVerifier.SELECTOR();
        bytes memory clientSignature = client.sign(request);

        // Broker fixture: per-fill seal is a setVerifier inclusion proof
        // and the assessor seal is a STARK proof over the assessor journal
        // — fulfill exercises both adapters end-to-end.
        FulfillmentBatch memory batch = createFillAndSubmitRootR0(request, APP_JOURNAL, testProverAddress);
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );
        vm.snapshotGasLastCall("priceAndFulfill: a single request (with selector)");

        expectRequestFulfilled(request.id);

        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();
    }

    function _testFulfillRepeatIndex(LockRequestMethod lockinMethod) private {
        Client client = getClient(1);

        // Create two distinct requests with the same ID. It should be the case that only one can be
        // filled, and if one is locked, the other cannot be filled.
        Offer memory offerA = client.defaultOffer();
        Offer memory offerB = client.defaultOffer();
        offerB.maxPrice = 3 ether;
        ProofRequest memory requestA = client.request(1, offerA);
        ProofRequest memory requestB = client.request(1, offerB);
        bytes memory clientSignatureA = client.sign(requestA);

        // Lock-in request A.
        if (lockinMethod == LockRequestMethod.LockRequest) {
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(requestA, clientSignatureA);
        } else if (lockinMethod == LockRequestMethod.LockRequestWithSig) {
            boundlessMarket.lockRequestWithSignature(
                requestA, clientSignatureA, testProver.signLockRequest(LockRequest({request: requestA}))
            );
        }

        client.snapshotBalance();
        testProver.snapshotBalance();

        // Attempt to fill request B.
        FulfillmentBatch memory batchB = createFulfillmentBatch(requestB, APP_JOURNAL, testProverAddress);

        if (lockinMethod == LockRequestMethod.None) {
            // Here we price with request A and try to fill with request B.
            vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsNotLockedOrPriced.selector, requestA.id));
            boundlessMarket.priceAndFulfill(
                _asArray(ProofRequestBatch({requests: _asArray(requestA), signatures: _asArray(clientSignatureA)})),
                _asArray(batchB)
            );

            expectRequestNotFulfilled(requestB.id);
        } else {
            // Attempting to fulfill request B should revert, since it has never been seen onchain.
            vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsNotLockedOrPriced.selector, requestA.id));
            boundlessMarket.fulfill(_asArray(batchB));
            expectRequestNotFulfilled(requestB.id);

            // Attempting to price and fulfill with request B should return a
            // payment error since request A is still locked.
            bytes[] memory paymentErrors = boundlessMarket.priceAndFulfill(
                _asArray(
                    ProofRequestBatch({requests: _asArray(requestB), signatures: _asArray(client.sign(requestB))})
                ),
                _asArray(batchB)
            );
            assert(
                keccak256(paymentErrors[0])
                    == keccak256(abi.encodeWithSelector(IBoundlessMarket.RequestIsLocked.selector, requestB.id))
            );
            expectRequestFulfilled(requestB.id);
        }

        // No balance changes should have occurred after lockin.
        client.expectBalanceChange(0 ether);
        testProver.expectBalanceChange(0 ether);
        expectMarketBalanceUnchanged();
    }

    /// @dev Pins the contract that one bad fill in any FulfillmentBatch
    ///      reverts the entire `fulfill()` tx. The router's per-fill
    ///      try/catch translates the adapter's revert into
    ///      `VerifierFailed(i, sel)` but does NOT isolate sibling batches in
    ///      the same tx — the market driver does not wrap `ROUTER.verifyBatch`
    ///      in try/catch. If this contract ever changes (e.g. by wrapping verifyBatch in the market
    ///      to settle sibling batches independently), this test must be
    ///      updated or deleted to reflect the new behavior.
    function testFulfillMultiBatchRevertingFillRevertsWholeTx() public {
        // Register a `RevertingVerifier` under the existing verifier class at
        // a fresh selector. The router treats it as a fully-conformant entry
        // (ERC-165 supportsInterface returns true) until it's actually called,
        // at which point it reverts with `Boom()`.
        bytes4 revertingSel = 0x00000012;
        address revertingImpl = address(new RevertingVerifier());
        vm.prank(ownerWallet.addr);
        router.instantiate(revertingSel, revertingImpl, VERIFIER_CLASS_ID, 0);

        // Two distinct clients, two locked requests — distinct state per
        // batch, so the only way batch B can fail is if the verifyBatch
        // failure on batch A propagates out.
        Client clientA = getClient(1);
        Client clientB = getClient(2);
        ProofRequest memory requestA = clientA.request(1);
        ProofRequest memory requestB = clientB.request(2);
        // Precompute signatures so the `vm.prank` below isn't consumed by
        // `Client.sign`'s external call before the actual `lockRequest`.
        bytes memory sigA = clientA.sign(requestA);
        bytes memory sigB = clientB.sign(requestB);
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(requestA, sigA);
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(requestB, sigB);

        clientA.snapshotBalance();
        clientB.snapshotBalance();
        testProver.snapshotBalance();

        // Build the two batches. batchA's per-fill seal selector points to
        // the reverting verifier; batchB stays on the working default
        // (NullVerifier). Both batches share the same default verifier
        // class, so signed `0x00000000` matches both.
        FulfillmentBatch memory batchA = createFulfillmentBatch(requestA, APP_JOURNAL, testProverAddress);
        FulfillmentBatch memory batchB = createFulfillmentBatch(requestB, APP_JOURNAL, testProverAddress);
        batchA.fills[0].seal = abi.encodePacked(revertingSel, hex"deadbeef");

        FulfillmentBatch[] memory batches = new FulfillmentBatch[](2);
        batches[0] = batchA;
        batches[1] = batchB;

        // The per-fill try/catch wraps `RevertingVerifier.Boom()` into a
        // structured `VerifierFailed(0, revertingSel)`. The router's
        // verifyBatch reverts; the market does not wrap that call, so the
        // whole tx reverts.
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.VerifierFailed.selector, uint256(0), revertingSel));
        boundlessMarket.fulfill(batches);

        // Neither request settled — settlement state never advanced past the
        // failing verifyBatch. The tx revert rolls back every state change.
        expectRequestNotFulfilled(requestA.id);
        expectRequestNotFulfilled(requestB.id);
        clientA.expectBalanceChange(0 ether);
        clientB.expectBalanceChange(0 ether);
        testProver.expectBalanceChange(0 ether);
        expectMarketBalanceUnchanged();
    }

    /// @dev Multi-batch fulfill where each batch lands on a different
    ///      adapter pair. Locks three requests, then settles them all in
    ///      a single `fulfill()` call:
    ///        * batch A — NullVerifier + NullAssessor (mock dispatch path).
    ///        * batch B — R0 setVerifier + R0 assessor (production path).
    ///        * batch C — NullJoint (joint-class dispatch; no assessor seam).
    function testFulfillMultiBatchHeterogeneousAdaptersAllSettle() public {
        // Register a joint class + NullJoint entry. Joint dispatch isn't
        // exercised by any production adapter today, so the path needs a
        // mock entry to drive.
        bytes4 jointClassId = 0x00000030;
        bytes4 jointEntrySel = 0x00000031;
        NullJoint nullJoint = new NullJoint();
        vm.startPrank(ownerWallet.addr);
        router.addClass(
            jointClassId,
            BoundlessRouter.ClassMetadata({
                interfaceTag: type(IBoundlessJointVerifierAssessor).interfaceId,
                permissionlessInstantiate: false,
                isDefault: false,
                requiredAssessorClass: bytes4(0),
                schemaArtifact: bytes32(0),
                schemaArtifactUrl: "",
                defaultGasLimit: 100_000,
                label: ""
            })
        );
        router.instantiate(jointEntrySel, address(nullJoint), jointClassId, 0);
        vm.stopPrank();

        Client clientA = getClient(1);
        Client clientB = getClient(2);
        Client clientC = getClient(3);
        ProofRequest memory requestA = clientA.request(1);
        ProofRequest memory requestB = clientB.request(2);
        ProofRequest memory requestC = clientC.request(3);
        // batch C's requestor signs against the joint CLASS id so the
        // signed-selector check matches whichever entry resolves under
        // that class. Signing chain-default (the zero sentinel) would
        // route to the verifier-class default instead.
        requestC.requirements.selector = jointClassId;

        boundlessMarket.lockRequestWithSignature(
            requestA, clientA.sign(requestA), testProver.signLockRequest(LockRequest({request: requestA}))
        );
        boundlessMarket.lockRequestWithSignature(
            requestB, clientB.sign(requestB), testProver.signLockRequest(LockRequest({request: requestB}))
        );
        boundlessMarket.lockRequestWithSignature(
            requestC, clientC.sign(requestC), testProver.signLockRequest(LockRequest({request: requestC}))
        );

        // batch A: NullVerifier + NullAssessor — default seal selectors
        // from `createFulfillmentBatch`.
        FulfillmentBatch memory batchA = createFulfillmentBatch(requestA, APP_JOURNAL, testProverAddress);

        // batch B: production R0 path — set-builder inclusion-proof
        // per-fill seal, R0 assessor seal.
        FulfillmentBatch memory batchB =
            createFillsAndSubmitRootR0(_asArray(requestB), _asArray(APP_JOURNAL), testProverAddress);

        // batch C: joint class. Repoint the per-fill seal to the
        // NullJoint entry and clear the assessor seal — the router
        // enforces `assessorSeal.length == 0` for joint classes.
        FulfillmentBatch memory batchC = createFulfillmentBatch(requestC, APP_JOURNAL, testProverAddress);
        batchC.fills[0].seal = abi.encodePacked(jointEntrySel, hex"deadbeef");
        batchC.assessorSeal = "";

        FulfillmentBatch[] memory batches = new FulfillmentBatch[](3);
        batches[0] = batchA;
        batches[1] = batchB;
        batches[2] = batchC;

        boundlessMarket.fulfill(batches);

        expectRequestFulfilled(requestA.id);
        expectRequestFulfilled(requestB.id);
        expectRequestFulfilled(requestC.id);

        clientA.expectBalanceChange(-1 ether);
        clientB.expectBalanceChange(-1 ether);
        clientC.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(3 ether);
        expectMarketBalanceUnchanged();
    }

    function testSubmitRootAndFulfill() public {
        (ProofRequest[] memory requests, bytes[] memory journals) = newBatch(2);
        (FulfillmentBatch memory batch, bytes32 root) = createFills(requests, journals, testProverAddress);

        bytes memory seal =
            verifier.mockProve(
            SET_BUILDER_IMAGE_ID, sha256(abi.encodePacked(SET_BUILDER_IMAGE_ID, uint256(1 << 255), root))
        )
        .seal;
        boundlessMarket.submitRootAndFulfill(address(setVerifier), root, seal, _asArray(batch));
        vm.snapshotGasLastCall("submitRootAndFulfill: a batch of 2 requests");

        for (uint256 j = 0; j < requests.length; j++) {
            expectRequestFulfilled(requests[j].id);
        }
    }

    function testSlashLockedRequestFullyExpired() public returns (Client, ProofRequest memory) {
        (Client client, ProofRequest memory request) = testFulfillLockedRequestFullyExpired();
        // Provers stake balance is subtracted at lock time, not when slash is called
        testProver.expectCollateralBalanceChange(-uint256(request.offer.lockCollateral).toInt256());

        snapshotMarketCollateralBalance();
        snapshotMarketStakeTreasuryBalance();

        // Slash the request
        // Burning = sending tokens to address 0xdEaD, expect a transfer event to be emitted to address 0xdEaD
        vm.expectEmit(true, true, true, false);
        emit IERC20.Transfer(address(proxy), address(0xdEaD), request.offer.lockCollateral);
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.ProverSlashed(
            request.id,
            expectedSlashBurnAmount(request.offer.lockCollateral),
            expectedSlashTransferAmount(request.offer.lockCollateral),
            address(boundlessMarket)
        );

        boundlessMarket.slash(request.id);
        vm.snapshotGasLastCall("slash: base case");

        expectMarketCollateralBalanceChange(-int256(int96(expectedSlashBurnAmount(request.offer.lockCollateral))));
        expectMarketCollateralTreasuryBalanceChange(
            int256(int96(expectedSlashTransferAmount(request.offer.lockCollateral)))
        );

        client.expectBalanceChange(0 ether);
        testProver.expectCollateralBalanceChange(-uint256(request.offer.lockCollateral).toInt256());

        // Check that the request is slashed and is not fulfilled
        expectRequestSlashed(request.id);

        return (client, request);
    }

    // Prover locks a request, the request expires, then they fulfill a request with the same ID.
    // Prover should be slashable, but still able to fulfill the other request and receive payment for it.
    function testSlashLockedRequestMultipleRequestsSameIndex() public {
        Client client = getClient(1);

        // Create two distinct requests with the same ID.
        Offer memory offerA = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            rampUpStart: uint64(block.timestamp),
            rampUpPeriod: uint32(10),
            lockTimeout: uint32(100),
            timeout: uint32(100),
            lockCollateral: 1 ether
        });
        Offer memory offerB = Offer({
            minPrice: 3 ether,
            maxPrice: 3 ether,
            rampUpStart: uint64(block.timestamp) + uint64(offerA.timeout) + 1,
            rampUpPeriod: uint32(10),
            lockTimeout: uint32(100),
            timeout: 100,
            lockCollateral: 1 ether
        });
        ProofRequest memory requestA = client.request(1, offerA);
        ProofRequest memory requestB = client.request(1, offerB);
        bytes memory clientSignatureA = client.sign(requestA);
        bytes memory clientSignatureB = client.sign(requestB);

        client.snapshotBalance();
        testProver.snapshotBalance();

        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(requestA, clientSignatureA);

        vm.warp(requestA.offer.deadline() + 1);

        // Attempt to fill request B.
        FulfillmentBatch memory batch = createFulfillmentBatch(requestB, APP_JOURNAL, testProverAddress);
        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(requestB), signatures: _asArray(clientSignatureB)})),
            _asArray(batch)
        );

        boundlessMarket.slash(requestA.id);

        expectRequestFulfilledAndSlashed(requestB.id);

        client.expectBalanceChange(-3 ether);
        testProver.expectBalanceChange(3 ether);
        // They lose their original stake, but gain a portion of the slashed stake.
        testProver.expectCollateralBalanceChange(
            -1 ether + int256(uint256(expectedSlashTransferAmount(requestA.offer.lockCollateral)))
        );
        expectMarketBalanceUnchanged();
    }

    // Handles case where a third-party that was not locked fulfills the request, and the locked prover does not.
    // Once the locked prover is slashed, we expect the request to be both "fulfilled" and "slashed".
    // We expect a portion of slashed funds to go to the market treasury.
    function testSlashLockedRequestFulfilledByOtherProverDuringLock() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);

        // Lock to "testProver" but "prover2" fulfills the request
        boundlessMarket.lockRequestWithSignature(
            request, client.sign(request), testProver.signLockRequest(LockRequest({request: request}))
        );

        Client testProver2 = getClient(2);
        (address testProver2Address,,,) = testProver2.wallet();
        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, testProver2Address);

        boundlessMarket.fulfill(_asArray(batch));
        expectRequestFulfilled(request.id);

        vm.warp(request.offer.deadline() + 1);

        // Slash the original prover that locked and didnt deliver
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.ProverSlashed(
            request.id,
            expectedSlashBurnAmount(request.offer.lockCollateral),
            expectedSlashTransferAmount(request.offer.lockCollateral),
            address(boundlessMarket)
        );
        boundlessMarket.slash(request.id);

        client.expectBalanceChange(0 ether);
        testProver.expectCollateralBalanceChange(-uint256(request.offer.lockCollateral).toInt256());
        testProver2.expectCollateralBalanceChange(0 ether);

        // We expect the request is both slashed and fulfilled
        require(boundlessMarket.requestIsSlashed(request.id), "Request should be slashed");
        require(boundlessMarket.requestIsFulfilled(request.id), "Request should be fulfilled");
    }

    function testSlashInvalidRequestID() public {
        // Attempt to slash an invalid request ID
        // should revert with "RequestIsNotLocked({requestId: request.id})"
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsNotLocked.selector, 0xa));
        boundlessMarket.slash(RequestId.wrap(0xa));

        expectMarketBalanceUnchanged();
    }

    function testSlashLockedRequestNotExpired() public {
        (, ProofRequest memory request) = testLockRequest();

        // Attempt to slash a request not expired
        // should revert with "RequestIsNotExpired({requestId: request.id,  deadline: deadline})"
        vm.expectRevert(
            abi.encodeWithSelector(IBoundlessMarket.RequestIsNotExpired.selector, request.id, request.offer.deadline())
        );
        boundlessMarket.slash(request.id);

        expectMarketBalanceUnchanged();
    }

    // Even if the lock has expired, you can not slash until the request is fully expired, as we need to know if the
    // request was eventually fulfilled or not to decide who to send stake to.
    function testSlashWasLockedRequestNotFullyExpired() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(
            1,
            Offer({
                minPrice: 1 ether,
                maxPrice: 2 ether,
                rampUpStart: uint64(block.timestamp),
                rampUpPeriod: uint32(50),
                lockTimeout: uint32(50),
                timeout: uint32(100),
                lockCollateral: 1 ether
            })
        );
        bytes memory clientSignature = client.sign(request);

        Client locker = getProver(1);
        client.snapshotBalance();
        locker.snapshotBalance();

        address lockerAddress = locker.addr();
        vm.prank(lockerAddress);
        boundlessMarket.lockRequest(request, clientSignature);
        // At this point the client should have only been charged the 1 ETH at lock time.
        client.expectBalanceChange(-1 ether);

        // Advance the chain ahead to simulate the lock timeout.
        vm.warp(request.offer.lockDeadline() + 1);

        // Attempt to slash a request not expired
        // should revert with "RequestIsNotExpired({requestId: request.id,  deadline: deadline})"
        vm.expectRevert(
            abi.encodeWithSelector(IBoundlessMarket.RequestIsNotExpired.selector, request.id, request.offer.deadline())
        );
        boundlessMarket.slash(request.id);

        expectMarketBalanceUnchanged();
    }

    function _testSlashFulfilledSameBlock(uint32 idx, LockRequestMethod lockinMethod) private {
        (, ProofRequest memory request) = _testFulfillSameBlock(idx, lockinMethod);

        if (lockinMethod == LockRequestMethod.None) {
            vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsNotLocked.selector, request.id));
        } else {
            vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsFulfilled.selector, request.id));
        }

        boundlessMarket.slash(request.id);

        expectMarketBalanceUnchanged();
    }

    function testSlashLockedRequestFulfilledByLocker() public {
        _testSlashFulfilledSameBlock(1, LockRequestMethod.LockRequest);
        _testSlashFulfilledSameBlock(2, LockRequestMethod.LockRequestWithSig);
    }

    function testSlashNeverLockedRequestFulfilled() public {
        _testSlashFulfilledSameBlock(3, LockRequestMethod.None);
    }

    // Test slashing in the scenario where a request is fulfilled by another prover after the lock expires.
    // but before the request as a whole has expired.
    function testSlashWasLockedRequestFulfilledByOtherProver()
        public
        returns (ProofRequest memory, Client, Client, Client)
    {
        snapshotMarketStakeTreasuryBalance();
        (ProofRequest memory request, Client client, Client locker, Client otherProver) =
            testFulfillWasLockedRequestByOtherProver();
        vm.warp(request.offer.deadline() + 1);
        otherProver.snapshotCollateralBalance();

        // We expect the prover that ultimately fulfilled the request to receive stake.
        // Burning = sending tokens to address 0xdEaD, expect a transfer event to be emitted to address 0xdEaD
        vm.expectEmit(true, true, true, false);
        emit IERC20.Transfer(address(proxy), address(0xdEaD), request.offer.lockCollateral);
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.ProverSlashed(
            request.id,
            expectedSlashBurnAmount(request.offer.lockCollateral),
            expectedSlashTransferAmount(request.offer.lockCollateral),
            otherProver.addr()
        );

        boundlessMarket.slash(request.id);
        vm.snapshotGasLastCall("slash: fulfilled request after lock deadline");

        // Prover should have their original balance less the stake amount.
        testProver.expectCollateralBalanceChange(-uint256(request.offer.lockCollateral).toInt256());
        // Other prover should receive a portion of the stake
        otherProver.expectCollateralBalanceChange(
            uint256(expectedSlashTransferAmount(request.offer.lockCollateral)).toInt256()
        );

        expectMarketCollateralTreasuryBalanceChange(0);
        expectMarketBalanceUnchanged();

        return (request, client, locker, otherProver);
    }

    // In this case the lock expires, the request is fulfilled by another prover, the request is slashed,
    // and then finally the locker tries to fulfill the request.
    //
    // In this case the request has fully expired, so the proof should NOT be delivered,
    // however we should not revert (as this allows partial fulfillment of other requests in the batch).
    function testSlashWasLockedRequestFulfilledByOtherProverFulfillAfterRequestExpired() public {
        (ProofRequest memory request, Client client, Client locker,) = testSlashWasLockedRequestFulfilledByOtherProver();
        vm.warp(request.offer.deadline() + 1);

        bytes memory clientSignature = client.sign(request);

        // Advance the chain ahead to simulate the request expiration.
        vm.warp(request.offer.deadline() + 1);

        // The locker should have no balance change.
        // Now the locker tries to fulfill the request.
        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, locker.addr());

        // In this case the request has fully expired, so the proof should NOT be delivered,
        // however we should not revert (as this allows partial fulfillment of other requests in the batch)
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.PaymentRequirementsFailed(abi.encodeWithSelector(
                IBoundlessMarket.RequestIsExpired.selector, request.id
            ));

        // The fulfillment should not revert, as we support multiple proofs being delivered for a single request.
        bytes[] memory paymentErrors = boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );
        assert(
            keccak256(paymentErrors[0])
                == keccak256(abi.encodeWithSelector(IBoundlessMarket.RequestIsExpired.selector, request.id))
        );
    }

    // Test slashing in the scenario where a request is fulfilled by the locker after the lock expires.
    // but before the request as a whole has expired.
    function testSlashWasLockedRequestFulfilledByLocker() public {
        snapshotMarketStakeTreasuryBalance();
        (ProofRequest memory request, Client prover) = testFulfillWasLockedRequestByOriginalLocker();
        vm.warp(request.offer.deadline() + 1);

        // We expect the prover that ultimately fulfilled the request to receive stake.
        // Burning = sending tokens to address 0xdEaD, expect a transfer event to be emitted to address 0xdEaD
        vm.expectEmit(true, true, true, false);
        emit IERC20.Transfer(address(proxy), address(0xdEaD), request.offer.lockCollateral);
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.ProverSlashed(
            request.id,
            expectedSlashBurnAmount(request.offer.lockCollateral),
            expectedSlashTransferAmount(request.offer.lockCollateral),
            prover.addr()
        );

        boundlessMarket.slash(request.id);

        // Prover should have their original balance less the stake amount plus the stake for eventually filling.
        prover.expectCollateralBalanceChange(
            -uint256(request.offer.lockCollateral).toInt256()
                + uint256(expectedSlashTransferAmount(request.offer.lockCollateral)).toInt256()
        );

        expectMarketCollateralTreasuryBalanceChange(0);
        expectMarketBalanceUnchanged();
    }

    function testSlashSlash() public {
        (, ProofRequest memory request) = testSlashLockedRequestFullyExpired();
        expectRequestSlashed(request.id);

        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsSlashed.selector, request.id));
        boundlessMarket.slash(request.id);
    }

    function testLockRequestSmartContractSignature() public {
        SmartContractClient client = getSmartContractClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSig = client.sign(request);

        // Expect isValidSignature to be called on the smart contract wallet
        bytes32 requestHash =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());
        vm.expectCall(client.addr(), abi.encodeWithSelector(IERC1271.isValidSignature.selector, requestHash, clientSig));

        // Call lockRequest with the smart contract signature
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSig);

        // Verify the lock request
        assertTrue(boundlessMarket.requestIsLocked(request.id), "Request should be locked");
    }

    // Test that the smart contract client receives the proof request when isValidSignature is called,
    // if the client signature provided is empty. This enables custom smart contract clients that want to authorize
    // payments based on how a proof request is structured.
    function testLockRequestSmartContractClientValidatesPassthroughEmptySignature() public {
        SmartContractClient client = getSmartContractClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSig = bytes("");
        client.setExpectedSignature(clientSig);

        // Expect isValidSignature to be called on the smart contract wallet with the proof request as the signature.
        bytes32 requestHash =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());
        vm.expectCall(client.addr(), abi.encodeWithSelector(IERC1271.isValidSignature.selector, requestHash, clientSig));

        // Call lockRequest with the smart contract signature
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSig);

        // Verify the lock request
        assertTrue(boundlessMarket.requestIsLocked(request.id), "Request should be locked");
    }

    function testLockRequestSmartContractSignatureInvalid() public {
        SmartContractClient client = getSmartContractClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSig = bytes("invalid_signature");

        // Expect isValidSignature to be called on the smart contract wallet
        bytes32 requestHash =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());
        vm.expectCall(client.addr(), abi.encodeWithSelector(IERC1271.isValidSignature.selector, requestHash, clientSig));

        // Call lockRequest with the smart contract signature
        vm.prank(testProverAddress);
        vm.expectRevert(IBoundlessMarket.InvalidSignature.selector);
        boundlessMarket.lockRequest(request, clientSig);
    }

    function testLockRequestSmartContractSignatureExceedsGasLimit() public {
        SmartContractClient client = getSmartContractClient(1);
        client.smartWallet().setGasCost(boundlessMarket.ERC1271_MAX_GAS_FOR_CHECK() + 1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSig = client.sign(request);

        // Expect isValidSignature to be called on the smart contract wallet
        bytes32 requestHash =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());
        vm.expectCall(client.addr(), abi.encodeWithSelector(IERC1271.isValidSignature.selector, requestHash, clientSig));

        // Call lockRequest with the smart contract signature
        vm.prank(testProverAddress);
        vm.expectRevert(bytes("")); // revert due to out of gas results in empty error
        boundlessMarket.lockRequest(request, clientSig);
    }

    function testLockRequestWithSignatureClientSmartContractSignatureInvalid() public {
        SmartContractClient client = getSmartContractClient(1);
        Client prover = getClient(2);

        ProofRequest memory request = client.request(1);
        bytes memory clientSig = bytes("invalid_signature");
        bytes memory proverSig = prover.signLockRequest(LockRequest({request: request}));

        address proverAddress = prover.addr();
        vm.prank(proverAddress);
        vm.expectRevert(IBoundlessMarket.InvalidSignature.selector);
        boundlessMarket.lockRequestWithSignature(request, clientSig, proverSig);
    }

    function testFulfillLockedRequestWithCallback() public {
        Client client = getClient(1);

        // Create request with low gas callback
        ProofRequest memory request = client.request(1);
        request.requirements.callback = Callback({addr: address(mockCallback), gasLimit: 500_000});
        request.requirements.selector = setVerifier.SELECTOR();

        bytes memory clientSignature = client.sign(request);
        client.snapshotBalance();
        testProver.snapshotBalance();

        // Lock and fulfill the request
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        FulfillmentBatch memory batch = createFillAndSubmitRoot(request, APP_JOURNAL, testProverAddress);
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        vm.expectEmit(true, true, true, false);
        bytes32 imageId = bytesToBytes32(request.requirements.predicate.data);
        emit MockCallback.MockCallbackCalled(imageId, APP_JOURNAL, batch.fills[0].seal);
        boundlessMarket.fulfill(_asArray(batch));

        // Verify callback was called exactly once
        assertEq(mockCallback.getCallCount(), 1, "Callback should be called exactly once");

        // Verify request state and balances
        expectRequestFulfilled(request.id);
        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();
    }

    function testFulfillLockedRequestWithCallbackNotEnoughGas() public {
        Client client = getClient(1);

        // Create request with low gas callback
        ProofRequest memory request = client.request(1);
        request.requirements.callback = Callback({addr: address(mockCallback), gasLimit: 500_000});

        bytes memory clientSignature = client.sign(request);
        client.snapshotBalance();
        testProver.snapshotBalance();

        // Lock and fulfill the request
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        FulfillmentBatch memory batch = createFulfillmentBatch(request, APP_JOURNAL, testProverAddress);

        vm.expectRevert(IBoundlessMarket.InsufficientGas.selector);
        boundlessMarket.fulfill{gas: 499_000}(_asArray(batch));

        // Verify callback was not called
        assertEq(mockCallback.getCallCount(), 0, "Callback should not be called");

        expectRequestNotFulfilled(request.id);
        expectMarketBalanceUnchanged();
    }

    function testFulfillLockedRequestWithCallbackExceedGasLimit() public {
        Client client = getClient(1);

        // Create request with high gas callback that will exceed limit
        ProofRequest memory request = client.request(1);
        request.requirements.callback = Callback({addr: address(mockHighGasCallback), gasLimit: 10_000});
        request.requirements.selector = setVerifier.SELECTOR();

        bytes memory clientSignature = client.sign(request);
        client.snapshotBalance();
        testProver.snapshotBalance();

        // Lock and fulfill the request
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        FulfillmentBatch memory batch = createFillAndSubmitRoot(request, APP_JOURNAL, testProverAddress);
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.CallbackFailed(request.id, address(mockHighGasCallback), "");
        boundlessMarket.fulfill(_asArray(batch));

        // Verify callback was attempted
        assertEq(mockHighGasCallback.getCallCount(), 0, "Callback not succeed");

        // Verify request state and balances
        expectRequestFulfilled(request.id);
        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();
    }

    function testFulfillLockedRequestWithCallbackByOtherProver() public {
        Client client = getClient(1);

        // Create request with low gas callback
        ProofRequest memory request = client.request(1);
        request.requirements.callback = Callback({addr: address(mockCallback), gasLimit: 100_000});
        request.requirements.selector = setVerifier.SELECTOR();

        bytes memory clientSignature = client.sign(request);

        // Lock request with testProver
        boundlessMarket.lockRequestWithSignature(
            request, clientSignature, testProver.signLockRequest(LockRequest({request: request}))
        );

        // Have otherProver fulfill without requiring payment
        Client otherProver = getProver(2);
        address otherProverAddress = otherProver.addr();
        FulfillmentBatch memory batch = createFillAndSubmitRoot(request, APP_JOURNAL, otherProverAddress);
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, otherProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.PaymentRequirementsFailed(abi.encodeWithSelector(
                IBoundlessMarket.RequestIsLocked.selector, request.id
            ));
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, otherProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        vm.expectEmit(true, true, true, true);
        bytes32 imageId = bytesToBytes32(request.requirements.predicate.data);
        emit MockCallback.MockCallbackCalled(imageId, APP_JOURNAL, batch.fills[0].seal);

        vm.prank(otherProverAddress);
        boundlessMarket.fulfill(_asArray(batch));

        // Verify callback was called exactly once
        assertEq(mockCallback.getCallCount(), 1, "Callback should be called exactly once");

        // Verify request state and balances
        expectRequestFulfilled(request.id);
        testProver.expectCollateralBalanceChange(-int256(uint256(request.offer.lockCollateral)));
        otherProver.expectBalanceChange(0);
        otherProver.expectCollateralBalanceChange(0);
        expectMarketBalanceUnchanged();
    }

    function testFulfillLockedRequestWithCallbackAlreadyFulfilledByOtherProver() public {
        Client client = getClient(1);

        ProofRequest memory request = client.request(1);
        request.requirements.callback = Callback({addr: address(mockCallback), gasLimit: 100_000});
        request.requirements.selector = setVerifier.SELECTOR();

        bytes memory clientSignature = client.sign(request);

        // Lock request with testProver
        boundlessMarket.lockRequestWithSignature(
            request, clientSignature, testProver.signLockRequest(LockRequest({request: request}))
        );

        // Have otherProver fulfill without requiring payment
        Client otherProver = getProver(2);
        address otherProverAddress = address(otherProver);
        FulfillmentBatch memory batch = createFillAndSubmitRoot(request, APP_JOURNAL, otherProverAddress);
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, otherProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.PaymentRequirementsFailed(abi.encodeWithSelector(
                IBoundlessMarket.RequestIsLocked.selector, request.id
            ));
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, otherProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        vm.expectEmit(true, true, true, true);
        bytes32 imageId = bytesToBytes32(request.requirements.predicate.data);
        emit MockCallback.MockCallbackCalled(imageId, APP_JOURNAL, batch.fills[0].seal);
        boundlessMarket.fulfill(_asArray(batch));

        // Verify callback was called exactly once
        assertEq(mockCallback.getCallCount(), 1, "Callback should be called exactly once");

        // Now have original locker fulfill to get payment
        batch = createFillAndSubmitRoot(request, APP_JOURNAL, testProverAddress);
        boundlessMarket.fulfill(_asArray(batch));

        // Verify callback is called again
        assertEq(mockCallback.getCallCount(), 2, "Callback should be called twice");

        expectRequestFulfilled(request.id);
        testProver.expectBalanceChange(1 ether);
        testProver.expectCollateralBalanceChange(0 ether);
        otherProver.expectBalanceChange(0);
        otherProver.expectCollateralBalanceChange(0);
        expectMarketBalanceUnchanged();
    }

    function testFulfillWasLockedRequestWithCallbackByOtherProver() public {
        Client client = getClient(1);

        // Create request with lock timeout of 50 blocks, overall timeout of 100
        ProofRequest memory request = client.request(
            1,
            Offer({
                minPrice: 1 ether,
                maxPrice: 2 ether,
                rampUpStart: uint64(block.timestamp),
                rampUpPeriod: uint32(50),
                lockTimeout: uint32(50),
                timeout: uint32(100),
                lockCollateral: 1 ether
            })
        );
        request.requirements.callback = Callback({addr: address(mockCallback), gasLimit: 100_000});
        request.requirements.selector = setVerifier.SELECTOR();
        bytes memory clientSignature = client.sign(request);

        Client locker = getProver(1);
        Client otherProver = getProver(2);

        client.snapshotBalance();
        locker.snapshotBalance();
        otherProver.snapshotBalance();

        address lockerAddress = locker.addr();
        vm.prank(lockerAddress);
        boundlessMarket.lockRequest(request, clientSignature);
        client.expectBalanceChange(-1 ether);

        // Advance chain ahead to simulate lock timeout
        vm.warp(request.offer.lockDeadline() + 1);

        FulfillmentBatch memory batch = createFillAndSubmitRoot(request, APP_JOURNAL, otherProver.addr());
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, otherProver.addr(), expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, otherProver.addr(), _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        vm.expectEmit(true, true, true, true);
        bytes32 imageId = bytesToBytes32(request.requirements.predicate.data);
        emit MockCallback.MockCallbackCalled(imageId, APP_JOURNAL, batch.fills[0].seal);
        boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(request), signatures: _asArray(clientSignature)})),
            _asArray(batch)
        );

        // Verify callback was called exactly once
        assertEq(mockCallback.getCallCount(), 1, "Callback should be called exactly once");

        // Check request state and balances
        expectRequestFulfilled(request.id);
        client.expectBalanceChange(0 ether);
        locker.expectBalanceChange(0 ether);
        locker.expectCollateralBalanceChange(-1 ether);
        otherProver.expectBalanceChange(0 ether);
        expectMarketBalanceUnchanged();
    }

    function testFulfillWasLockedRequestWithCallbackMultipleRequestsSameIndex() public {
        Client client = getClient(1);

        // Create first request with callback A
        Offer memory offerA = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            rampUpStart: uint64(block.timestamp),
            rampUpPeriod: uint32(10),
            lockTimeout: uint32(100),
            timeout: uint32(100),
            lockCollateral: 1 ether
        });
        ProofRequest memory requestA = client.request(1, offerA);
        requestA.requirements.callback = Callback({addr: address(mockCallback), gasLimit: 10_000});
        requestA.requirements.selector = setVerifier.SELECTOR();
        bytes memory clientSignatureA = client.sign(requestA);

        // Create second request with same ID but different callback
        Offer memory offerB = Offer({
            minPrice: 1 ether,
            maxPrice: 3 ether,
            rampUpStart: offerA.rampUpStart,
            rampUpPeriod: offerA.rampUpPeriod,
            lockTimeout: offerA.lockTimeout + 100,
            timeout: offerA.timeout + 100,
            lockCollateral: offerA.lockCollateral
        });
        ProofRequest memory requestB = client.request(1, offerB);
        requestB.requirements.callback = Callback({addr: address(mockHighGasCallback), gasLimit: 300_000});
        requestB.requirements.selector = setVerifier.SELECTOR();
        bytes memory clientSignatureB = client.sign(requestB);

        client.snapshotBalance();
        testProver.snapshotBalance();

        // Withdraw some funds so we only have funds to cover for the first offer
        // and we have a deficit for the second offer to test the partial payment path
        vm.prank(client.addr());
        boundlessMarket.withdraw(DEFAULT_BALANCE - 2 ether);

        // Lock request A
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(requestA, clientSignatureA);

        // Advance chain ahead to simulate request A lock timeout
        vm.warp(requestA.offer.lockDeadline() + 1);

        FulfillmentBatch memory batch = createFillAndSubmitRoot(requestB, APP_JOURNAL, testProverAddress);
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), requestB.eip712Digest());

        // Since the request being fulfilled is distinct from the one that was locked, the
        // transaction should revert if the request is not priced before fulfillment.
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsNotLockedOrPriced.selector, requestB.id));
        boundlessMarket.fulfill(_asArray(batch));

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(requestB.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            requestB.id, testProverAddress, _legacyFill(requestB.id, expectedRequestDigest, batch.fills[0])
        );
        vm.expectEmit(true, true, true, true);
        bytes32 imageId = bytesToBytes32(requestB.requirements.predicate.data);
        emit MockCallback.MockCallbackCalled(imageId, APP_JOURNAL, batch.fills[0].seal);
        bytes[] memory errors = boundlessMarket.priceAndFulfill(
            _asArray(ProofRequestBatch({requests: _asArray(requestB), signatures: _asArray(clientSignatureB)})),
            _asArray(batch)
        );
        // Verify that the second request was partially payed
        assertEq(errors.length, 1, "Expected one error");
        assertEq(
            errors[0],
            abi.encodeWithSelector(IBoundlessMarket.PartialPayment.selector, 3 ether, 2 ether),
            "Unexpected error"
        );

        // Verify only the second request's callback was called
        assertEq(mockCallback.getCallCount(), 0, "First request's callback should not be called");
        assertEq(mockHighGasCallback.getCallCount(), 1, "Second request's callback should be called once");

        // Deposit back original funds so that the Market original balance is restored
        vm.prank(client.addr());
        boundlessMarket.deposit{value: DEFAULT_BALANCE - 2 ether}();

        // Verify request state and balances
        expectRequestFulfilled(requestB.id);
        client.expectBalanceChange(-2 ether);
        testProver.expectBalanceChange(2 ether);
        testProver.expectCollateralBalanceChange(-1 ether); // Lost stake from lock
        expectMarketBalanceUnchanged();
    }

    function testFulfillLockedRequestClaimDigestWithFulfillmentDataImageIdAndJournal() public {
        Client client = getClient(1);
        bytes32 claimDigest = ReceiptClaimLib.ok(APP_IMAGE_ID, sha256(APP_JOURNAL)).digest();

        // Create request
        ProofRequest memory request = client.request(1);
        request.requirements.predicate = PredicateLibrary.createClaimDigestMatchPredicate(claimDigest);

        bytes memory clientSignature = client.sign(request);
        client.snapshotBalance();
        testProver.snapshotBalance();

        // Lock and fulfill the request
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        FulfillmentBatch memory batch = createFulfillmentBatch(
            _asArray(request), _asArray(APP_JOURNAL), testProverAddress, FulfillmentDataType.ImageIdAndJournal
        );
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        boundlessMarket.fulfill(_asArray(batch));

        // Verify request state and balances
        expectRequestFulfilled(request.id);
        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();
    }

    function testFulfillLockedRequesClaimDigestWithFulfillmentDataNone() public {
        Client client = getClient(1);
        bytes32 claimDigest = ReceiptClaimLib.ok(APP_IMAGE_ID, sha256(APP_JOURNAL)).digest();

        // Create request
        ProofRequest memory request = client.request(1);
        request.requirements.predicate = PredicateLibrary.createClaimDigestMatchPredicate(claimDigest);

        bytes memory clientSignature = client.sign(request);
        client.snapshotBalance();
        testProver.snapshotBalance();

        // Lock and fulfill the request
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        FulfillmentBatch memory batch = createFulfillmentBatch(
            _asArray(request), _asArray(APP_JOURNAL), testProverAddress, FulfillmentDataType.None
        );
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        boundlessMarket.fulfill(_asArray(batch));

        // Verify request state and balances
        expectRequestFulfilled(request.id);
        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();
    }

    // Test that if a callback was requested, but the fulfillment data doesnt have the journal,
    // the fulfillment reverts and the callback is not called.
    function testFulfillLockedRequestWithCallbackAndFulfillmentDataNone() public {
        Client client = getClient(1);
        bytes32 claimDigest = ReceiptClaimLib.ok(APP_IMAGE_ID, sha256(APP_JOURNAL)).digest();

        // Create request with low gas callback
        ProofRequest memory request = client.request(1);
        request.requirements.callback = Callback({addr: address(mockCallback), gasLimit: 500_000});
        request.requirements.predicate = PredicateLibrary.createClaimDigestMatchPredicate(claimDigest);

        bytes memory clientSignature = client.sign(request);
        client.snapshotBalance();
        testProver.snapshotBalance();

        // Lock and fulfill the request
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        FulfillmentBatch memory batch = createFulfillmentBatch(
            _asArray(request), _asArray(APP_JOURNAL), testProverAddress, FulfillmentDataType.None
        );

        vm.expectRevert(IBoundlessMarket.UnfulfillableCallback.selector);
        boundlessMarket.fulfill(_asArray(batch));

        // Verify callback was not called
        assertEq(mockCallback.getCallCount(), 0, "Callback should be called exactly 0 times");

        // Verify request state and balances
        expectRequestNotFulfilled(request.id);
        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(0 ether);
        expectMarketBalanceUnchanged();
    }

    function testFulfillLockedRequestClaimDigestWithCallbackImageIdAndJournal() public {
        Client client = getClient(1);
        bytes32 claimDigest = ReceiptClaimLib.ok(APP_IMAGE_ID, sha256(APP_JOURNAL)).digest();
        // Create request
        ProofRequest memory request = client.request(1);
        request.requirements.callback = Callback({addr: address(mockCallback), gasLimit: 500_000});
        request.requirements.predicate = PredicateLibrary.createClaimDigestMatchPredicate(claimDigest);
        request.requirements.selector = setVerifier.SELECTOR();

        bytes memory clientSignature = client.sign(request);
        client.snapshotBalance();
        testProver.snapshotBalance();

        // Lock and fulfill the request
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        FulfillmentBatch memory batch = createFillsAndSubmitRoot(
            _asArray(request), _asArray(APP_JOURNAL), testProverAddress, FulfillmentDataType.ImageIdAndJournal
        );
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        vm.expectEmit(true, true, true, true);
        emit MockCallback.MockCallbackCalled(APP_IMAGE_ID, APP_JOURNAL, batch.fills[0].seal);

        boundlessMarket.fulfill(_asArray(batch));

        assertEq(mockCallback.getCallCount(), 1, "Callback should be called exactly 1 time");

        // Verify request state and balances
        expectRequestFulfilled(request.id);
        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();
    }
} // <-- closes BoundlessMarketBasicTest

/// @title BoundlessMarketOnChainAssessorTest — e2e through the native assessor.
///
/// @notice Mirrors the canonical fulfill / callback / slash scenarios that
///         `BoundlessMarketBasicTest` exercises through `NullAssessor` /
///         `R0BoundlessAssessorAdapter`, but routes every batch through
///         `OnChainAssessor` instead.
///
///         The point is parity: the market's state transitions, balance
///         changes, and event emissions must be identical regardless of
///         which assessor adapter brokers selected. The on-chain adapter's
///         job is to be drop-in compatible with the guest-based path; these
///         tests pin that.
///
///         All requests here use `setVerifier.SELECTOR()` as the signed
///         verifier selector — that's the production shape (per-fill seals
///         are set-builder inclusion proofs) and also what callback tests
///         require so `BoundlessMarketCallback`'s defense re-verify passes.
contract BoundlessMarketOnChainAssessorTest is BoundlessMarketTest {
    using ReceiptClaimLib for ReceiptClaim;
    using BoundlessMarketLib for ProofRequest;
    using BoundlessMarketLib for Offer;

    /// @dev `Client.wallet` is `Vm.Wallet public` — Solidity's auto-getter
    ///      returns the struct as a tuple, not as a `Vm.Wallet`. Rehydrate
    ///      so the OnChainAssessor fixtures can take a typed wallet.
    function _proverWallet(Client prover) internal view returns (Vm.Wallet memory w) {
        (address a, uint256 x, uint256 y, uint256 p) = prover.wallet();
        w.addr = a;
        w.publicKeyX = x;
        w.publicKeyY = y;
        w.privateKey = p;
    }

    // ─── Happy paths ────────────────────────────────────────────────────

    function testFulfillLockedRequest_OnChainAssessor() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        request.requirements.selector = setVerifier.SELECTOR();
        bytes memory clientSignature = client.sign(request);

        client.snapshotBalance();
        testProver.snapshotBalance();

        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        FulfillmentBatch memory batch = createFillAndSubmitRootOnChain(request, APP_JOURNAL, _proverWallet(testProver));
        bytes32 expectedRequestDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), request.eip712Digest());

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, testProverAddress, expectedRequestDigest);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id, testProverAddress, _legacyFill(request.id, expectedRequestDigest, batch.fills[0])
        );
        boundlessMarket.fulfill(_asArray(batch));

        expectRequestFulfilled(request.id);
        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();
    }

    function testFulfillBatch_OnChainAssessor() public {
        uint256 batchSize = 3;
        ProofRequest[] memory requests = new ProofRequest[](batchSize);
        bytes[] memory journals = new bytes[](batchSize);

        for (uint256 i = 0; i < batchSize; i++) {
            Client client = getClient(i + 1);
            ProofRequest memory request = client.request(uint32(i + 1));
            request.requirements.selector = setVerifier.SELECTOR();
            bytes memory sig = client.sign(request);
            vm.prank(testProverAddress);
            boundlessMarket.lockRequest(request, sig);
            requests[i] = request;
            journals[i] = APP_JOURNAL;
        }

        FulfillmentBatch memory batch = createFillsAndSubmitRootOnChain(requests, journals, _proverWallet(testProver));
        boundlessMarket.fulfill(_asArray(batch));

        for (uint256 i = 0; i < batchSize; i++) {
            expectRequestFulfilled(requests[i].id);
        }
        expectMarketBalanceUnchanged();
    }

    /// @notice The scenario the OnChainAssessor's ClaimDigestMatch
    ///         reconstruction guard exists for: predicate is
    ///         `ClaimDigestMatch`, prover attaches `(imageId, journal)` so
    ///         the callback can act on them, and a malicious prover would
    ///         otherwise be free to attach bytes that don't match the
    ///         proven claim.
    ///
    ///         Happy path: matching (imageId, journal) → fulfill + callback
    ///         both succeed.
    function testFulfillCallback_ClaimDigestMatch_OnChainAssessor_matchingBytes() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes32 imageId = APP_IMAGE_ID;
        bytes32 claimDigest = ReceiptClaimLib.ok(imageId, sha256(APP_JOURNAL)).digest();
        request.requirements.predicate = PredicateLibrary.createClaimDigestMatchPredicate(claimDigest);
        request.requirements.callback = Callback({addr: address(mockCallback), gasLimit: 500_000});
        request.requirements.selector = setVerifier.SELECTOR();
        bytes memory clientSignature = client.sign(request);

        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        FulfillmentBatch memory batch = createFillAndSubmitRootOnChain(request, APP_JOURNAL, _proverWallet(testProver));
        vm.expectEmit(true, true, true, false);
        emit MockCallback.MockCallbackCalled(imageId, APP_JOURNAL, batch.fills[0].seal);
        boundlessMarket.fulfill(_asArray(batch));

        assertEq(mockCallback.getCallCount(), 1);
        expectRequestFulfilled(request.id);
        expectMarketBalanceUnchanged();
    }

    /// @notice The negative half of the above: prover attaches
    ///         `(imageId, journal)` that does NOT reconstruct to the proven
    ///         claim digest. Without the reconstruction guard the callback
    ///         would fire with unproven bytes; with it the adapter reverts
    ///         and the market never dispatches the callback.
    function testFulfillCallback_ClaimDigestMatch_OnChainAssessor_mismatchedBytesReverts() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes32 imageId = APP_IMAGE_ID;
        bytes32 claimDigest = ReceiptClaimLib.ok(imageId, sha256(APP_JOURNAL)).digest();
        request.requirements.predicate = PredicateLibrary.createClaimDigestMatchPredicate(claimDigest);
        request.requirements.callback = Callback({addr: address(mockCallback), gasLimit: 500_000});
        request.requirements.selector = setVerifier.SELECTOR();
        bytes memory clientSignature = client.sign(request);

        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        // Build the batch normally so the fixture computes the right
        // claimDigest and assessor seal, then swap in a non-matching journal
        // before submitting. The lock's stored requestDigest still matches
        // (slim payload is unchanged), but the (imageId, journal) attached
        // to fulfillmentData no longer reconstructs to claimDigest.
        FulfillmentBatch memory batch = createFillAndSubmitRootOnChain(request, APP_JOURNAL, _proverWallet(testProver));
        batch.fills[0].fulfillmentData =
            abi.encode(FulfillmentDataImageIdAndJournal({imageId: imageId, journal: bytes("LIES")}));

        vm.expectRevert(abi.encodeWithSelector(OnChainAssessor.ClaimDigestMismatch.selector, uint256(0)));
        boundlessMarket.fulfill(_asArray(batch));

        // The fulfill reverted — request is still locked, callback never fired.
        assertEq(mockCallback.getCallCount(), 0);
        expectRequestNotFulfilled(request.id);
    }

    // ─── Mixed-adapter batches in one tx ────────────────────────────────

    /// @notice One `fulfill` call carrying two `FulfillmentBatch[]`es of
    ///         different assessor classes — one through `OnChainAssessor`,
    ///         one through `R0BoundlessAssessorAdapter`. The market routes
    ///         each batch independently; both must settle without cross-
    ///         contamination.
    function testFulfillMixedAdapters_OnChainAndR0_inOneTx() public {
        Client clientA = getClient(1);
        Client clientB = getClient(2);

        ProofRequest memory requestA = clientA.request(1);
        requestA.requirements.selector = setVerifier.SELECTOR();
        ProofRequest memory requestB = clientB.request(2);
        requestB.requirements.selector = setVerifier.SELECTOR();

        bytes memory sigA = clientA.sign(requestA);
        bytes memory sigB = clientB.sign(requestB);

        vm.startPrank(testProverAddress);
        boundlessMarket.lockRequest(requestA, sigA);
        boundlessMarket.lockRequest(requestB, sigB);
        vm.stopPrank();

        FulfillmentBatch memory batchOnChain =
            createFillAndSubmitRootOnChain(requestA, APP_JOURNAL, _proverWallet(testProver));
        FulfillmentBatch memory batchR0 = createFillAndSubmitRootR0(requestB, APP_JOURNAL, testProverAddress);

        FulfillmentBatch[] memory batches = new FulfillmentBatch[](2);
        batches[0] = batchOnChain;
        batches[1] = batchR0;
        boundlessMarket.fulfill(batches);

        expectRequestFulfilled(requestA.id);
        expectRequestFulfilled(requestB.id);
        expectMarketBalanceUnchanged();
    }
}

contract BoundlessMarketBench is BoundlessMarketTest {
    using BoundlessMarketLib for Offer;

    // Bench helpers run through the R0 proof-based assessor adapter so the numbers
    // reflect what real users pay end-to-end: the router resolving each fill via
    // the setVerifier plus the `R0BoundlessAssessorAdapter`.

    function benchFulfill(uint256 batchSize, string memory snapshot) public {
        (ProofRequest[] memory requests, bytes[] memory journals) = newBatch(batchSize);
        FulfillmentBatch memory batch = createFillsAndSubmitRootR0(requests, journals, testProverAddress);

        boundlessMarket.fulfill(_asArray(batch));
        vm.snapshotGasLastCall(string.concat("fulfill: batch of ", snapshot));

        for (uint256 j = 0; j < requests.length; j++) {
            expectRequestFulfilled(requests[j].id);
        }
    }

    function benchFulfillWithSelector(uint256 batchSize, string memory snapshot) public {
        (ProofRequest[] memory requests, bytes[] memory journals) =
            newBatchWithSelector(batchSize, setVerifier.SELECTOR());
        FulfillmentBatch memory batch = createFillsAndSubmitRootR0(requests, journals, testProverAddress);

        boundlessMarket.fulfill(_asArray(batch));
        vm.snapshotGasLastCall(string.concat("fulfill (with selector): batch of ", snapshot));

        for (uint256 j = 0; j < requests.length; j++) {
            expectRequestFulfilled(requests[j].id);
        }
    }

    function benchFulfillWithCallback(uint256 batchSize, string memory snapshot) public {
        (ProofRequest[] memory requests, bytes[] memory journals) = newBatchWithCallback(batchSize);
        FulfillmentBatch memory batch = createFillsAndSubmitRootR0(requests, journals, testProverAddress);

        boundlessMarket.fulfill(_asArray(batch));
        vm.snapshotGasLastCall(string.concat("fulfill (with callback): batch of ", snapshot));

        for (uint256 j = 0; j < requests.length; j++) {
            expectRequestFulfilled(requests[j].id);
        }
    }

    function testBenchFulfill001() public {
        benchFulfill(1, "001");
    }

    function testBenchFulfill002() public {
        benchFulfill(2, "002");
    }

    function testBenchFulfill004() public {
        benchFulfill(4, "004");
    }

    function testBenchFulfill008() public {
        benchFulfill(8, "008");
    }

    function testBenchFulfill016() public {
        benchFulfill(16, "016");
    }

    function testBenchFulfill032() public {
        benchFulfill(32, "032");
    }

    function testBenchFulfill064() public {
        benchFulfill(64, "064");
    }

    function testBenchFulfill128() public {
        benchFulfill(128, "128");
    }

    function testBenchFulfillWithSelector001() public {
        benchFulfillWithSelector(1, "001");
    }

    function testBenchFulfillWithSelector002() public {
        benchFulfillWithSelector(2, "002");
    }

    function testBenchFulfillWithSelector004() public {
        benchFulfillWithSelector(4, "004");
    }

    function testBenchFulfillWithSelector008() public {
        benchFulfillWithSelector(8, "008");
    }

    function testBenchFulfillWithSelector016() public {
        benchFulfillWithSelector(16, "016");
    }

    function testBenchFulfillWithSelector032() public {
        benchFulfillWithSelector(32, "032");
    }

    function testBenchFulfillWithCallback001() public {
        benchFulfillWithCallback(1, "001");
    }

    function testBenchFulfillWithCallback002() public {
        benchFulfillWithCallback(2, "002");
    }

    function testBenchFulfillWithCallback004() public {
        benchFulfillWithCallback(4, "004");
    }

    function testBenchFulfillWithCallback008() public {
        benchFulfillWithCallback(8, "008");
    }

    function testBenchFulfillWithCallback016() public {
        benchFulfillWithCallback(16, "016");
    }

    function testBenchFulfillWithCallback032() public {
        benchFulfillWithCallback(32, "032");
    }
}

contract BoundlessMarketUpgradeTest is BoundlessMarketTest {
    using BoundlessMarketLib for Offer;

    function testUnsafeUpgrade() public {
        vm.startPrank(ownerWallet.addr);
        proxy = UnsafeUpgrades.deployUUPSProxy(
            address(new BoundlessMarket(router, address(collateralToken), legacyImpl)),
            abi.encodeCall(BoundlessMarket.initialize, (ownerWallet.addr))
        );
        boundlessMarket = BoundlessMarket(payable(proxy));
        address implAddressV1 = UnsafeUpgrades.getImplementationAddress(proxy);

        // Should emit an `Upgraded` event
        vm.expectEmit(false, true, true, true);
        emit IERC1967.Upgraded(address(0));
        UnsafeUpgrades.upgradeProxy(
            proxy, address(new BoundlessMarket(router, address(collateralToken), legacyImpl)), "", ownerWallet.addr
        );
        vm.stopPrank();
        address implAddressV2 = UnsafeUpgrades.getImplementationAddress(proxy);

        assertFalse(implAddressV2 == implAddressV1);
    }

    function testGrantAdminRole() public {
        address newAdmin = vm.createWallet("NEW_ADMIN").addr;
        bytes32 adminRole = boundlessMarket.ADMIN_ROLE();

        vm.prank(ownerWallet.addr);
        boundlessMarket.grantRole(adminRole, newAdmin);

        assertTrue(boundlessMarket.hasRole(adminRole, newAdmin), "New admin should have admin role");
        assertTrue(boundlessMarket.hasRole(adminRole, ownerWallet.addr), "Original owner should still have admin role");
    }
}
