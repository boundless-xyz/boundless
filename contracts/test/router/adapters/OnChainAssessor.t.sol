// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {Test} from "forge-std/Test.sol";
import {IERC165} from "@openzeppelin/contracts/utils/introspection/IERC165.sol";
import {MessageHashUtils} from "@openzeppelin/contracts/utils/cryptography/MessageHashUtils.sol";
import {ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";

import {OnChainAssessor} from "../../../src/router/adapters/OnChainAssessor.sol";
import {IBoundlessAssessor} from "../../../src/router/interfaces/IBoundlessAssessor.sol";
import {ProofRequest} from "../../../src/types/ProofRequest.sol";
import {Fulfillment} from "../../../src/types/Fulfillment.sol";
import {FulfillmentBatch} from "../../../src/types/FulfillmentBatch.sol";
import {FulfillmentDataType, FulfillmentDataImageIdAndJournal} from "../../../src/types/FulfillmentData.sol";
import {Predicate, PredicateType, PredicateLibrary} from "../../../src/types/Predicate.sol";
import {Requirements} from "../../../src/types/Requirements.sol";
import {Callback} from "../../../src/types/Callback.sol";
import {Input, InputType, InputLibrary} from "../../../src/types/Input.sol";
import {Offer, OfferLibrary} from "../../../src/types/Offer.sol";
import {RequestIdLibrary} from "../../../src/types/RequestId.sol";
import {SlimRequest, SlimRequestLibrary} from "../../../src/types/SlimRequest.sol";

/// @title OnChainAssessorTest — unit tests for the native Solidity assessor.
///
/// @notice Pins the on-chain adapter's behavior at parity with the off-chain
///         guest-based path:
///           * predicate evaluation for all three `PredicateType`s,
///           * claim-digest binding to the supplied `(imageId, journal)` (both
///             the `DigestMatch` / `PrefixMatch` path and the
///             `ClaimDigestMatch + ImageIdAndJournal` reconstruction guard),
///           * batch-level prover signature binding,
///           * slim-payload digest reconstruction matching the original
///             `ProofRequest.eip712Digest()`,
///           * length and seal-format input guards,
///           * tamper detection on caller-supplied `requestDigests` and on
///             post-signing `fulfillmentData` mutations,
///           * ERC-165 conformance.
contract OnChainAssessorTest is Test {
    using ReceiptClaimLib for ReceiptClaim;

    OnChainAssessor internal adapter;

    /// @dev Prover wallet used for the EIP-712 batch signature. Sourced via
    ///      `vm.makeAddrAndKey` so `vm.addr(pk)` and `vm.sign(pk, ...)` agree.
    address internal proverAddr;
    uint256 internal proverPk;

    address internal constant CLIENT = address(0xA11CE);

    /// @dev The selector the seal prefixes when an OnChainAssessor caller
    ///      builds a router-style seal. The adapter itself doesn't validate
    ///      this value (the router does), but the seal format requires it
    ///      so that `_verifyProverSignature` strips the correct 4 bytes.
    bytes4 internal constant ASSESSOR_SEL = 0x00000021;

    /// @dev Stand-in verifier-entry selector signed into every fixture
    ///      `SlimRequest`. The adapter never reads this — it's downstream of
    ///      the router — but the slim payload requires a value.
    bytes4 internal constant VERIFIER_SEL = 0x00000011;

    function setUp() public {
        adapter = new OnChainAssessor();
        (proverAddr, proverPk) = makeAddrAndKey("prover");
    }

    // ─── Single-fill happy paths ────────────────────────────────────────

    function test_singleFill_digestMatch_passes() external view {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    function test_singleFill_claimDigestMatch_passes() external view {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.ClaimDigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    /// @notice ClaimDigestMatch + ImageIdAndJournal: the prover attached
    ///         (imageId, journal) — typically because the request has a
    ///         callback — and they reconstruct to the proven claimDigest.
    ///         Mirrors the guest's behavior (assessor lib `Predicate::eval`
    ///         asserts the reconstruction matches `predicate.data`).
    function test_claimDigestMatch_withMatchingFulfillmentData_passes() external view {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.ClaimDigestMatch);
        // ClaimDigestMatch fills are built with fulfillmentData=None; attach
        // the matching (imageId, journal) here. `_imageAndJournal(0)` is the
        // same source the request's claimDigest was derived from.
        (bytes32 imageId, bytes memory journal) = _imageAndJournal(0);
        f[0].fulfillmentDataType = FulfillmentDataType.ImageIdAndJournal;
        f[0].fulfillmentData = abi.encode(FulfillmentDataImageIdAndJournal({imageId: imageId, journal: journal}));
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    function test_singleFill_prefixMatch_passes() external view {
        (ProofRequest memory req, Fulfillment memory fill) = _makePrefixMatchFill(0);
        ProofRequest[] memory r = _asArray(req);
        Fulfillment[] memory f = _asArray(fill);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    // ─── Multi-fill happy paths ─────────────────────────────────────────

    function test_multiFill_digestMatch_passes() external view {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(5, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    function test_multiFill_claimDigestMatch_passes() external view {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(5, PredicateType.ClaimDigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    /// @notice Mixed predicate types in one batch — exercises the per-fill
    ///         predicate switch without making the test depend on order.
    function test_multiFill_mixedPredicates_passes() external view {
        ProofRequest[] memory r = new ProofRequest[](3);
        Fulfillment[] memory f = new Fulfillment[](3);
        (r[0], f[0]) = _makeFill(0, PredicateType.DigestMatch);
        (r[1], f[1]) = _makeFill(1, PredicateType.ClaimDigestMatch);
        (r[2], f[2]) = _makePrefixMatchFill(2);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    // ─── Predicate failures ─────────────────────────────────────────────

    function test_predicateFailure_digestMatch_wrongJournal_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        // Tamper with fulfillment journal — predicate eval should fail before
        // the signature check, so the seal contents don't matter.
        (bytes32 imageId,) = _imageAndJournal(0);
        f[0].fulfillmentData =
            abi.encode(FulfillmentDataImageIdAndJournal({imageId: imageId, journal: bytes("not-the-journal")}));
        bytes memory seal = _buildSeal(s, f);
        vm.expectRevert(abi.encodeWithSelector(OnChainAssessor.ClaimDigestMismatch.selector, uint256(0)));
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    function test_predicateFailure_digestMatch_wrongImageId_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        // Use index 0's journal but index 1's imageId — predicate fails on
        // imageId mismatch even though the journal is otherwise pristine.
        (, bytes memory journal) = _imageAndJournal(0);
        (bytes32 wrongImageId,) = _imageAndJournal(1);
        f[0].fulfillmentData = abi.encode(FulfillmentDataImageIdAndJournal({imageId: wrongImageId, journal: journal}));
        bytes memory seal = _buildSeal(s, f);
        vm.expectRevert(abi.encodeWithSelector(OnChainAssessor.ClaimDigestMismatch.selector, uint256(0)));
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    function test_predicateFailure_prefixMatch_journalDoesNotStartWithPrefix_reverts() external {
        (ProofRequest memory req, Fulfillment memory fill) = _makePrefixMatchFill(0);
        // Replace the journal with one that doesn't start with the prefix the
        // request signed, and update claimDigest so the reconstruction guard
        // passes — isolating the prefix check as the failure path.
        (bytes32 imageId,) = _imageAndJournal(0);
        bytes memory newJournal = bytes("xxxxxxxxRESTOFTHEJOURNAL");
        fill.fulfillmentData = abi.encode(FulfillmentDataImageIdAndJournal({imageId: imageId, journal: newJournal}));
        fill.claimDigest = ReceiptClaimLib.ok(imageId, sha256(newJournal)).digest();
        ProofRequest[] memory r = _asArray(req);
        Fulfillment[] memory f = _asArray(fill);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        vm.expectRevert(abi.encodeWithSelector(OnChainAssessor.PredicateFailed.selector, uint256(0)));
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    /// @notice ClaimDigestMatch + ImageIdAndJournal: prover attached
    ///         (imageId, journal) that does NOT reconstruct to the proven
    ///         claimDigest. Without this check a callback would dispatch
    ///         unproven bytes; the adapter must reject so it stays in lock-step
    ///         with the guest's `Predicate::eval`.
    function test_claimDigestMatch_withMismatchedFulfillmentData_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.ClaimDigestMatch);
        // Attach (imageId, journal) sourced from a *different* index so the
        // reconstructed claim digest can't match. claimDigest itself is
        // unchanged — the predicate.eval(fill.claimDigest) check still
        // passes; only the reconstruction guard catches the mismatch.
        (bytes32 imageId, bytes memory journal) = _imageAndJournal(1);
        f[0].fulfillmentDataType = FulfillmentDataType.ImageIdAndJournal;
        f[0].fulfillmentData = abi.encode(FulfillmentDataImageIdAndJournal({imageId: imageId, journal: journal}));
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        vm.expectRevert(abi.encodeWithSelector(OnChainAssessor.ClaimDigestMismatch.selector, uint256(0)));
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    // ─── Claim digest binding ───────────────────────────────────────────

    function test_claimDigestMismatch_postSigning_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        // Predicate eval passes (fulfillmentData intact), but the supplied
        // claim digest doesn't reconstruct from the journal — should revert.
        f[0].claimDigest = bytes32(uint256(f[0].claimDigest) ^ 1);
        bytes memory seal = _buildSeal(s, f);
        vm.expectRevert(abi.encodeWithSelector(OnChainAssessor.ClaimDigestMismatch.selector, uint256(0)));
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    // ─── MissingFulfillmentData ─────────────────────────────────────────

    function test_digestMatch_withFulfillmentDataNone_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        // DigestMatch requires (imageId, journal); the adapter MUST refuse to
        // run if the prover didn't attach them.
        f[0].fulfillmentDataType = FulfillmentDataType.None;
        f[0].fulfillmentData = "";
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        vm.expectRevert(abi.encodeWithSelector(OnChainAssessor.MissingFulfillmentData.selector, uint256(0)));
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    function test_prefixMatch_withFulfillmentDataNone_reverts() external {
        (ProofRequest memory req, Fulfillment memory fill) = _makePrefixMatchFill(0);
        fill.fulfillmentDataType = FulfillmentDataType.None;
        fill.fulfillmentData = "";
        ProofRequest[] memory r = _asArray(req);
        Fulfillment[] memory f = _asArray(fill);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        vm.expectRevert(abi.encodeWithSelector(OnChainAssessor.MissingFulfillmentData.selector, uint256(0)));
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    // ─── Length checks ──────────────────────────────────────────────────

    function test_lengthMismatch_fillsShorterThanRequests_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(2, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        // Truncate fills to length 1 — requests still length 2.
        Fulfillment[] memory truncated = new Fulfillment[](1);
        truncated[0] = f[0];
        bytes memory seal = _buildSeal(s, f);
        vm.expectRevert(OnChainAssessor.LengthMismatch.selector);
        adapter.verifyAssessor(_makeBatch(s, truncated, proverAddr, seal), rd);
    }

    function test_lengthMismatch_requestDigestsShorterThanRequests_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(2, PredicateType.DigestMatch);
        (SlimRequest[] memory s,) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        bytes32[] memory shortRd = new bytes32[](1);
        vm.expectRevert(OnChainAssessor.LengthMismatch.selector);
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), shortRd);
    }

    // ─── Malformed seal ─────────────────────────────────────────────────

    function test_malformedSeal_onlySelector_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = abi.encodePacked(ASSESSOR_SEL);
        vm.expectRevert(OnChainAssessor.MalformedProverSignature.selector);
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    function test_malformedSeal_oneByteShort_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        // 68 bytes = 4 selector + 64 sig bytes (one byte short of the standard
        // 65-byte ECDSA signature). The adapter requires exactly 4 + 65.
        bytes memory seal = abi.encodePacked(ASSESSOR_SEL, new bytes(64));
        vm.expectRevert(OnChainAssessor.MalformedProverSignature.selector);
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    function test_malformedSeal_oneByteLong_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory baseSeal = _buildSeal(s, f);
        bytes memory bloated = abi.encodePacked(baseSeal, hex"00");
        vm.expectRevert(OnChainAssessor.MalformedProverSignature.selector);
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, bloated), rd);
    }

    // ─── Tamper detection ───────────────────────────────────────────────

    function test_tamper_requestDigest_postSigning_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(2, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        // Flip a bit in one caller-supplied requestDigest after signing. The
        // adapter recomputes the signed struct hash over the supplied array,
        // so the recovered signer no longer equals `proverAddr`.
        rd[1] = bytes32(uint256(rd[1]) ^ 1);
        vm.expectPartialRevert(OnChainAssessor.ProverSignatureMismatch.selector);
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    function test_tamper_claimDigest_postSigning_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(2, PredicateType.ClaimDigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        // ClaimDigestMatch + None: the predicate.eval(claimDigest) check
        // catches the tampered value before the signature step.
        f[1].claimDigest = bytes32(uint256(f[1].claimDigest) ^ 1);
        vm.expectRevert(abi.encodeWithSelector(OnChainAssessor.PredicateFailed.selector, uint256(1)));
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    /// @dev Tamper fulfillmentData in a way that defeats both the predicate
    ///      check and the reconstruction guard, isolating the per-batch
    ///      signature as the failure path. We use PrefixMatch so the new
    ///      journal can still satisfy the predicate (it preserves the signed
    ///      prefix), and we update fill.claimDigest so the reconstruction
    ///      guard also passes. The seal — signed before the mutation — is
    ///      bound to the *original* claimDigest, so signature recovery yields
    ///      the wrong address.
    function test_tamper_fulfillmentData_postSigning_reverts() external {
        (ProofRequest memory req, Fulfillment memory fill) = _makePrefixMatchFill(0);
        ProofRequest[] memory r = _asArray(req);
        Fulfillment[] memory f = _asArray(fill);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);

        // _imageAndJournal(0) sets the first 8 bytes (the prefix) to zero, so
        // a fresh zero-initialized buffer already starts with the signed
        // prefix; one trailing non-zero byte is enough to diverge from the
        // original journal and produce a different claimDigest.
        (bytes32 imageId,) = _imageAndJournal(0);
        bytes memory newJournal = new bytes(9);
        newJournal[8] = 0xAA;
        f[0].fulfillmentData = abi.encode(FulfillmentDataImageIdAndJournal({imageId: imageId, journal: newJournal}));
        f[0].claimDigest = ReceiptClaimLib.ok(imageId, sha256(newJournal)).digest();

        vm.expectPartialRevert(OnChainAssessor.ProverSignatureMismatch.selector);
        adapter.verifyAssessor(_makeBatch(s, f, proverAddr, seal), rd);
    }

    function test_tamper_proverAddress_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildSeal(s, f);
        // Seal is valid for `proverAddr`, but the batch claims a different prover.
        // The recovered signer doesn't match the supplied `prover`.
        vm.expectPartialRevert(OnChainAssessor.ProverSignatureMismatch.selector);
        adapter.verifyAssessor(_makeBatch(s, f, address(0xDEAD), seal), rd);
    }

    // ─── ERC-165 ────────────────────────────────────────────────────────

    function test_erc165_supportsIBoundlessAssessor() external view {
        assertTrue(adapter.supportsInterface(type(IBoundlessAssessor).interfaceId));
        assertTrue(adapter.supportsInterface(type(IERC165).interfaceId));
        assertFalse(adapter.supportsInterface(bytes4(0xdeadbeef)));
    }

    // ─── Domain separator ───────────────────────────────────────────────

    function test_domainSeparator_bindsChainIdAndAddress() external view {
        bytes32 expected = keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                keccak256("OnChainAssessor"),
                keccak256("1"),
                block.chainid,
                address(adapter)
            )
        );
        assertEq(adapter.DOMAIN_SEPARATOR(), expected);
    }

    /// @notice Pin the literal type string to the contract's typehash. The
    ///         seal helper above reads `FULFILLMENT_BATCH_AUTH_TYPEHASH()`
    ///         from the contract for ergonomics; this test guards against a
    ///         silent rename of the type string that would invalidate every
    ///         broker's pre-computed seal.
    function test_typehash_matchesLiteralTypeString() external view {
        bytes32 expected =
            keccak256("FulfillmentBatchAuth(address prover,bytes32[] requestDigests,bytes32[] claimDigests)");
        assertEq(adapter.FULFILLMENT_BATCH_AUTH_TYPEHASH(), expected);
        assertEq(keccak256(bytes(adapter.FULFILLMENT_BATCH_AUTH_TYPE())), expected);
    }

    // ════════════════════════════════════════════════════════════════════
    // Fixture builders
    // ════════════════════════════════════════════════════════════════════
    //
    // These produce deterministic, self-consistent (request, fulfillment)
    // tuples for each predicate type. Index `i` seeds the imageId, journal,
    // and request id so distinct fills don't collide. Real ECDSA signing
    // happens in `_buildSeal` against `adapter.DOMAIN_SEPARATOR()`.

    /// @dev Deterministic (imageId, journal) for fill index `i`. The journal
    ///      is 16 bytes — the order-generator-sized payload that covers ~80%
    ///      of Base traffic — with a non-zero pattern so calldata costs
    ///      reflect real-traffic shape.
    function _imageAndJournal(uint256 i) internal pure returns (bytes32 imageId, bytes memory journal) {
        imageId = keccak256(abi.encodePacked("img", i));
        journal = new bytes(16);
        uint64 input = uint64(i) << 20;
        uint64 nonce = uint64(uint256(keccak256(abi.encodePacked("nonce", i))));
        for (uint256 k = 0; k < 8; k++) {
            journal[k] = bytes1(uint8(input >> (8 * k)));
        }
        for (uint256 k = 0; k < 8; k++) {
            journal[8 + k] = bytes1(uint8(nonce >> (8 * k)));
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

    /// @dev Build a `ProofRequest` + matching `Fulfillment` for index `i`.
    ///      DigestMatch and PrefixMatch fills carry an `ImageIdAndJournal`
    ///      payload; ClaimDigestMatch carries `None` (the common case — no
    ///      callback). PrefixMatch is built by `_makePrefixMatchFill` since
    ///      its predicate layout differs.
    function _makeFill(uint256 i, PredicateType ptype)
        internal
        view
        returns (ProofRequest memory req, Fulfillment memory fill)
    {
        (bytes32 imageId, bytes memory journal) = _imageAndJournal(i);
        bytes32 journalDigest = sha256(journal);
        bytes32 claimDigest = ReceiptClaimLib.ok(imageId, journalDigest).digest();

        Predicate memory predicate;
        if (ptype == PredicateType.DigestMatch) {
            predicate = PredicateLibrary.createDigestMatchPredicate(imageId, journalDigest);
        } else if (ptype == PredicateType.ClaimDigestMatch) {
            predicate = PredicateLibrary.createClaimDigestMatchPredicate(claimDigest);
        } else {
            revert("Use _makePrefixMatchFill for PrefixMatch");
        }

        req = ProofRequest({
            id: RequestIdLibrary.from(CLIENT, uint32(i + 1)),
            requirements: Requirements({
                callback: Callback({addr: address(0), gasLimit: 0}), predicate: predicate, selector: VERIFIER_SEL
            }),
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: _defaultOffer()
        });

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
            seal: abi.encodePacked(VERIFIER_SEL, hex"deadbeef")
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

    /// @dev PrefixMatch fixture: same (imageId, journal) layout as
    ///      `_makeFill`, with the predicate prefix set to the first 8 journal
    ///      bytes (the order-generator's `input` field).
    function _makePrefixMatchFill(uint256 i) internal view returns (ProofRequest memory req, Fulfillment memory fill) {
        (bytes32 imageId, bytes memory journal) = _imageAndJournal(i);
        bytes32 claimDigest = ReceiptClaimLib.ok(imageId, sha256(journal)).digest();

        bytes memory prefix = new bytes(8);
        for (uint256 k = 0; k < 8; k++) {
            prefix[k] = journal[k];
        }
        Predicate memory predicate = PredicateLibrary.createPrefixMatchPredicate(imageId, prefix);

        req = ProofRequest({
            id: RequestIdLibrary.from(CLIENT, uint32(i + 1)),
            requirements: Requirements({
                callback: Callback({addr: address(0), gasLimit: 0}), predicate: predicate, selector: VERIFIER_SEL
            }),
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: _defaultOffer()
        });
        fill = Fulfillment({
            claimDigest: claimDigest,
            fulfillmentDataType: FulfillmentDataType.ImageIdAndJournal,
            fulfillmentData: abi.encode(FulfillmentDataImageIdAndJournal({imageId: imageId, journal: journal})),
            seal: abi.encodePacked(VERIFIER_SEL, hex"deadbeef")
        });
    }

    function _toSlim(ProofRequest memory req) internal pure returns (SlimRequest memory) {
        return SlimRequest({
            id: req.id,
            predicate: req.requirements.predicate,
            callback: req.requirements.callback,
            selector: req.requirements.selector,
            imageUrlHash: keccak256(bytes(req.imageUrl)),
            inputDigest: InputLibrary.eip712Digest(req.input),
            offerDigest: OfferLibrary.eip712Digest(req.offer)
        });
    }

    function _toSlimBatch(ProofRequest[] memory full)
        internal
        pure
        returns (SlimRequest[] memory slim, bytes32[] memory requestDigests)
    {
        slim = new SlimRequest[](full.length);
        requestDigests = new bytes32[](full.length);
        for (uint256 i = 0; i < full.length; i++) {
            slim[i] = _toSlim(full[i]);
            requestDigests[i] = SlimRequestLibrary.reconstructRequestDigest(slim[i]);
        }
    }

    function _makeBatch(
        SlimRequest[] memory slim,
        Fulfillment[] memory fills,
        address prover,
        bytes memory assessorSeal
    ) internal pure returns (FulfillmentBatch memory) {
        return FulfillmentBatch({requests: slim, fills: fills, assessorSeal: assessorSeal, prover: prover});
    }

    /// @dev Build the OnChainAssessor seal: `ASSESSOR_SEL || ECDSA(prover signs
    ///      FulfillmentBatchAuth)`. The selector prefix is what the router
    ///      would strip in production; the adapter strips it via the same
    ///      length offset. The typehash and EIP-712 digest construction are
    ///      sourced from the adapter and OpenZeppelin respectively so this
    ///      helper stays in lock-step with what the contract verifies.
    function _buildSeal(SlimRequest[] memory slim, Fulfillment[] memory fills) internal view returns (bytes memory) {
        uint256 n = slim.length;
        bytes32[] memory rd = new bytes32[](n);
        bytes32[] memory cd = new bytes32[](n);
        for (uint256 i = 0; i < n; i++) {
            rd[i] = SlimRequestLibrary.reconstructRequestDigest(slim[i]);
            cd[i] = fills[i].claimDigest;
        }
        bytes32 structHash = keccak256(
            abi.encode(
                adapter.FULFILLMENT_BATCH_AUTH_TYPEHASH(),
                proverAddr,
                keccak256(abi.encodePacked(rd)),
                keccak256(abi.encodePacked(cd))
            )
        );
        bytes32 digest = MessageHashUtils.toTypedDataHash(adapter.DOMAIN_SEPARATOR(), structHash);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(proverPk, digest);
        return abi.encodePacked(ASSESSOR_SEL, r, s, v);
    }

    // ─── Tiny array helpers ─────────────────────────────────────────────

    function _asArray(ProofRequest memory req) internal pure returns (ProofRequest[] memory arr) {
        arr = new ProofRequest[](1);
        arr[0] = req;
    }

    function _asArray(Fulfillment memory fill) internal pure returns (Fulfillment[] memory arr) {
        arr = new Fulfillment[](1);
        arr[0] = fill;
    }
}
