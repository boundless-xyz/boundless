// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {Vm} from "forge-std/Vm.sol";
import {
    BoundlessMarketLegacyViaFallbackTest, ASSESSOR_IMAGE_ID, APP_JOURNAL
} from "./BoundlessMarketLegacyViaFallback.t.sol";
import {Client} from "./clients/Client.sol";
import {ProofRequest} from "../../src/legacy/types/ProofRequest.sol";
import {Fulfillment} from "../../src/legacy/types/Fulfillment.sol";
import {AssessorReceipt} from "../../src/legacy/types/AssessorReceipt.sol";
import {RequestId} from "../../src/legacy/types/RequestId.sol";

/// @title CrossABITest
/// @notice Pins down behaviors that are unique to the legacy-via-fallback
///         deployment shape — that is, behaviors not exercised by the
///         standalone-legacy or standalone-new test suites in isolation.
///
/// Coverage rationale:
/// - Shared selectors (lockRequest, slash, withdraw, every view getter that
///   exists on both contracts) always execute on the new market because the
///   dispatcher matches before the fallback fires. Behavior across these
///   selectors is already covered by BoundlessMarketLegacyViaFallback's 133
///   tests (which pass through the new impl when selectors collide).
/// - Legacy-only selectors (e.g. fulfill(Fulfillment[], AssessorReceipt),
///   imageInfo, verifyDelivery, VERIFIER, ASSESSOR_ID, the legacy
///   submitRootAnd* variants) are routed via fallback to the legacy impl.
///   The via-fallback suite covers many of these but does not isolate the
///   load-bearing invariants. This file calls them out as named tests so
///   regressions surface clearly.
contract CrossABITest is BoundlessMarketLegacyViaFallbackTest {
    /// Legacy-only view methods remain callable via the fallback.
    /// `imageInfo()` lives only on the legacy impl, so it routes through
    /// `fallback() -> delegatecall(LEGACY_IMPL)`. The new market's
    /// initialize(address) signature does not accept an image URL, so the
    /// proxy's storage slot 2 (imageUrl) is left empty; the assessor image
    /// id, however, is read from an immutable baked into the legacy impl's
    /// own bytecode and is unaffected by which impl is active.
    function testLegacyImageInfoViaFallback() public view {
        (bytes32 assessorId, string memory imageUrl) = boundlessMarket.imageInfo();
        assertEq(assessorId, ASSESSOR_IMAGE_ID, "legacy ASSESSOR_ID immutable should round-trip via fallback");
        assertEq(imageUrl, "", "imageUrl slot was not initialized by the new market's initialize signature");
    }

    /// Immutables declared on the legacy impl (here VERIFIER, ASSESSOR_ID)
    /// are baked into the legacy impl's bytecode, not the proxy's storage.
    /// Reading them through the proxy means executing the legacy impl's
    /// auto-generated getter under delegate-call, which pulls the value
    /// from the legacy bytecode and returns it. The returned values must
    /// match what the legacy impl was constructed with.
    function testLegacyImmutablesReadableViaFallback() public {
        bytes memory verifierData = _callViaFallback(abi.encodeWithSignature("VERIFIER()"));
        bytes memory assessorIdData = _callViaFallback(abi.encodeWithSignature("ASSESSOR_ID()"));

        address verifier = abi.decode(verifierData, (address));
        bytes32 assessorId = abi.decode(assessorIdData, (bytes32));

        assertEq(verifier, address(setVerifier), "VERIFIER immutable should equal the legacy ctor arg");
        assertEq(assessorId, ASSESSOR_IMAGE_ID, "ASSESSOR_ID immutable should equal the legacy ctor arg");
    }

    /// A selector not declared on either the new market or the legacy impl
    /// reverts cleanly. The fallback delegate-calls into the legacy impl;
    /// the legacy impl's dispatcher does not match either and returns
    /// empty calldata after running out of options, producing a plain
    /// revert that propagates back through the fallback.
    function testFallbackRevertsOnUnknownSelector() public {
        bytes memory data = abi.encodeWithSelector(bytes4(keccak256("nonExistentMethod()")));
        (bool ok,) = address(boundlessMarket).call(data);
        assertFalse(ok, "unknown selector must revert");
    }

    /// After a legacy fulfill that flows through fallback, the new market's
    /// shared view selectors (requestIsFulfilled lives on both contracts;
    /// it routes to the new impl because the selector collides) read the
    /// state the legacy impl wrote. This is the load-bearing storage
    /// interop invariant for cross-ABI lifecycles: write via legacy, read
    /// via new.
    function testSharedViewReadsLegacyFulfilledState() public {
        Client client = getClient(0);
        ProofRequest memory request = client.request(0);
        bytes memory clientSignature = client.sign(request);

        // Lock via the shared lockRequest selector (executes on the new market).
        vm.prank(testProverAddress);
        boundlessMarket.lockRequest(request, clientSignature);

        // Build a single-request fulfillment and submit via the legacy
        // fulfill(Fulfillment[], AssessorReceipt) entrypoint. That selector
        // is legacy-only — the new market declares fulfill(FulfillmentBatch[])
        // at a different selector — so the call falls through fallback() into
        // the legacy impl.
        ProofRequest[] memory requests = new ProofRequest[](1);
        requests[0] = request;
        bytes[] memory journals = new bytes[](1);
        journals[0] = APP_JOURNAL;
        (Fulfillment[] memory fills, AssessorReceipt memory assessorReceipt) =
            createFillsAndSubmitRoot(requests, journals, testProverAddress);

        vm.prank(testProverAddress);
        boundlessMarket.fulfill(fills, assessorReceipt);

        // Read state via the shared view: the new market's
        // requestIsFulfilled dispatcher entry executes; it queries the same
        // accounts/requestLocks slots the legacy impl just wrote to.
        assertTrue(
            boundlessMarket.requestIsFulfilled(request.id),
            "shared requestIsFulfilled must see the state written by the legacy fulfill"
        );
    }

    /// @dev Low-level helper to issue a call through the proxy. Used for
    ///      methods that are not declared on the legacy contract type the
    ///      test imports (e.g. immutable getters that exist only on the
    ///      legacy contract symbol, which is not what `boundlessMarket` is
    ///      cast to at the test's import-level type).
    function _callViaFallback(bytes memory data) internal returns (bytes memory) {
        (bool ok, bytes memory ret) = address(boundlessMarket).call(data);
        assertTrue(ok, "call via fallback should succeed");
        return ret;
    }
}
