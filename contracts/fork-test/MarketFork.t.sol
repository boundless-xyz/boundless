// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {IAccessControl} from "@openzeppelin/contracts/access/IAccessControl.sol";
import {Initializable} from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";

import {SunsetTransaction} from "../scripts/sunset/SunsetTransaction.sol";
import {MarketSunset} from "../scripts/sunset/MarketSunset.sol";
import {RuntimeCheck} from "../scripts/sunset/RuntimeCheck.sol";

import {SafeFork} from "./SafeFork.sol";
import {BoundlessMarket} from "boundless-market/BoundlessMarket.sol";
import {IBoundlessRouter} from "../src/router/interfaces/IBoundlessRouter.sol";
import {IBoundlessMarket as ILegacyMarket} from "boundless-market-legacy/IBoundlessMarketLegacy.sol";
import {ProofRequest} from "../src/types/ProofRequest.sol";
import {ProofRequestLibrary} from "../src/types/ProofRequest.sol";
import {RequestId} from "../src/types/RequestId.sol";
import {FulfillmentBatch} from "../src/types/FulfillmentBatch.sol";
import {ProofRequestBatch} from "../src/types/ProofRequestBatch.sol";
import {Fulfillment as LegacyFulfillment} from "boundless-market-legacy/types/Fulfillment.sol";
import {AssessorReceipt} from "boundless-market-legacy/types/AssessorReceipt.sol";
import {Sunset} from "../src/libraries/Sunset.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

