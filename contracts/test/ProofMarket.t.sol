// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import {MessageHashUtils} from "@openzeppelin/contracts/utils/cryptography/MessageHashUtils.sol";
import {Test} from "forge-std/Test.sol";
import {Vm} from "forge-std/Vm.sol";
import {console2} from "forge-std/console2.sol";
import {ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {TestReceipt} from "risc0/../test/TestReceipt.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import {TestUtils} from "./TestUtils.sol";
import {UnsafeUpgrades, Upgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";

import {ProofMarket, MerkleProofish, AssessorJournal} from "../src/ProofMarket.sol";
import {
    Fulfillment,
    IProofMarket,
    Input,
    InputType,
    Offer,
    Predicate,
    PredicateType,
    ProvingRequest,
    Requirements
} from "../src/IProofMarket.sol";
import {ProofMarketLib} from "../src/ProofMarketLib.sol";
import {ProofMarketV2Test} from "./contracts/ProofMarketV2Test.sol";
import {RiscZeroSetVerifier} from "../src/RiscZeroSetVerifier.sol";

contract ProofMarketTest is Test {
    using ReceiptClaimLib for ReceiptClaim;
    using ProofMarketLib for Requirements;
    using ProofMarketLib for ProvingRequest;
    using ProofMarketLib for Offer;
    using TestUtils for RiscZeroSetVerifier;

    RiscZeroMockVerifier private verifier;
    ProofMarket private proofMarket;
    address private proxy;
    RiscZeroSetVerifier private setVerifier;
    mapping(uint256 => bool) private clientWallets;
    uint256 initialBalance;

    uint256 DEFAULT_BALANCE = 1000 ether;

    bytes4 MOCK_SELECTOR = bytes4(0);
    bytes32 internal APP_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000001;
    bytes32 internal SET_BUILDER_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000002;
    bytes32 internal ASSESSOR_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000003;

    bytes internal APP_JOURNAL = bytes("GUEST JOURNAL");
    ReceiptClaim internal APP_CLAIM = ReceiptClaimLib.ok(APP_IMAGE_ID, sha256(APP_JOURNAL));

    Requirements internal REQUIREMENTS = Requirements({
        imageId: APP_IMAGE_ID,
        predicate: Predicate({predicateType: PredicateType.DigestMatch, data: abi.encode(sha256(APP_JOURNAL))})
    });

    Vm.Wallet internal OWNER_WALLET = vm.createWallet("OWNER");
    Vm.Wallet internal PROVER_WALLET = vm.createWallet("PROVER");

    function setUp() public {
        vm.deal(OWNER_WALLET.addr, DEFAULT_BALANCE);
        vm.deal(PROVER_WALLET.addr, DEFAULT_BALANCE);

        vm.startPrank(OWNER_WALLET.addr);

        // Deploy the implementation contracts
        verifier = new RiscZeroMockVerifier(MOCK_SELECTOR);
        setVerifier = new RiscZeroSetVerifier(verifier, SET_BUILDER_IMAGE_ID, "https://set-builder.dev.null");

        // Deploy the UUPS proxy with the implementation
        proxy = Upgrades.deployUUPSProxy(
            "ProofMarket.sol:ProofMarket",
            abi.encodeCall(
                ProofMarket.initialize, (OWNER_WALLET.addr, setVerifier, ASSESSOR_IMAGE_ID, "https://assessor.dev.null")
            )
        );
        proofMarket = ProofMarket(proxy);

        vm.stopPrank();

        vm.prank(PROVER_WALLET.addr);
        proofMarket.deposit{value: DEFAULT_BALANCE}();

        for (uint256 i = 0; i < 5; i++) {
            createClient(i);
        }

        initialBalance = address(proofMarket).balance;

        // Verify that OWNER is the actual owner
        assertEq(proofMarket.owner(), OWNER_WALLET.addr, "OWNER address is not the contract owner after deployment");
    }

    // Utility function to check initial and final balance difference
    function checkProofMarketBalance() internal view {
        uint256 finalBalance = address(proofMarket).balance;
        console2.log("Initial balance:", initialBalance);
        console2.log("Final balance:", finalBalance);
        require(finalBalance == initialBalance, "Contract balance changed during the test");
    }

    function checkBurnedBalance(uint256 burnedBalance) internal view {
        uint256 finalBalance = address(proofMarket).balance;
        console2.log("Initial balance:", initialBalance);
        console2.log("Final balance:", finalBalance);
        require(finalBalance == initialBalance - burnedBalance, "Contract balance changed during the test");
    }

    // Creates a client account with the given index, gives it some Ether, and deposits from Ether in the market.
    function createClient(uint256 index) internal returns (Vm.Wallet memory) {
        Vm.Wallet memory wallet = vm.createWallet(string.concat("CLIENT_", vm.toString(index)));
        if (clientWallets[index]) {
            return wallet;
        }
        clientWallets[index] = true;
        vm.deal(wallet.addr, DEFAULT_BALANCE);
        vm.startPrank(wallet.addr);
        proofMarket.deposit{value: DEFAULT_BALANCE}();
        vm.stopPrank();
        return wallet;
    }

    function newRequest(Offer memory offer, address client, uint32 idx) internal view returns (ProvingRequest memory) {
        return ProvingRequest({
            id: ProofMarketLib.requestId(client, idx),
            requirements: REQUIREMENTS,
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: offer
        });
    }

    function defaultRequest(address client, uint32 idx) internal view returns (ProvingRequest memory) {
        return ProvingRequest({
            id: ProofMarketLib.requestId(client, idx),
            requirements: REQUIREMENTS,
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: Offer({
                minPrice: 1 ether,
                maxPrice: 2 ether,
                biddingStart: uint64(block.number),
                rampUpPeriod: uint32(10),
                timeout: type(uint32).max,
                lockinStake: 1 ether
            })
        });
    }

    function signRequest(Vm.Wallet memory wallet, ProvingRequest memory request) internal returns (bytes memory) {
        bytes32 structDigest =
            MessageHashUtils.toTypedDataHash(proofMarket.eip712DomainSeparator(), request.eip712Digest());
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(wallet, structDigest);
        return abi.encodePacked(r, s, v);
    }

    function publishRoot(bytes32 root) internal {
        setVerifier.submitMerkleRoot(
            root, verifier.mockProve(SET_BUILDER_IMAGE_ID, sha256(abi.encodePacked(SET_BUILDER_IMAGE_ID, root))).seal
        );
    }

    function fulfillRequest(ProvingRequest memory request, bytes memory journal)
        internal
        returns (Fulfillment memory, bytes memory assessorSeal)
    {
        ProvingRequest[] memory requests = new ProvingRequest[](1);
        requests[0] = request;
        bytes[] memory journals = new bytes[](1);
        journals[0] = journal;
        (Fulfillment[] memory fills, bytes memory seal) = fulfillRequestBatch(requests, journals);
        return (fills[0], seal);
    }

    function fulfillRequestBatch(ProvingRequest[] memory requests, bytes[] memory journals)
        internal
        returns (Fulfillment[] memory fills, bytes memory assessorSeal)
    {
        // initialize the fullfillments; one for each request;
        // the seal is filled in later, by calling fillInclusionProof
        fills = new Fulfillment[](requests.length);
        for (uint256 i = 0; i < requests.length; i++) {
            Fulfillment memory fill = Fulfillment({
                id: requests[i].id,
                imageId: requests[i].requirements.imageId,
                journal: journals[i],
                seal: bytes("")
            });
            fills[i] = fill;
        }

        // compute the assessor claim
        ReceiptClaim memory assessorClaim =
            TestUtils.mockAssessor(fills, ASSESSOR_IMAGE_ID, proofMarket.eip712DomainSeparator());
        // compute the batchRoot of the batch Merkle Tree (without the assessor)
        (bytes32 batchRoot, bytes32[][] memory tree) = TestUtils.mockSetBuilder(fills);
        // Join the batchRoot with the assessor digest.
        bytes32 root = MerkleProofish._hashPair(batchRoot, assessorClaim.digest());
        // submit the root to the set verifier
        publishRoot(root);
        // compute all the inclusion proofs for the fullfillments
        TestUtils.fillInclusionProofs(setVerifier, fills, assessorClaim.digest(), tree);
        // compute the assessor seal
        assessorSeal = TestUtils.mockAssessorSeal(setVerifier, batchRoot);

        return (fills, assessorSeal);
    }

    function testSubmitRequest() public {
        // Submit request
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = defaultRequest(client.addr, 1);
        bytes memory clientSignature = signRequest(client, request);

        // Submit the request with no funds
        // Expect the event to be emitted
        vm.expectEmit(true, true, false, true);
        emit IProofMarket.RequestSubmitted(request, clientSignature);
        vm.prank(client.addr);
        proofMarket.submitRequest(request, clientSignature);

        // Submit the request with funds
        // Expect the event to be emitted
        vm.expectEmit(true, true, false, true);
        emit IProofMarket.RequestSubmitted(request, clientSignature);

        vm.deal(client.addr, request.offer.maxPrice);
        vm.prank(client.addr);
        proofMarket.submitRequest{value: request.offer.maxPrice}(request, clientSignature);
    }

    function testLockin() public {
        // Submit request
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = defaultRequest(client.addr, 1);
        bytes memory clientSignature = signRequest(client, request);

        uint256 clientBalanceBefore = proofMarket.balanceOf(client.addr);
        console2.log("Client balance before:", clientBalanceBefore);
        uint256 proverBalanceBefore = proofMarket.balanceOf(PROVER_WALLET.addr);
        console2.log("Prover balance before:", proverBalanceBefore);

        // Expect the event to be emitted
        vm.expectEmit(true, true, false, true);
        emit IProofMarket.RequestLockedin(request.id, PROVER_WALLET.addr);
        vm.prank(PROVER_WALLET.addr);
        proofMarket.lockin(request, clientSignature);

        // Ensure the balances are correct
        assertEq(proofMarket.balanceOf(client.addr), clientBalanceBefore - 1 ether);
        assertEq(proofMarket.balanceOf(PROVER_WALLET.addr), proverBalanceBefore - 1 ether);

        // Verify the lockin
        assertTrue(proofMarket.requestIsLocked(request.id), "Request should be locked-in");

        checkProofMarketBalance();
    }

    function testLockinAlreadyLocked() public {
        // Submit request
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = defaultRequest(client.addr, 1);
        bytes memory clientSignature = signRequest(client, request);
        testLockin();
        // Attempt to lock in the request again
        // should revert with "RequestIsLocked({requestId: request.id})"
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.RequestIsLocked.selector, request.id));
        proofMarket.lockin(request, clientSignature);

        checkProofMarketBalance();
    }

    function testLockinBadClientSignature() public {
        // Submit request
        Vm.Wallet memory clientA = createClient(1);
        Vm.Wallet memory ClientB = createClient(2);
        ProvingRequest memory requestA = defaultRequest(clientA.addr, 1);
        ProvingRequest memory requestB = defaultRequest(clientA.addr, 2);

        // case: request signed by a different client
        // should revert with "Invalid client signature"
        bytes memory badClientSignature = signRequest(ClientB, requestA);
        vm.expectRevert("Invalid client signature");
        vm.prank(PROVER_WALLET.addr);
        proofMarket.lockin(requestA, badClientSignature);

        // case: client signed a different request
        // should revert with "Invalid client signature"
        badClientSignature = signRequest(clientA, requestB);
        vm.expectRevert("Invalid client signature");
        vm.prank(PROVER_WALLET.addr);
        proofMarket.lockin(requestA, badClientSignature);

        checkProofMarketBalance();
    }

    function testLockinNotEnoughFunds() public {
        // Submit request
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = defaultRequest(client.addr, 1);
        bytes memory clientSignature = signRequest(client, request);
        vm.prank(client.addr);
        proofMarket.withdraw(DEFAULT_BALANCE);

        // case: client does not have enough funds to cover for the lockin
        // should revert with "InsufficientBalance(address requester)"
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.InsufficientBalance.selector, client.addr));
        vm.prank(PROVER_WALLET.addr);
        proofMarket.lockin(request, clientSignature);

        vm.prank(client.addr);
        proofMarket.deposit{value: DEFAULT_BALANCE}();

        vm.startPrank(PROVER_WALLET.addr);
        proofMarket.withdraw(DEFAULT_BALANCE);
        // case: prover does not have enough funds to cover for the lockin stake
        // should revert with "InsufficientBalance(address requester)"
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.InsufficientBalance.selector, PROVER_WALLET.addr));
        proofMarket.lockin(request, clientSignature);
        vm.stopPrank();
    }

    function testLockinExpired() public {
        Offer memory offer = Offer({
            minPrice: 1 ether,
            maxPrice: 1 ether,
            biddingStart: uint64(0),
            rampUpPeriod: uint32(0),
            timeout: uint32(1),
            lockinStake: 10 ether
        });

        // Submit request
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = newRequest(offer, client.addr, 1);
        bytes memory clientSignature = signRequest(client, request);

        vm.roll(2);

        // Attempt to lock in the request after it has expired
        // should revert with "RequestIsExpired({requestId: request.id, deadline: deadline})"
        vm.expectRevert(
            abi.encodeWithSelector(IProofMarket.RequestIsExpired.selector, request.id, request.offer.deadline())
        );
        vm.prank(PROVER_WALLET.addr);
        proofMarket.lockin(request, clientSignature);

        checkProofMarketBalance();
    }

    function testLockinInvalidRequest1() public {
        Offer memory offer = Offer({
            minPrice: 2 ether,
            maxPrice: 1 ether,
            biddingStart: uint64(0),
            rampUpPeriod: uint32(0),
            timeout: uint32(1),
            lockinStake: 10 ether
        });

        // Submit request
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = newRequest(offer, client.addr, 1);
        bytes memory clientSignature = signRequest(client, request);

        // Attempt to lockin a request with maxPrice smaller than minPrice
        // should revert with "maxPrice cannot be smaller than minPrice"
        vm.expectRevert("maxPrice cannot be smaller than minPrice");
        vm.prank(PROVER_WALLET.addr);
        proofMarket.lockin(request, clientSignature);

        checkProofMarketBalance();
    }

    function testLockinInvalidRequest2() public {
        Offer memory offer = Offer({
            minPrice: 1 ether,
            maxPrice: 1 ether,
            biddingStart: uint64(0),
            rampUpPeriod: uint32(2),
            timeout: uint32(1),
            lockinStake: 10 ether
        });

        // Submit request
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = newRequest(offer, client.addr, 1);
        bytes memory clientSignature = signRequest(client, request);

        // Attempt to lockin a request with rampUpPeriod greater than timeout
        // should revert with "Request cannot expire before end of bidding period"
        vm.expectRevert("Request cannot expire before end of bidding period");
        vm.prank(PROVER_WALLET.addr);
        proofMarket.lockin(request, clientSignature);

        checkProofMarketBalance();
    }

    function _testFulfill(uint32 requestIdx) private {
        // Submit request
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = defaultRequest(client.addr, requestIdx);
        bytes memory clientSignature = signRequest(client, request);

        uint256 balanceBefore = proofMarket.balanceOf(PROVER_WALLET.addr);
        console2.log("Prover balance before:", balanceBefore);

        vm.startPrank(PROVER_WALLET.addr);
        proofMarket.lockin(request, clientSignature);
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL);
        proofMarket.fulfill(fill, assessorSeal);
        // console2.log("fulfill - Gas used:", vm.gasUsed());
        vm.stopPrank();

        // Check that the proof was submitted
        assertTrue(proofMarket.requestIsFulfilled(fill.id), "Request should have fulfilled status");

        uint256 balanceAfter = proofMarket.balanceOf(PROVER_WALLET.addr);
        console2.log("Prover balance after:", balanceAfter);
        assertEq(balanceBefore + 1 ether, balanceAfter);

        checkProofMarketBalance();
    }

    function testFulfill() public {
        _testFulfill(1);
    }

    function testFulfillLotsOfRequests() public {
        // Check that a single client can create many requests, with the full range of indices, and
        // complete the flow each time.
        for (uint32 idx = 0; idx < 512; idx++) {
            console2.log(idx);
            _testFulfill(idx);
        }
        console2.log(uint32(0xdeadbeef));
        _testFulfill(0xdeadbeef);
        console2.log(uint32(0xffffffff));
        _testFulfill(0xffffffff);
    }

    function testFulfillWithSig() public {
        Vm.Wallet memory client = createClient(1);

        ProvingRequest memory request = defaultRequest(client.addr, 3);

        bytes memory clientSignature = signRequest(client, request);
        bytes memory proverSignature = signRequest(PROVER_WALLET, request);

        uint256 balanceBefore = proofMarket.balanceOf(PROVER_WALLET.addr);
        console2.log("Prover balance before:", balanceBefore);

        // Note that this does not come from any particular address.
        proofMarket.lockinWithSig(request, clientSignature, proverSignature);
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL);
        proofMarket.fulfill(fill, assessorSeal);

        // Check that the proof was submitted
        assertTrue(proofMarket.requestIsFulfilled(fill.id), "Request should have fulfilled status");

        uint256 balanceAfter = proofMarket.balanceOf(PROVER_WALLET.addr);
        console2.log("Prover balance after:", balanceAfter);
        assertEq(balanceBefore + 1 ether, balanceAfter);

        checkProofMarketBalance();
    }

    function testFulfillBatchWithSig() public {
        // Provide a batch definition as an array of clients and how many requests each submits.
        uint256[5] memory batch = [uint256(1), 2, 1, 3, 1];
        uint256 batchSize = 0;
        for (uint256 i = 0; i < batch.length; i++) {
            batchSize += batch[i];
        }

        uint256 balanceBefore = proofMarket.balanceOf(PROVER_WALLET.addr);
        console2.log("Prover balance before:", balanceBefore);

        ProvingRequest[] memory requests = new ProvingRequest[](batchSize);
        bytes[] memory journals = new bytes[](batchSize);
        uint96 expectedRevenue = 0;
        uint256 idx = 0;
        for (uint256 i = 0; i < batch.length; i++) {
            Vm.Wallet memory client = createClient(i);

            for (uint256 j = 0; j < batch[i]; j++) {
                ProvingRequest memory request = defaultRequest(client.addr, uint32(j));

                bytes memory clientSignature = signRequest(client, request);
                bytes memory proverSignature = signRequest(PROVER_WALLET, request);

                // TODO: This is a fragile part of this test. It should be improved.
                uint96 desiredPrice = uint96(1.5 ether);
                vm.roll(request.offer.blockAtPrice(desiredPrice));
                expectedRevenue += desiredPrice;

                proofMarket.lockinWithSig(request, clientSignature, proverSignature);

                requests[idx] = request;
                journals[idx] = APP_JOURNAL;
                idx++;
            }
        }

        (Fulfillment[] memory fills, bytes memory assessorSeal) = fulfillRequestBatch(requests, journals);

        proofMarket.fulfillBatch(fills, assessorSeal);

        for (uint256 i = 0; i < fills.length; i++) {
            // Check that the proof was submitted
            assertTrue(proofMarket.requestIsFulfilled(fills[i].id), "Request should have fulfilled status");
        }

        uint256 balanceAfter = proofMarket.balanceOf(PROVER_WALLET.addr);
        console2.log("Prover balance after:", balanceAfter);
        assertEq(balanceBefore + expectedRevenue, balanceAfter);

        checkProofMarketBalance();
    }

    function testFulfillAlreadyFulfilled() public {
        // Submit request and fulfill it
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = defaultRequest(client.addr, 1);
        testFulfill();

        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL);
        // Attempt to fulfill a request already fulfilled
        // should revert with "RequestIsFulfilled({requestId: request.id})"
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.RequestIsFulfilled.selector, request.id));
        vm.prank(PROVER_WALLET.addr);
        proofMarket.fulfill(fill, assessorSeal);

        checkProofMarketBalance();
    }

    function testFulfillRequestNotLocked() public {
        // Attempt to prove a non-existent request
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = defaultRequest(client.addr, 1);
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL);

        // Attempt to fulfill a request not lockeed
        // should revert with "RequestIsNotLocked({requestId: request.id})"
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.RequestIsNotLocked.selector, request.id));
        vm.prank(PROVER_WALLET.addr);
        proofMarket.fulfill(fill, assessorSeal);

        checkProofMarketBalance();
    }

    function testFulfillfExpired() public {
        Offer memory offer = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            biddingStart: uint64(0),
            rampUpPeriod: uint32(0),
            timeout: uint32(1),
            lockinStake: 10 ether
        });
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = newRequest(offer, client.addr, 1);
        bytes memory clientSignature = signRequest(client, request);

        vm.startPrank(PROVER_WALLET.addr);
        proofMarket.lockin(request, clientSignature);
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL);

        vm.roll(2);

        // Attempt to fulfill an expired request
        // should revert with "RequestIsExpired({requestId: request.id, deadline: deadline})"
        vm.expectRevert(
            abi.encodeWithSelector(IProofMarket.RequestIsExpired.selector, request.id, request.offer.deadline())
        );
        proofMarket.fulfill(fill, assessorSeal);
        vm.stopPrank();

        checkProofMarketBalance();
    }

    function testSlash() public {
        Offer memory offer = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            biddingStart: uint64(0),
            rampUpPeriod: uint32(0),
            timeout: uint32(1),
            lockinStake: 10 ether
        });
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = newRequest(offer, client.addr, 1);

        testFulfillfExpired();

        // Slash the request
        vm.expectEmit(true, true, false, true);
        emit IProofMarket.LockinStakeBurned(request.id, request.offer.lockinStake);
        proofMarket.slash(request.id);

        uint256 clientBalance = proofMarket.balanceOf(client.addr);
        console2.log("Client balance after slash:", clientBalance);
        assertEq(clientBalance, DEFAULT_BALANCE);

        uint256 proverBalance = proofMarket.balanceOf(PROVER_WALLET.addr);
        console2.log("Prover balance after slash:", proverBalance);
        assertEq(proverBalance, DEFAULT_BALANCE - offer.lockinStake);

        checkBurnedBalance(request.offer.lockinStake);
    }

    function testSlashInvalidRequestID() public {
        // Attempt to slash an invalid request ID
        // should revert with "RequestIsNotLocked({requestId: request.id})"
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.RequestIsNotLocked.selector, 0xa));
        proofMarket.slash(0xa);

        checkProofMarketBalance();
    }

    function testSlashNotExpired() public {
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = defaultRequest(client.addr, 1);
        testLockin();

        // Attempt to slash a request not expired
        // should revert with "RequestIsNotExpired({requestId: request.id,  deadline: deadline})"
        vm.expectRevert(
            abi.encodeWithSelector(IProofMarket.RequestIsNotExpired.selector, request.id, request.offer.deadline())
        );
        proofMarket.slash(request.id);

        checkProofMarketBalance();
    }

    function testSlashFulfilled() public {
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = defaultRequest(client.addr, 1);
        testFulfill();

        // Attempt to slash a fulfilled request
        // should revert with "RequestIsFulfilled({requestId: request.id})"
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.RequestIsFulfilled.selector, request.id));
        proofMarket.slash(request.id);

        checkProofMarketBalance();
    }

    function testSlashSlash() public {
        Offer memory offer = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            biddingStart: uint64(0),
            rampUpPeriod: uint32(0),
            timeout: uint32(1),
            lockinStake: 10 ether
        });
        Vm.Wallet memory client = createClient(1);
        ProvingRequest memory request = newRequest(offer, client.addr, 1);

        testSlash();

        // Attempt to slash a request twice
        // should revert with "RequestAlreadySlashed({requestId: request.id})"
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.RequestAlreadySlashed.selector, request.id));
        proofMarket.slash(request.id);

        checkBurnedBalance(request.offer.lockinStake);
    }

    function newBatch(uint256 batchSize) internal returns (ProvingRequest[] memory requests, bytes[] memory journals) {
        requests = new ProvingRequest[](batchSize);
        journals = new bytes[](batchSize);
        Vm.Wallet[5] memory clients;
        for (uint256 j = 0; j < 5; j++) {
            clients[j] = createClient(j);
        }
        for (uint256 i = 0; i < batchSize; i++) {
            Vm.Wallet memory client = clients[i % 5];
            ProvingRequest memory request = defaultRequest(client.addr, uint32(i / 5));
            bytes memory clientSignature = signRequest(client, request);
            vm.prank(PROVER_WALLET.addr);
            proofMarket.lockin(request, clientSignature);
            requests[i] = request;
            journals[i] = APP_JOURNAL;
        }
    }

    function benchFulfillBatch(uint256 batchSize) public {
        (ProvingRequest[] memory requests, bytes[] memory journals) = newBatch(batchSize);
        (Fulfillment[] memory fills, bytes memory assessorSeal) = fulfillRequestBatch(requests, journals);

        uint256 gasBefore = gasleft();
        proofMarket.fulfillBatch(fills, assessorSeal);
        uint256 gasAfter = gasleft();
        // Calculate the gas used
        uint256 gasUsed = gasBefore - gasAfter;
        console2.log("fulfillBatch - Gas used:", gasUsed);

        for (uint256 j = 0; j < fills.length; j++) {
            assertTrue(proofMarket.requestIsFulfilled(fills[j].id), "Request should have fulfilled status");
        }
    }

    // Benchmark fulfillBatch with different batch sizes
    // use the following command to run the benchmark:
    // forge test -vv --match-test "testBenchFulfillBatch"

    function testBenchFulfillBatch1() public {
        benchFulfillBatch(1);
    }

    function testBenchFulfillBatch2() public {
        benchFulfillBatch(2);
    }

    function testBenchFulfillBatch4() public {
        benchFulfillBatch(4);
    }

    function testBenchFulfillBatch8() public {
        benchFulfillBatch(8);
    }

    function testBenchFulfillBatch16() public {
        benchFulfillBatch(16);
    }

    function testBenchFulfillBatch32() public {
        benchFulfillBatch(32);
    }

    function testBenchFulfillBatch64() public {
        benchFulfillBatch(64);
    }

    function testBenchFulfillBatch128() public {
        benchFulfillBatch(128);
    }

    function testProcessTree2() public pure {
        bytes32[] memory leaves = new bytes32[](2);
        leaves[0] = 0x6a428060b5d51f04583182f2ff1b565f9db661da12ee7bdc003e9ab6d5d91ba9;
        leaves[1] = 0x6a428060b5d51f04583182f2ff1b565f9db661da12ee7bdc003e9ab6d5d91ba9;

        bytes32 root = MerkleProofish.processTree(leaves);
        assertEq(root, 0x5032880539b5d039d4a4a8042745c9ad14934c96b76d7e61ea03550e29b234af);
    }

    function testProcessTree3() public pure {
        bytes32[] memory leaves = new bytes32[](3);
        leaves[0] = 0x6a428060b5d51f04583182f2ff1b565f9db661da12ee7bdc003e9ab6d5d91ba9;
        leaves[1] = 0x6a428060b5d51f04583182f2ff1b565f9db661da12ee7bdc003e9ab6d5d91ba9;
        leaves[2] = 0x6a428060b5d51f04583182f2ff1b565f9db661da12ee7bdc003e9ab6d5d91ba9;

        bytes32 root = MerkleProofish.processTree(leaves);
        assertEq(root, 0xe004c72e4cb697fa97669508df099edbc053309343772a25e56412fc7db8ebef);
    }

    function testUpgradeability() public {
        address implAddressV1 = Upgrades.getImplementationAddress(proxy);
        vm.startPrank(OWNER_WALLET.addr);
        // Deploy a new implementation of the same contract
        Upgrades.upgradeProxy(proxy, "ProofMarketV2Test.sol:ProofMarketV2Test", "", OWNER_WALLET.addr);
        vm.stopPrank();
        address implAddressV2 = Upgrades.getImplementationAddress(proxy);
        assertFalse(implAddressV2 == implAddressV1);

        (bytes32 imageID, string memory imageUrl) = proofMarket.imageInfo();
        assertEq(imageID, ASSESSOR_IMAGE_ID, "Image ID should be the same after upgrade");
        assertEq(imageUrl, "https://assessor.dev.null", "Image URL should be the same after upgrade");
    }

    function testUnsafeUpgrade() public {
        vm.startPrank(OWNER_WALLET.addr);
        proxy = UnsafeUpgrades.deployUUPSProxy(
            address(new ProofMarket()),
            abi.encodeCall(
                ProofMarket.initialize, (OWNER_WALLET.addr, setVerifier, ASSESSOR_IMAGE_ID, "https://assessor.dev.null")
            )
        );
        proofMarket = ProofMarket(proxy);
        address implAddressV1 = UnsafeUpgrades.getImplementationAddress(proxy);

        UnsafeUpgrades.upgradeProxy(proxy, address(new ProofMarket()), "", OWNER_WALLET.addr);
        vm.stopPrank();
        address implAddressV2 = UnsafeUpgrades.getImplementationAddress(proxy);

        assertFalse(implAddressV2 == implAddressV1);

        (bytes32 imageID, string memory imageUrl) = proofMarket.imageInfo();
        assertEq(imageID, ASSESSOR_IMAGE_ID, "Image ID should be the same after upgrade");
        assertEq(imageUrl, "https://assessor.dev.null", "Image URL should be the same after upgrade");
    }

    function testTransferOwnership() public {
        address newOwner = vm.createWallet("NEW_OWNER").addr;
        vm.prank(OWNER_WALLET.addr);
        proofMarket.transferOwnership(newOwner);

        vm.prank(newOwner);
        proofMarket.acceptOwnership();

        assertEq(proofMarket.owner(), newOwner, "Owner should be changed");
    }
}
