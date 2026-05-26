// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {Test} from "forge-std/Test.sol";
import {UnsafeUpgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";
import {ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";

import {OnChainAssessor} from "../../src/router/adapters/OnChainAssessor.sol";
import {R0BoundlessAssessorAdapter} from "../../src/router/adapters/R0BoundlessAssessorAdapter.sol";
import {IBoundlessAssessor} from "../../src/router/interfaces/IBoundlessAssessor.sol";
import {IBoundlessVerifier} from "../../src/router/interfaces/IBoundlessVerifier.sol";
import {BoundlessRouter} from "../../src/router/BoundlessRouter.sol";

import {ProofRequest} from "../../src/types/ProofRequest.sol";
import {Requirements} from "../../src/types/Requirements.sol";
import {Callback} from "../../src/types/Callback.sol";
import {Predicate, PredicateType, PredicateLibrary} from "../../src/types/Predicate.sol";
import {Input, InputType, InputLibrary} from "../../src/types/Input.sol";
import {Offer, OfferLibrary} from "../../src/types/Offer.sol";
import {RequestId, RequestIdLibrary} from "../../src/types/RequestId.sol";
import {Fulfillment} from "../../src/types/Fulfillment.sol";
import {FulfillmentBatch} from "../../src/types/FulfillmentBatch.sol";
import {FulfillmentDataType, FulfillmentDataImageIdAndJournal} from "../../src/types/FulfillmentData.sol";
import {SlimRequest, SlimRequestLibrary} from "../../src/types/SlimRequest.sol";

import {NullVerifier, NullAssessor, NullRiscZeroVerifier} from "../mocks/RouterMocks.sol";

// ─── Harnesses ────────────────────────────────────────────────────────────

/// @notice Cross-contract gas-measurement wrapper. Calls the adapter directly
///         (no router). The caller is responsible for any market-side
///         preprocessing (e.g. binding check); the harness itself only times
///         the call so the measured number is the adapter's own cost.
contract DirectHarness {
    IBoundlessAssessor public immutable ADAPTER;

    constructor(IBoundlessAssessor adapter) {
        ADAPTER = adapter;
    }

    function measure(FulfillmentBatch calldata batch, bytes32[] calldata requestDigests)
        external
        view
        returns (uint256 gasUsed)
    {
        uint256 g0 = gasleft();
        ADAPTER.verifyAssessor(batch, requestDigests);
        gasUsed = g0 - gasleft();
    }
}

/// @notice Gas-measurement wrapper that dispatches through the router.
///         Measures the realistic end-to-end cost of a call to the router
///         seam (verifier dispatch + assessor dispatch); does not include
///         any market-side preprocessing.
contract RouterHarness {
    BoundlessRouter public immutable ROUTER;

    constructor(BoundlessRouter router) {
        ROUTER = router;
    }

    function measure(FulfillmentBatch calldata batch, bytes32[] calldata requestDigests)
        external
        view
        returns (uint256 gasUsed)
    {
        uint256 g0 = gasleft();
        ROUTER.verifyBatch(batch, requestDigests);
        gasUsed = g0 - gasleft();
    }
}

/// @notice Two-call router harness. Invokes the router twice in one tx; first
///         call pays cold SLOAD costs on `entries[]` / `classes[]` /
///         `tombstoned[]`, the second is warm. Delta isolates the cold-only
///         portion of router overhead.
contract MultiCallRouterHarness {
    BoundlessRouter public immutable ROUTER;

    constructor(BoundlessRouter router) {
        ROUTER = router;
    }

    function measureColdWarm(FulfillmentBatch calldata batch, bytes32[] calldata requestDigests)
        external
        view
        returns (uint256 coldGas, uint256 warmGas)
    {
        uint256 g0 = gasleft();
        ROUTER.verifyBatch(batch, requestDigests);
        coldGas = g0 - gasleft();

        uint256 g1 = gasleft();
        ROUTER.verifyBatch(batch, requestDigests);
        warmGas = g1 - gasleft();
    }
}

// ─── Bench base ───────────────────────────────────────────────────────────

/// @title BenchBase — shared setup + fixtures for `AdapterBench` and `RouterBench`.
///
/// @notice Stands up a `BoundlessRouter` with three assessor entries
///         (`OnChainAssessor`, `R0BoundlessAssessorAdapter`, `NullAssessor`)
///         under one assessor class, and a single verifier entry (`NullVerifier`)
///         under one verifier class flagged as the chain default. Constructs a
///         prover wallet for ECDSA signing.
///
///         Subclass and use the public fixture builders + harnesses to write
///         benches that target a specific layer of the stack.
abstract contract BenchBase is Test {
    using ReceiptClaimLib for ReceiptClaim;

    // Adapters under test.
    OnChainAssessor internal onChainAssessor;
    R0BoundlessAssessorAdapter internal r0Assessor;
    NullAssessor internal nullAssessor;
    NullVerifier internal verifier;
    NullRiscZeroVerifier internal nullR0;
    BoundlessRouter internal router;

    // Harnesses bound to each adapter (for direct-call paths) + the router.
    DirectHarness internal directOnChain;
    DirectHarness internal directR0;
    DirectHarness internal directNull;
    RouterHarness internal routerHarness;
    MultiCallRouterHarness internal multiCallHarness;

    /// @dev Prover wallet sourced via `vm.makeAddrAndKey` so `vm.addr(pk)` and
    ///      `vm.sign(pk, ...)` agree.
    uint256 internal proverPk;
    address internal proverAddr;
    address internal CLIENT = address(0xA11CE);
    address internal constant ADMIN = address(0xA);

    bytes4 internal constant VERIFIER_CLASS_ID = 0x00000010;
    bytes4 internal constant VERIFIER_ENTRY_SEL = 0x00000011;
    bytes4 internal constant ASSESSOR_CLASS_ID = 0x00000020;
    bytes4 internal constant ASSESSOR_ON_CHAIN_SEL = 0x00000021;
    bytes4 internal constant ASSESSOR_R0_SEL = 0x00000022;
    bytes4 internal constant ASSESSOR_NULL_SEL = 0x00000023;

    /// @dev Fake assessor image id for the R0 adapter. The mock verifier
    ///      ignores it; any non-zero value works.
    bytes32 internal constant R0_ASSESSOR_IMAGE_ID = bytes32(uint256(0xA55E550100));

    /// @dev Analytical add-back for the off-chain Groth16 STARK verify that
    ///      the `NullRiscZeroVerifier` mock skips. Conservative upper bound
    ///      sourced from production Groth16 verifier cost on Base.
    uint256 internal constant R0_GROTH16_VERIFY_GAS = 280_000;

    function setUp() public virtual {
        (proverAddr, proverPk) = makeAddrAndKey("prover");

        onChainAssessor = new OnChainAssessor();
        nullR0 = new NullRiscZeroVerifier();
        r0Assessor = new R0BoundlessAssessorAdapter(nullR0, R0_ASSESSOR_IMAGE_ID);
        nullAssessor = new NullAssessor();
        verifier = new NullVerifier();

        BoundlessRouter implementation = new BoundlessRouter();
        address proxy = UnsafeUpgrades.deployUUPSProxy(
            address(implementation), abi.encodeCall(BoundlessRouter.initialize, (ADMIN))
        );
        router = BoundlessRouter(proxy);

        vm.startPrank(ADMIN);
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
        router.instantiate(ASSESSOR_ON_CHAIN_SEL, address(onChainAssessor), ASSESSOR_CLASS_ID, 0);
        router.instantiate(ASSESSOR_R0_SEL, address(r0Assessor), ASSESSOR_CLASS_ID, 0);
        router.instantiate(ASSESSOR_NULL_SEL, address(nullAssessor), ASSESSOR_CLASS_ID, 0);

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
        router.instantiate(VERIFIER_ENTRY_SEL, address(verifier), VERIFIER_CLASS_ID, 0);
        vm.stopPrank();

        directOnChain = new DirectHarness(onChainAssessor);
        directR0 = new DirectHarness(r0Assessor);
        directNull = new DirectHarness(nullAssessor);
        routerHarness = new RouterHarness(router);
        multiCallHarness = new MultiCallRouterHarness(router);
    }

    // ─── Fixture construction ─────────────────────────────────────────────

    /// @dev Order-generator (~80% of Base traffic) commits a 16-byte journal
    ///      of `input.to_le_bytes()(8) || nonce.to_le_bytes()(8)` — see
    ///      `crates/order-generator/src/main.rs:356`. Default fixture matches.
    uint256 internal constant SMALL_JOURNAL_BYTES = 16;

    /// @dev Larger journal sizing for Steel / app-output workloads. Not directly
    ///      sampled from production; ~128 B is a reasonable upper bound for
    ///      typical hash-or-structured-result commitments.
    uint256 internal constant LARGE_JOURNAL_BYTES = 128;

    function _imageAndJournal(uint256 i) internal pure returns (bytes32 imageId, bytes memory journal) {
        return _imageAndJournal(i, SMALL_JOURNAL_BYTES);
    }

    /// @dev Build a deterministic `(imageId, journal)` pair where the journal
    ///      is `journalBytes` long. The first 16 bytes mirror the
    ///      order-generator's `(input || nonce)` layout; the tail (when
    ///      `journalBytes > 16`) is filled with a deterministic non-zero
    ///      pattern so tx-intrinsic calldata cost (4 vs 16 gas per byte)
    ///      reflects real-traffic shape rather than getting the zero-byte
    ///      discount on the padding.
    function _imageAndJournal(uint256 i, uint256 journalBytes)
        internal
        pure
        returns (bytes32 imageId, bytes memory journal)
    {
        imageId = keccak256(abi.encodePacked("img", i));
        journal = new bytes(journalBytes);
        // Order-generator-style 16-byte prefix: input(8) || nonce(8).
        uint64 input = uint64(i) << 20;
        uint64 nonce = uint64(uint256(keccak256(abi.encodePacked("nonce", i))));
        for (uint256 k = 0; k < 8 && k < journalBytes; k++) {
            journal[k] = bytes1(uint8(input >> (8 * k)));
        }
        for (uint256 k = 0; k < 8 && 8 + k < journalBytes; k++) {
            journal[8 + k] = bytes1(uint8(nonce >> (8 * k)));
        }
        // Non-zero tail: OR-ing with 0x80 keeps the high bit set so every
        // byte is in 0x80..0xff regardless of `k`'s low byte.
        for (uint256 k = 16; k < journalBytes; k++) {
            journal[k] = bytes1(uint8(k) | 0x80);
        }
    }

    function _defaultOffer() internal view returns (Offer memory) {
        return Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            rampUpStart: uint64(block.timestamp),
            rampUpPeriod: 10,
            lockTimeout: 100,
            timeout: 200,
            lockCollateral: 1 ether
        });
    }

    /// @dev Build a `ProofRequest` + matching `Fulfillment` for index `i`
    ///      using the small (order-generator-sized) journal.
    function _makeFill(uint256 i, PredicateType ptype)
        internal
        view
        returns (ProofRequest memory req, Fulfillment memory fill)
    {
        return _makeFill(i, ptype, SMALL_JOURNAL_BYTES);
    }

    /// @dev Build a `ProofRequest` + matching `Fulfillment` for index `i` with
    ///      a journal of length `journalBytes`. The seal selector is set to
    ///      the registered verifier entry's selector so the router dispatches
    ///      correctly.
    function _makeFill(uint256 i, PredicateType ptype, uint256 journalBytes)
        internal
        view
        returns (ProofRequest memory req, Fulfillment memory fill)
    {
        (bytes32 imageId, bytes memory journal) = _imageAndJournal(i, journalBytes);
        bytes32 journalDigest = sha256(abi.encode(journal));
        bytes32 claimDigest = ReceiptClaimLib.ok(imageId, journalDigest).digest();

        Predicate memory predicate;
        if (ptype == PredicateType.DigestMatch) {
            predicate = PredicateLibrary.createDigestMatchPredicate(imageId, journalDigest);
        } else if (ptype == PredicateType.ClaimDigestMatch) {
            predicate = PredicateLibrary.createClaimDigestMatchPredicate(claimDigest);
        } else {
            revert("PrefixMatch not benched (0% Base usage)");
        }

        req = ProofRequest({
            id: RequestIdLibrary.from(CLIENT, uint32(i + 1)),
            requirements: Requirements({
                callback: Callback({addr: address(0), gasLimit: 0}), predicate: predicate, selector: VERIFIER_ENTRY_SEL
            }),
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: _defaultOffer()
        });

        // For ClaimDigestMatch the journal is not posted on-chain — the claim
        // digest itself is the binding, and the assessor never reads the
        // fulfillment data. Modeling it as `None` makes ClaimDigestMatch
        // calldata journal-independent, matching real-traffic shape.
        FulfillmentDataType dataType;
        bytes memory fulfillmentData;
        if (ptype == PredicateType.ClaimDigestMatch) {
            dataType = FulfillmentDataType.None;
            fulfillmentData = "";
        } else {
            dataType = FulfillmentDataType.ImageIdAndJournal;
            fulfillmentData = abi.encode(FulfillmentDataImageIdAndJournal({imageId: imageId, journal: journal}));
        }
        fill = Fulfillment({
            claimDigest: claimDigest,
            fulfillmentDataType: dataType,
            fulfillmentData: fulfillmentData,
            seal: abi.encodePacked(VERIFIER_ENTRY_SEL, hex"deadbeef")
        });
    }

    function _buildBatch(uint256 n, PredicateType ptype)
        internal
        view
        returns (ProofRequest[] memory requests, Fulfillment[] memory fills)
    {
        return _buildBatch(n, ptype, SMALL_JOURNAL_BYTES);
    }

    function _buildBatch(uint256 n, PredicateType ptype, uint256 journalBytes)
        internal
        view
        returns (ProofRequest[] memory requests, Fulfillment[] memory fills)
    {
        requests = new ProofRequest[](n);
        fills = new Fulfillment[](n);
        for (uint256 i = 0; i < n; i++) {
            (requests[i], fills[i]) = _makeFill(i, ptype, journalBytes);
        }
    }

    function _toSlim(ProofRequest memory req) internal pure returns (SlimRequest memory slim) {
        slim = SlimRequest({
            id: req.id,
            predicate: req.requirements.predicate,
            callback: req.requirements.callback,
            selector: req.requirements.selector,
            imageUrlHash: keccak256(bytes(req.imageUrl)),
            inputDigest: InputLibrary.eip712Digest(req.input),
            offerDigest: OfferLibrary.eip712Digest(req.offer)
        });
    }

    /// @dev Build the per-fill SlimRequest + pre-computed requestDigest arrays.
    ///      The bench wants the digest pre-computed (market-side work, out of
    ///      scope for adapter/router measurement).
    function _toSlimBatch(ProofRequest[] memory fullRequests)
        internal
        pure
        returns (SlimRequest[] memory slimRequests, bytes32[] memory requestDigests)
    {
        slimRequests = new SlimRequest[](fullRequests.length);
        requestDigests = new bytes32[](fullRequests.length);
        for (uint256 i = 0; i < fullRequests.length; i++) {
            slimRequests[i] = _toSlim(fullRequests[i]);
            requestDigests[i] = SlimRequestLibrary.reconstructRequestDigest(slimRequests[i]);
        }
    }

    /// @dev Convenience wrapper that packs the harness's 4 raw inputs into the
    ///      `FulfillmentBatch` struct the router and assessor adapters now take.
    function _makeBatch(
        SlimRequest[] memory slim,
        Fulfillment[] memory fills,
        address prover,
        bytes memory assessorSeal
    ) internal pure returns (FulfillmentBatch memory) {
        return FulfillmentBatch({requests: slim, fills: fills, assessorSeal: assessorSeal, prover: prover});
    }

    // ─── Seal builders ────────────────────────────────────────────────────

    /// @dev `OnChainAssessor` seal: `selector || ECDSA(prover signs FulfillmentBatchAuth)`.
    function _buildOnChainSeal(SlimRequest[] memory slim, Fulfillment[] memory fills)
        internal
        view
        returns (bytes memory)
    {
        uint256 n = slim.length;
        bytes32[] memory rd = new bytes32[](n);
        bytes32[] memory cd = new bytes32[](n);
        for (uint256 i = 0; i < n; i++) {
            rd[i] = SlimRequestLibrary.reconstructRequestDigest(slim[i]);
            cd[i] = fills[i].claimDigest;
        }
        bytes32 typehash =
            keccak256("FulfillmentBatchAuth(address prover,bytes32[] requestDigests,bytes32[] claimDigests)");
        bytes32 structHash = keccak256(
            abi.encode(typehash, proverAddr, keccak256(abi.encodePacked(rd)), keccak256(abi.encodePacked(cd)))
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", onChainAssessor.DOMAIN_SEPARATOR(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(proverPk, digest);
        return abi.encodePacked(ASSESSOR_ON_CHAIN_SEL, r, s, v);
    }

    /// @dev `R0BoundlessAssessorAdapter` seal: `selector || innerSeal`. The
    ///      mock R0 verifier ignores the inner seal; production seals are
    ///      ~200 bytes of set-inclusion proof — we use 200 zero bytes to keep
    ///      calldata cost realistic.
    function _buildR0Seal() internal pure returns (bytes memory) {
        // Fill with non-zero bytes — EVM charges 16 gas / non-zero calldata
        // byte vs 4 gas / zero byte, and production seals are essentially
        // random bytes, so zero-filling here would understate calldata gas
        // by ~4x for this slice.
        bytes memory innerSeal = new bytes(200);
        for (uint256 i = 0; i < innerSeal.length; i++) {
            innerSeal[i] = 0xAA;
        }
        return abi.encodePacked(ASSESSOR_R0_SEL, innerSeal);
    }

    /// @dev `NullAssessor` seal: just the selector.
    function _buildNullSeal() internal pure returns (bytes memory) {
        return abi.encodePacked(ASSESSOR_NULL_SEL);
    }
}