contract MarketForkTest is SafeFork {
    using ProofRequestLibrary for ProofRequest;
    BoundlessMarket market;
    address safe;
    address oldImplementation;
    address candidate;
    SunsetTransaction.Transaction transaction;
    address user;
    IERC20 collateral;
    address router;
    address legacy;
    bytes32 baselineHash;

    function setUp() public {
        // Fail rather than skip if this is not the explicitly pinned production fork.
        if (block.chainid == 8453) {
            assertEq(block.number, vm.envOr("SUNSET_BLOCK_NUMBER", uint256(53113412)));
            baselineHash = 0x857c1fb15b2673b0434ea6ce56800e7fda921baa1e29c6fd0f68b4e04bdccda6;
            market = BoundlessMarket(payable(0xFd152dADc5183870710FE54f939Eae3aB9F0fE82));
            safe = 0xB04D1a222789A76E74168a919b43b20F66e24f0b;
            oldImplementation = 0xBf3b740FaF258F8E4ab214702a96918ab739a686;
        } else {
            assertEq(block.chainid, 167000, "use Base or Taiko fork");
            assertEq(block.number, vm.envOr("SUNSET_BLOCK_NUMBER", uint256(13186463)));
            baselineHash = 0x894769a576b0342d1a34c8a38db498320e054dab89ca9ac55d013de95331a0b7;
            market = BoundlessMarket(payable(0xb3f5C7B4379052EADE8C7f3fa6da37Fb871DA28B));
            safe = 0x8E537AAeddbC98a9a095397aDb7c0d738458Cc8b;
            oldImplementation = 0x0824b650350CC2F510aa22e061a917524945b78c;
        }
        assertEq(implementation(address(market)), oldImplementation, "unexpected baseline");
        assertEq(oldImplementation.codehash, baselineHash, "unexpected runtime code");
        assertTrue(market.hasRole(market.ADMIN_ROLE(), safe));
        router = address(market.ROUTER());
        legacy = market.LEGACY_IMPL();
        collateral = IERC20(market.COLLATERAL_TOKEN_CONTRACT());
        (transaction, candidate) = MarketSunset.prepare(vm.envOr("SUNSET_IMPL", address(0)));
        user = vm.addr(0xabc123);
        vm.deal(user, 10 ether);
        vm.prank(user);
        market.deposit{value: 2 ether}();
        // Only this test account is seeded; execution uses the real collateral token.
        deal(address(collateral), user, 3 ether);
        vm.startPrank(user);
        collateral.approve(address(market), 3 ether);
        market.depositCollateral(3 ether);
        vm.stopPrank();
    }

    function upgrade() internal {
        uint256 etherBalance = address(market).balance;
        uint256 tokens = collateral.balanceOf(address(market));
        vm.record();
        executeSafe(transaction.safe, transaction.to, transaction.data, transaction.operation, true);
        assertOnlyImplementationWritten(address(market));
        assertEq(implementation(address(market)), candidate);
        assertEq(address(market.ROUTER()), router);
        assertEq(market.LEGACY_IMPL(), legacy);
        assertEq(market.COLLATERAL_TOKEN_CONTRACT(), address(collateral));
        assertEq(address(market).balance, etherBalance);
        assertEq(collateral.balanceOf(address(market)), tokens);
        assertEq(market.balanceOf(user), 2 ether);
        assertEq(market.balanceOfCollateral(user), 3 ether);
        assertTrue(market.hasRole(market.ADMIN_ROLE(), safe));
    }

    function request() internal view returns (ProofRequest memory r, bytes memory signature) {
        r.id = RequestId.wrap(uint256(uint160(user)) << 32 | 42);
        r.offer.minPrice = 1 ether;
        r.offer.maxPrice = 1 ether;
        r.offer.rampUpStart = uint64(T - 100);
        r.offer.lockTimeout = 3600;
        r.offer.timeout = 7200;
        r.offer.lockCollateral = 1 ether;
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", market.eip712DomainSeparator(), r.eip712Digest()));
        (uint8 v, bytes32 sigR, bytes32 sigS) = vm.sign(0xabc123, digest);
        signature = abi.encodePacked(sigR, sigS, v);
    }

    function testForkSafeUpgradeAndCutoff() public {
        upgrade();
        vm.warp(T - 1);
        (ProofRequest memory r, bytes memory signature) = request();
        vm.startPrank(user);
        market.submitRequest(r, signature);
        market.lockRequest(r, signature);
        vm.stopPrank();
        assertTrue(market.requestIsLocked(r.id));
        for (uint256 i; i < 2; ++i) {
            vm.warp(T + i);
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            market.submitRequest(r, signature);
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            market.lockRequest(r, signature);
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            market.lockRequestWithSignature(r, signature, "");
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            market.priceRequest(r, signature);
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            market.commitFulfillment(bytes32(uint256(1)));
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            market.submitRoot(address(1), bytes32(0), "");
            FulfillmentBatch[] memory batches = new FulfillmentBatch[](1);
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            market.fulfill(batches);
            LegacyFulfillment[] memory fills = new LegacyFulfillment[](1);
            AssessorReceipt memory receipt;
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            ILegacyMarket(address(market)).fulfill(fills, receipt);
        }
    }

    function testForkWithdrawalsReadsAndRollback() public {
        upgrade();
        vm.warp(T);
        ILegacyMarket(address(market)).imageInfo();
        vm.startPrank(user);
        market.withdraw(2 ether);
        market.withdrawCollateral(3 ether);
        vm.stopPrank();
        assertEq(market.balanceOf(user), 0);
        assertEq(market.balanceOfCollateral(user), 0);
        assertEq(collateral.balanceOf(user), 3 ether);
        // Rehearse storage-compatible rollback and restore the sunset within this local test.
        executeSafe(
            safe, address(market), abi.encodeCall(market.upgradeToAndCall, (oldImplementation, bytes(""))), 0, true
        );
        assertEq(implementation(address(market)), oldImplementation);
        assertEq(market.balanceOf(user), 0);
        executeSafe(transaction.safe, transaction.to, transaction.data, transaction.operation, true);
        assertEq(implementation(address(market)), candidate);
    }

    function testForkUnauthorizedUpgradeAndReinitializationFail() public {
        bytes memory unauthorized =
            abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, user, market.ADMIN_ROLE());
        vm.prank(user);
        vm.expectRevert(unauthorized);
        market.upgradeToAndCall(candidate, "");
        assertEq(implementation(address(market)), oldImplementation);
        upgrade();
        vm.prank(user);
        vm.expectRevert(unauthorized);
        market.upgradeToAndCall(oldImplementation, "");
        assertEq(implementation(address(market)), candidate);
        vm.expectRevert(Initializable.InvalidInitialization.selector);
        market.initialize(user);
        vm.expectRevert(Initializable.InvalidInitialization.selector);
        BoundlessMarket(payable(candidate)).initialize(user);
    }

    function checkCandidate(address value) external {
        MarketSunset.prepare(value);
    }

    function testForkCandidateRuntimeValidation() public {
        this.checkCandidate(candidate);
        vm.expectRevert(abi.encodeWithSelector(RuntimeCheck.RuntimeMismatch.selector, oldImplementation));
        this.checkCandidate(oldImplementation);
        vm.expectRevert(abi.encodeWithSelector(RuntimeCheck.RuntimeMismatch.selector, address(1)));
        this.checkCandidate(address(1));
        address wrong = address(new BoundlessMarket(IBoundlessRouter(router), address(collateral), address(market)));
        vm.expectRevert(abi.encodeWithSelector(RuntimeCheck.RuntimeMismatch.selector, wrong));
        this.checkCandidate(wrong);
    }

    function checkUndeployedReferenceAddress() external {
        address predicted = vm.computeCreateAddress(address(this), vm.getNonce(address(this)));
        assertEq(predicted.code.length, 0);
        MarketSunset.prepare(predicted);
    }

    function testForkRejectsUndeployedReferenceAddress() public {
        vm.expectPartialRevert(RuntimeCheck.RuntimeMismatch.selector);
        this.checkUndeployedReferenceAddress();
    }
}
