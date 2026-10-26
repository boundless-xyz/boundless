// Copyright 2026 Boundless Foundation, Inc.
// SPDX-License-Identifier: BUSL-1.1
pragma solidity ^0.8.26;

import "forge-std/Test.sol";
import {BoundlessMarket} from "boundless-market/BoundlessMarket.sol";
import {BoundlessMarketOriginal} from "sunset-reference/";
import {IBoundlessMarket} from "../../src/IBoundlessMarket.sol";
import {IBoundlessMarket as ILegacyMarket} from "boundless-market-legacy/IBoundlessMarketLegacy.sol";
import {BoundlessMarket as LegacyMarket} from "boundless-market-legacy/BoundlessMarketLegacy.sol";
import {ProofRequest} from "../../src/types/ProofRequest.sol";
import {ProofRequest as LegacyRequest} from "boundless-market-legacy/types/ProofRequest.sol";
import {Fulfillment as LegacyFulfillment} from "boundless-market-legacy/types/Fulfillment.sol";
import {AssessorReceipt} from "boundless-market-legacy/types/AssessorReceipt.sol";
import {ProofRequestBatch} from "../../src/types/ProofRequestBatch.sol";
import {FulfillmentBatch} from "../../src/types/FulfillmentBatch.sol";
import {IBoundlessRouter} from "../../src/router/interfaces/IBoundlessRouter.sol";
import {Sunset} from "../../src/libraries/Sunset.sol";
import {IRiscZeroVerifier, Receipt} from "risc0/IRiscZeroVerifier.sol";
import {ProofRequestLibrary as LegacyRequestLibrary} from "boundless-market-legacy/types/ProofRequest.sol";
import {RequestId as LegacyRequestId} from "boundless-market-legacy/types/RequestId.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {HitPoints} from "../../src/HitPoints.sol";

contract MarketSunsetVerifier {
    function verify(bytes calldata, bytes32, bytes32) external pure {}
    function verifyIntegrity(Receipt calldata) external pure {}
    function submitMerkleRoot(bytes32, bytes calldata) external pure {}
}

contract ExtraLegacyMethod {
    function extraOperation() external pure returns (uint256) {
        return 1;
    }
}

