// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {Test, Vm} from "forge-std/Test.sol";
import {console2} from "forge-std/console2.sol";
import {UnsafeUpgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";
import {ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {IERC165} from "@openzeppelin/contracts/utils/introspection/IERC165.sol";

import {OnChainAssessor} from "../../src/router/adapters/OnChainAssessor.sol";
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
import {FulfillmentDataType, FulfillmentDataImageIdAndJournal} from "../../src/types/FulfillmentData.sol";
import {SlimRequest, SlimRequestLibrary} from "../../src/types/SlimRequest.sol";

/// @notice Always-passing `IBoundlessVerifier` used so the per-fill verifier
///         dispatch in `BoundlessRouter.verifySubBatch` doesn't revert during
///         the bench. We're measuring the assessor seam, not the verifier.
contract NullVerifier is IBoundlessVerifier, IERC165 {
    function verify(bytes calldata, bytes32) external pure {}

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == type(IBoundlessVerifier).interfaceId || id == type(IERC165).interfaceId;
    }
}

/// @notice Direct-path harness: simulates the market binding check, then
///         calls the adapter directly (no router). Measures the lower bound
///         of the on-chain assessor's cost.
contract DirectHarness {
    IBoundlessAssessor public immutable ADAPTER;

    error BindingMismatch(uint256 index);

    constructor(IBoundlessAssessor adapter) {
        ADAPTER = adapter;
    }

    function measure(
        SlimRequest[] calldata requests,
        Fulfillment[] calldata fills,
        bytes32[] calldata expectedDigests,
        address prover,
        bytes calldata assessorSeal
    ) external view returns (uint256 gasUsed) {
        uint256 g0 = gasleft();
        // Market-side binding check: reconstruct each requestDigest and assert
        // it matches the stored lock value. We capture the reconstructed values
        // into a memory array so we can forward them to the adapter without
        // having it recompute.
        uint256 n = requests.length;
        bytes32[] memory requestDigests = new bytes32[](n);
        for (uint256 i = 0; i < n; i++) {
            bytes32 reconstructed = SlimRequestLibrary.reconstructRequestDigest(requests[i]);
            if (reconstructed != expectedDigests[i]) revert BindingMismatch(i);
            requestDigests[i] = reconstructed;
        }
        ADAPTER.verifyAssessor(requests, fills, requestDigests, prover, assessorSeal);
        gasUsed = g0 - gasleft();
    }
}

/// @notice Router-path harness: simulates the market binding check, then
///         dispatches through `BoundlessRouter.verifySubBatch`. Measures
///         the realistic end-to-end cost a production transaction would
///         incur.
contract RouterHarness {
    BoundlessRouter public immutable ROUTER;

    error BindingMismatch(uint256 index);

    constructor(BoundlessRouter router) {
        ROUTER = router;
    }

    function measure(
        SlimRequest[] calldata requests,
        Fulfillment[] calldata fills,
        bytes32[] calldata expectedDigests,
        address prover,
        bytes calldata assessorSeal
    ) external view returns (uint256 gasUsed) {
        uint256 g0 = gasleft();
        uint256 n = requests.length;
        bytes32[] memory requestDigests = new bytes32[](n);
        for (uint256 i = 0; i < n; i++) {
            bytes32 reconstructed = SlimRequestLibrary.reconstructRequestDigest(requests[i]);
            if (reconstructed != expectedDigests[i]) revert BindingMismatch(i);
            requestDigests[i] = reconstructed;
        }
        ROUTER.verifySubBatch(requests, fills, requestDigests, prover, assessorSeal);
        gasUsed = g0 - gasleft();
    }
}

contract OnChainAssessorBench is Test {
    using ReceiptClaimLib for ReceiptClaim;

    OnChainAssessor internal assessor;
    NullVerifier internal verifier;
    BoundlessRouter internal router;

    DirectHarness internal directHarness;
    RouterHarness internal routerHarness;

    /// @dev Prover private key + address sourced via `vm.makeAddrAndKey` so
    ///      `vm.addr(pk)` and `vm.sign(pk, ...)` are guaranteed to agree
    ///      (avoids foundry quirks with hash-derived or struct-returned keys).
    uint256 internal proverPk;
    address internal proverAddr;
    address internal CLIENT = address(0xA11CE);
    address internal constant ADMIN = address(0xA);

    bytes4 internal constant VERIFIER_CLASS_ID = 0x00000010;
    bytes4 internal constant VERIFIER_ENTRY_SEL = 0x00000011;
    bytes4 internal constant ASSESSOR_CLASS_ID = 0x00000020;
    bytes4 internal constant ASSESSOR_ENTRY_SEL = 0x00000021;

    function setUp() public {
        (proverAddr, proverPk) = makeAddrAndKey("prover");

        assessor = new OnChainAssessor();
        verifier = new NullVerifier();

        BoundlessRouter implementation = new BoundlessRouter();
        address proxy =
            UnsafeUpgrades.deployUUPSProxy(address(implementation), abi.encodeCall(BoundlessRouter.initialize, (ADMIN)));
        router = BoundlessRouter(proxy);

        // Register the assessor class first so the verifier class can reference it.
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
                // Large enough for N=100 batches in the bench (claim-digest
                // reconstruction + ECDSA recover + sparse-array building).
                defaultGasLimit: 10_000_000,
                label: ""
            })
        );
        router.instantiate(ASSESSOR_ENTRY_SEL, address(assessor), ASSESSOR_CLASS_ID, 0);

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

        directHarness = new DirectHarness(assessor);
        routerHarness = new RouterHarness(router);
    }

    // ─── Fixture construction ─────────────────────────────────────────────

    function _imageAndJournal(uint256 i) internal pure returns (bytes32 imageId, bytes memory journal) {
        imageId = keccak256(abi.encodePacked("img", i));
        journal = abi.encodePacked("journal", i);
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

    /// @dev Build a `ProofRequest` + matching `Fulfillment` with seal-selector
    ///      set to the registered verifier entry's selector.
    function _makeFill(uint256 i, PredicateType ptype)
        internal
        view
        returns (ProofRequest memory req, Fulfillment memory fill)
    {
        (bytes32 imageId, bytes memory journal) = _imageAndJournal(i);
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
                callback: Callback({addr: address(0), gasLimit: 0}),
                predicate: predicate,
                selector: VERIFIER_ENTRY_SEL
            }),
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: _defaultOffer()
        });

        // The first 4 bytes of `seal` MUST be the registered verifier entry's
        // selector so the router's per-fill dispatch resolves correctly.
        bytes memory fulfillmentData =
            abi.encode(FulfillmentDataImageIdAndJournal({imageId: imageId, journal: journal}));
        fill = Fulfillment({
            claimDigest: claimDigest,
            fulfillmentDataType: FulfillmentDataType.ImageIdAndJournal,
            fulfillmentData: fulfillmentData,
            seal: abi.encodePacked(VERIFIER_ENTRY_SEL, hex"deadbeef") // selector || dummy
        });
    }

    function _buildBatch(uint256 n, PredicateType ptype)
        internal
        view
        returns (ProofRequest[] memory requests, Fulfillment[] memory fills)
    {
        requests = new ProofRequest[](n);
        fills = new Fulfillment[](n);
        for (uint256 i = 0; i < n; i++) {
            (requests[i], fills[i]) = _makeFill(i, ptype);
        }
    }

    function _buildMixedBatch(uint256 n)
        internal
        view
        returns (ProofRequest[] memory requests, Fulfillment[] memory fills)
    {
        requests = new ProofRequest[](n);
        fills = new Fulfillment[](n);
        for (uint256 i = 0; i < n; i++) {
            PredicateType ptype = (i % 5 == 0) ? PredicateType.ClaimDigestMatch : PredicateType.DigestMatch;
            (requests[i], fills[i]) = _makeFill(i, ptype);
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

    function _toSlimBatch(ProofRequest[] memory fullRequests)
        internal
        pure
        returns (SlimRequest[] memory slimRequests, bytes32[] memory expectedDigests)
    {
        slimRequests = new SlimRequest[](fullRequests.length);
        expectedDigests = new bytes32[](fullRequests.length);
        for (uint256 i = 0; i < fullRequests.length; i++) {
            slimRequests[i] = _toSlim(fullRequests[i]);
            expectedDigests[i] = fullRequests[i].eip712Digest();
        }
    }

    /// @dev Build a valid `assessorSeal = ASSESSOR_ENTRY_SEL || sig` where `sig`
    ///      is the prover's ECDSA signature over the EIP-712 SubBatchAuth digest.
    function _buildAssessorSeal(SlimRequest[] memory slim, Fulfillment[] memory fills)
        internal
        returns (bytes memory)
    {
        uint256 n = slim.length;
        bytes32[] memory rd = new bytes32[](n);
        bytes32[] memory cd = new bytes32[](n);
        for (uint256 i = 0; i < n; i++) {
            rd[i] = SlimRequestLibrary.reconstructRequestDigest(slim[i]);
            cd[i] = fills[i].claimDigest;
        }
        bytes32 typehash = keccak256("SubBatchAuth(address prover,bytes32[] requestDigests,bytes32[] claimDigests)");
        bytes32 structHash = keccak256(
            abi.encode(
                typehash, proverAddr, keccak256(abi.encodePacked(rd)), keccak256(abi.encodePacked(cd))
            )
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", assessor.DOMAIN_SEPARATOR(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(proverPk, digest);
        return abi.encodePacked(ASSESSOR_ENTRY_SEL, r, s, v);
    }

    // ─── Bench ────────────────────────────────────────────────────────────

    function test_bench_table() external {
        uint256[5] memory sizes = [uint256(1), 5, 10, 50, 100];
        address prover = proverAddr;

        console2.log("");
        console2.log("=== DIRECT (market binding + OnChainAssessor, no router) ===");
        console2.log("| N   | DigestMatch total | DigestMatch / fill | ClaimDigestMatch total | ClaimDigestMatch / fill |");
        console2.log("|-----|-------------------|--------------------|------------------------|-------------------------|");
        for (uint256 k = 0; k < sizes.length; k++) {
            uint256 n = sizes[k];

            (ProofRequest[] memory rd, Fulfillment[] memory fd) = _buildBatch(n, PredicateType.DigestMatch);
            (SlimRequest[] memory sd, bytes32[] memory ed) = _toSlimBatch(rd);
            bytes memory sealD = _buildAssessorSeal(sd, fd);
            uint256 gd = directHarness.measure(sd, fd, ed, prover, sealD);

            (ProofRequest[] memory rc, Fulfillment[] memory fc) = _buildBatch(n, PredicateType.ClaimDigestMatch);
            (SlimRequest[] memory sc, bytes32[] memory ec) = _toSlimBatch(rc);
            bytes memory sealC = _buildAssessorSeal(sc, fc);
            uint256 gc = directHarness.measure(sc, fc, ec, prover, sealC);

            console2.log(_row(n, gd, gc));
        }

        console2.log("");
        console2.log("=== ROUTER (market binding + BoundlessRouter dispatch + OnChainAssessor) ===");
        console2.log("| N   | DigestMatch total | DigestMatch / fill | ClaimDigestMatch total | ClaimDigestMatch / fill |");
        console2.log("|-----|-------------------|--------------------|------------------------|-------------------------|");
        for (uint256 k = 0; k < sizes.length; k++) {
            uint256 n = sizes[k];

            (ProofRequest[] memory rd, Fulfillment[] memory fd) = _buildBatch(n, PredicateType.DigestMatch);
            (SlimRequest[] memory sd, bytes32[] memory ed) = _toSlimBatch(rd);
            bytes memory sealD = _buildAssessorSeal(sd, fd);
            uint256 gd = routerHarness.measure(sd, fd, ed, prover, sealD);

            (ProofRequest[] memory rc, Fulfillment[] memory fc) = _buildBatch(n, PredicateType.ClaimDigestMatch);
            (SlimRequest[] memory sc, bytes32[] memory ec) = _toSlimBatch(rc);
            bytes memory sealC = _buildAssessorSeal(sc, fc);
            uint256 gc = routerHarness.measure(sc, fc, ec, prover, sealC);

            console2.log(_row(n, gd, gc));
        }

        console2.log("");
        console2.log("=== Mixed 80/20 DigestMatch / ClaimDigestMatch (direct, router) ===");
        for (uint256 k = 0; k < sizes.length; k++) {
            uint256 n = sizes[k];
            (ProofRequest[] memory rm, Fulfillment[] memory fm) = _buildMixedBatch(n);
            (SlimRequest[] memory sm, bytes32[] memory em) = _toSlimBatch(rm);
            bytes memory sealM = _buildAssessorSeal(sm, fm);
            uint256 gd = directHarness.measure(sm, fm, em, prover, sealM);
            uint256 gr = routerHarness.measure(sm, fm, em, prover, sealM);
            console2.log("  N=%d  direct/fill=%d  router/fill=%d", n, gd / n, gr / n);
        }
    }

    function _row(uint256 n, uint256 a, uint256 b) internal pure returns (string memory) {
        return string.concat(
            "| ",
            _pad(_u2s(n), 3),
            " | ",
            _pad(_u2s(a), 17),
            " | ",
            _pad(_u2s(a / n), 18),
            " | ",
            _pad(_u2s(b), 22),
            " | ",
            _pad(_u2s(b / n), 23),
            " |"
        );
    }

    // ─── Sanity tests ─────────────────────────────────────────────────────

    function test_slim_reconstructionMatchesFullDigest() external view {
        (ProofRequest[] memory rd,) = _buildBatch(3, PredicateType.DigestMatch);
        for (uint256 i = 0; i < rd.length; i++) {
            SlimRequest memory slim = _toSlim(rd[i]);
            assertEq(SlimRequestLibrary.reconstructRequestDigest(slim), rd[i].eip712Digest());
        }
    }

    function test_direct_singleFill_passes() external {
        (ProofRequest[] memory rd, Fulfillment[] memory fd) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory sd, bytes32[] memory ed) = _toSlimBatch(rd);
        bytes memory seal = _buildAssessorSeal(sd, fd);
        directHarness.measure(sd, fd, ed, proverAddr, seal);
    }

    function test_router_singleFill_passes() external {
        (ProofRequest[] memory rd, Fulfillment[] memory fd) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory sd, bytes32[] memory ed) = _toSlimBatch(rd);
        bytes memory seal = _buildAssessorSeal(sd, fd);
        routerHarness.measure(sd, fd, ed, proverAddr, seal);
    }

    function test_predicateFailureReverts() external {
        (ProofRequest[] memory rd, Fulfillment[] memory fd) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory sd, bytes32[] memory ed) = _toSlimBatch(rd);
        // Tamper with fulfillment journal — predicate eval should fail before
        // the signature check, so the seal can be any 69-byte placeholder.
        bytes memory wrongJournal = bytes("not-the-journal");
        (bytes32 imageId,) = _imageAndJournal(0);
        fd[0].fulfillmentData =
            abi.encode(FulfillmentDataImageIdAndJournal({imageId: imageId, journal: wrongJournal}));
        bytes memory seal = _buildAssessorSeal(sd, fd);

        vm.expectRevert(abi.encodeWithSelector(OnChainAssessor.PredicateFailed.selector, uint256(0)));
        directHarness.measure(sd, fd, ed, proverAddr, seal);
    }

    function test_bindingMismatchReverts() external {
        (ProofRequest[] memory rd, Fulfillment[] memory fd) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory sd, bytes32[] memory ed) = _toSlimBatch(rd);
        ed[0] = bytes32(uint256(ed[0]) ^ 1);
        bytes memory seal = _buildAssessorSeal(sd, fd);
        vm.expectRevert(abi.encodeWithSelector(DirectHarness.BindingMismatch.selector, uint256(0)));
        directHarness.measure(sd, fd, ed, proverAddr, seal);
    }

    function test_proverSignatureMismatchReverts() external {
        (ProofRequest[] memory rd, Fulfillment[] memory fd) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory sd, bytes32[] memory ed) = _toSlimBatch(rd);
        // Build a valid seal — but pass a different prover address. The
        // adapter reconstructs its expected digest with the supplied prover,
        // which differs from the digest the signature actually signs.
        // ECDSA.recover returns an unrelated address; the assertion is just
        // that the mismatch is detected (selector-only match).
        bytes memory seal = _buildAssessorSeal(sd, fd);
        vm.expectPartialRevert(OnChainAssessor.ProverSignatureMismatch.selector);
        directHarness.measure(sd, fd, ed, address(0xDEAD), seal);
    }

    function test_claimDigestMismatchReverts() external {
        (ProofRequest[] memory rd, Fulfillment[] memory fd) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory sd, bytes32[] memory ed) = _toSlimBatch(rd);
        // Keep fulfillmentData (predicate eval passes) but break the claim digest.
        fd[0].claimDigest = bytes32(uint256(fd[0].claimDigest) ^ 1);
        bytes memory seal = _buildAssessorSeal(sd, fd);
        vm.expectRevert(abi.encodeWithSelector(OnChainAssessor.ClaimDigestMismatch.selector, uint256(0)));
        directHarness.measure(sd, fd, ed, proverAddr, seal);
    }

    // ─── Utility ─────────────────────────────────────────────────────────

    function _u2s(uint256 v) internal pure returns (string memory) {
        return vm.toString(v);
    }

    function _pad(string memory s, uint256 width) internal pure returns (string memory) {
        bytes memory b = bytes(s);
        if (b.length >= width) return s;
        bytes memory padded = new bytes(width);
        for (uint256 i = 0; i < width - b.length; i++) padded[i] = bytes1(" ");
        for (uint256 i = 0; i < b.length; i++) padded[width - b.length + i] = b[i];
        return string(padded);
    }
}