contract MarketSunsetTest is Test {
    using LegacyRequestLibrary for LegacyRequest;
    uint256 constant T = 1793836800;
    BoundlessMarket market;
    LegacyMarket legacy;
    HitPoints collateral;
    address admin = makeAddr("admin");
    address user = makeAddr("user");
    address implementation;
    MarketSunsetVerifier verifier;

    function setUp() public {
        vm.warp(T - 2 days);
        collateral = new HitPoints(address(this));
        collateral.grantMinterRole(address(this));
        verifier = new MarketSunsetVerifier();
        legacy = new LegacyMarket(
            IRiscZeroVerifier(address(verifier)),
            IRiscZeroVerifier(address(verifier)),
            bytes32(uint256(1)),
            bytes32(0),
            0,
            address(collateral)
        );
        implementation =
            address(new BoundlessMarket(IBoundlessRouter(address(0x2222)), address(collateral), address(legacy)));
        BoundlessMarketOriginal original =
            new BoundlessMarketOriginal(IBoundlessRouter(address(0x2222)), address(collateral), address(legacy));
        market = BoundlessMarket(
            payable(address(
                    new ERC1967Proxy(address(original), abi.encodeCall(BoundlessMarketOriginal.initialize, (admin)))
                ))
        );
        collateral.grantAuthorizedTransferRole(address(market));
        vm.deal(user, 10 ether);
        vm.prank(user);
        market.deposit{value: 2 ether}();
        collateral.mint(user, 3 ether);
        vm.startPrank(user);
        collateral.approve(address(market), 3 ether);
        market.depositCollateral(3 ether);
        vm.stopPrank();
        vm.prank(admin);
        market.upgradeToAndCall(implementation, "");
    }

    function testCurrentAndLegacyPathsOpenBeforeCutoff() public {
        vm.warp(T - 1);
        assertNotSunset(currentCalls());
        assertNotSunset(legacyCalls());
        FulfillmentBatch[] memory batches = new FulfillmentBatch[](0);
        market.fulfill(batches);
        assertEq(market.balanceOf(user), 2 ether);
    }

    function testAllCurrentAndLegacyPathsRevertAtCutoff() public {
        vm.warp(T);
        assertSunset(currentCalls());
        assertSunset(legacyCalls());
    }

    function testAllCurrentAndLegacyPathsRevertAfterCutoff() public {
        vm.warp(T + 1);
        assertSunset(currentCalls());
        assertSunset(legacyCalls());
    }

    function assertSunset(bytes[] memory calls) internal {
        for (uint256 i; i < calls.length; ++i) {
            vm.recordLogs();
            (bool success, bytes memory data) = address(market).call(calls[i]);
            assertFalse(success, string.concat("call succeeded: ", vm.toString(i)));
            assertEq(
                data,
                abi.encodeWithSelector(Sunset.ProtocolSunset.selector),
                string.concat("wrong revert: ", vm.toString(i))
            );
            assertEq(vm.getRecordedLogs().length, 0);
        }
        assertEq(market.balanceOf(user), 2 ether);
        assertEq(market.balanceOfCollateral(user), 3 ether);
    }

    function assertNotSunset(bytes[] memory calls) internal {
        for (uint256 i; i < calls.length; ++i) {
            (, bytes memory data) = address(market).call(calls[i]);
            assertNotEq(data, abi.encodeWithSelector(Sunset.ProtocolSunset.selector));
        }
    }

    function testWithdrawalsLegacyReadsAndAdminUpgradeRemainAvailable() public {
        vm.warp(T);
        assertEq(market.balanceOf(user), 2 ether);
        assertEq(market.balanceOfCollateral(user), 3 ether);
        (bytes32 imageId,) = ILegacyMarket(address(market)).imageInfo();
        assertEq(imageId, bytes32(uint256(1)));
        vm.startPrank(user);
        market.withdraw(2 ether);
        market.withdrawCollateral(3 ether);
        vm.stopPrank();
        assertEq(market.balanceOf(user), 0);
        assertEq(market.balanceOfCollateral(user), 0);
        assertEq(collateral.balanceOf(user), 3 ether);
        vm.expectRevert();
        vm.prank(user);
        market.upgradeToAndCall(implementation, "");
        vm.prank(admin);
        market.upgradeToAndCall(implementation, "");
        assertTrue(market.hasRole(market.ADMIN_ROLE(), admin));
    }

    function testValidLegacyLockAndNonemptyFulfillmentBeforeCutoff() public {
        vm.warp(T - 1);
        (LegacyRequest memory request, bytes memory signature) = signedLegacyRequest();
        vm.prank(user);
        ILegacyMarket(address(market)).lockRequest(request, signature);
        assertTrue(ILegacyMarket(address(market)).requestIsLocked(request.id));
        (LegacyFulfillment[] memory fills, AssessorReceipt memory receipt) = legacyProof(request);
        ILegacyMarket(address(market)).submitRootAndFulfill(address(verifier), bytes32(uint256(1)), "", fills, receipt);
        assertTrue(ILegacyMarket(address(market)).requestIsFulfilled(request.id));
        assertEq(market.balanceOf(user), 3 ether);
        assertEq(market.balanceOfCollateral(user), 3 ether);
    }

    function testValidLegacyPricingAndNonemptyFulfillmentBeforeCutoff() public {
        vm.warp(T - 1);
        (LegacyRequest memory request, bytes memory signature) = signedLegacyRequest();
        LegacyRequest[] memory requests = new LegacyRequest[](1);
        requests[0] = request;
        bytes[] memory signatures = new bytes[](1);
        signatures[0] = signature;
        (LegacyFulfillment[] memory fills, AssessorReceipt memory receipt) = legacyProof(request);
        ILegacyMarket(address(market)).priceAndFulfill(requests, signatures, fills, receipt);
        assertTrue(ILegacyMarket(address(market)).requestIsFulfilled(request.id));
        assertEq(market.balanceOf(user), 3 ether);
    }

    function signedLegacyRequest() internal returns (LegacyRequest memory request, bytes memory signature) {
        uint256 key = 0x999;
        address client = vm.addr(key);
        vm.deal(client, 2 ether);
        vm.prank(client);
        market.deposit{value: 2 ether}();
        request.id = LegacyRequestId.wrap((uint256(uint160(client)) << 32) | 1);
        request.offer.minPrice = 1 ether;
        request.offer.maxPrice = 1 ether;
        request.offer.rampUpStart = uint64(T - 10);
        request.offer.lockTimeout = 3600;
        request.offer.timeout = 7200;
        request.offer.lockCollateral = 1 ether;
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", market.eip712DomainSeparator(), request.eip712Digest()));
        (uint8 v, bytes32 r, bytes32 sigS) = vm.sign(key, digest);
        signature = abi.encodePacked(r, sigS, v);
    }

    function legacyProof(LegacyRequest memory request)
        internal
        view
        returns (LegacyFulfillment[] memory fills, AssessorReceipt memory receipt)
    {
        fills = new LegacyFulfillment[](1);
        fills[0].id = request.id;
        fills[0].requestDigest =
            keccak256(abi.encodePacked("\x19\x01", market.eip712DomainSeparator(), request.eip712Digest()));
        fills[0].claimDigest = bytes32(uint256(1));
        receipt.prover = user;
    }

    function testUnknownLegacySelectorFailsClosedAfterCutoff() public {
        BoundlessMarket replacement = new BoundlessMarket(
            IBoundlessRouter(address(0x2222)), address(collateral), address(new ExtraLegacyMethod())
        );
        vm.prank(admin);
        market.upgradeToAndCall(address(replacement), "");
        (bool success,) = address(market).call(abi.encodeCall(ExtraLegacyMethod.extraOperation, ()));
        assertTrue(success);
        vm.warp(T);
        vm.expectRevert(Sunset.ProtocolSunset.selector);
        ExtraLegacyMethod(address(market)).extraOperation();
    }

    function testKnownLegacyReadsAndAdminSetterRemainAvailable() public {
        vm.warp(T);
        vm.prank(admin);
        (bool success,) = address(market).call(abi.encodeWithSignature("setImageUrl(string)", "archived"));
        assertTrue(success);
        (, string memory url) = ILegacyMarket(address(market)).imageInfo();
        assertEq(url, "archived");
        LegacyRequest memory request;
        (LegacyFulfillment[] memory fills, AssessorReceipt memory receipt) = legacyProof(request);
        ILegacyMarket(address(market)).verifyDelivery(fills, receipt);
        assertEq(ILegacyMarket(address(market)).requestIsLocked(request.id), false);
        assertEq(ILegacyMarket(address(market)).requestIsFulfilled(request.id), false);
        assertEq(ILegacyMarket(address(market)).requestIsSlashed(request.id), false);
        (success,) = address(market).call("");
        assertFalse(success);
        string[6] memory getters = [
            "VERIFIER()",
            "APPLICATION_VERIFIER()",
            "ASSESSOR_ID()",
            "DEPRECATED_ASSESSOR_ID()",
            "DEPRECATED_ASSESSOR_EXPIRES_AT()",
            "DEFAULT_MAX_GAS_FOR_VERIFY()"
        ];
        for (uint256 i; i < getters.length; ++i) {
            bytes memory data;
            (success, data) = address(market).staticcall(abi.encodeWithSignature(getters[i]));
            assertTrue(success);
            assertEq(data.length, 32);
        }
    }

    function currentCalls() internal pure returns (bytes[] memory calls) {
        ProofRequest memory request;
        ProofRequestBatch[] memory requests = new ProofRequestBatch[](0);
        FulfillmentBatch[] memory batches = new FulfillmentBatch[](0);
        calls = new bytes[](14);
        calls[0] = abi.encodeCall(IBoundlessMarket.submitRequest, (request, ""));
        calls[1] = abi.encodeCall(IBoundlessMarket.lockRequest, (request, ""));
        calls[2] = abi.encodeCall(IBoundlessMarket.lockRequestWithSignature, (request, "", ""));
        calls[3] = abi.encodeCall(IBoundlessMarket.priceRequest, (request, ""));
        calls[4] = abi.encodeCall(IBoundlessMarket.commitFulfillment, (bytes32(0)));
        calls[5] = abi.encodeCall(IBoundlessMarket.fulfill, (batches));
        calls[6] = abi.encodeCall(IBoundlessMarket.fulfillAndWithdraw, (batches));
        calls[7] = abi.encodeCall(IBoundlessMarket.priceAndFulfill, (requests, batches));
        calls[8] = abi.encodeCall(IBoundlessMarket.priceAndFulfillAndWithdraw, (requests, batches));
        calls[9] = abi.encodeCall(IBoundlessMarket.submitRoot, (address(0x1111), bytes32(0), ""));
        calls[10] = abi.encodeCall(IBoundlessMarket.submitRootAndFulfill, (address(0x1111), bytes32(0), "", batches));
        calls[11] = abi.encodeCall(
            IBoundlessMarket.submitRootAndFulfillAndWithdraw, (address(0x1111), bytes32(0), "", batches)
        );
        calls[12] = abi.encodeCall(
            IBoundlessMarket.submitRootAndPriceAndFulfill, (address(0x1111), bytes32(0), "", requests, batches)
        );
        calls[13] = abi.encodeCall(
            IBoundlessMarket.submitRootAndPriceAndFulfillAndWithdraw,
            (address(0x1111), bytes32(0), "", requests, batches)
        );
    }

    function legacyCalls() internal pure returns (bytes[] memory calls) {
        LegacyRequest memory request;
        LegacyRequest[] memory requests = new LegacyRequest[](0);
        bytes[] memory signatures = new bytes[](0);
        LegacyFulfillment[] memory fills = new LegacyFulfillment[](0);
        AssessorReceipt memory receipt;
        calls = new bytes[](13);
        calls[0] = abi.encodeCall(ILegacyMarket.submitRequest, (request, ""));
        calls[1] = abi.encodeCall(ILegacyMarket.lockRequest, (request, ""));
        calls[2] = abi.encodeCall(ILegacyMarket.lockRequestWithSignature, (request, "", ""));
        calls[3] = abi.encodeCall(ILegacyMarket.priceRequest, (request, ""));
        calls[4] = abi.encodeCall(ILegacyMarket.fulfill, (fills, receipt));
        calls[5] = abi.encodeCall(ILegacyMarket.fulfillAndWithdraw, (fills, receipt));
        calls[6] = abi.encodeCall(ILegacyMarket.priceAndFulfill, (requests, signatures, fills, receipt));
        calls[7] = abi.encodeCall(ILegacyMarket.priceAndFulfillAndWithdraw, (requests, signatures, fills, receipt));
        calls[8] = abi.encodeCall(ILegacyMarket.submitRoot, (address(0x1111), bytes32(0), ""));
        calls[9] = abi.encodeCall(ILegacyMarket.submitRootAndFulfill, (address(0x1111), bytes32(0), "", fills, receipt));
        calls[10] = abi.encodeCall(
            ILegacyMarket.submitRootAndFulfillAndWithdraw, (address(0x1111), bytes32(0), "", fills, receipt)
        );
        calls[11] = abi.encodeCall(
            ILegacyMarket.submitRootAndPriceAndFulfill,
            (address(0x1111), bytes32(0), "", requests, signatures, fills, receipt)
        );
        calls[12] = abi.encodeCall(
            ILegacyMarket.submitRootAndPriceAndFulfillAndWithdraw,
            (address(0x1111), bytes32(0), "", requests, signatures, fills, receipt)
        );
    }
}
