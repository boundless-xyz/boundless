// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";
import {MessageHashUtils} from "@openzeppelin/contracts/utils/cryptography/MessageHashUtils.sol";
import {Test} from "forge-std/Test.sol";
import {Vm} from "forge-std/Vm.sol";
import {console2} from "forge-std/console2.sol";
import {ReceiptClaim, ReceiptClaimLib, VerificationFailed} from "risc0/IRiscZeroVerifier.sol";
import {TestReceipt} from "risc0/../test/TestReceipt.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import {TestUtils} from "./TestUtils.sol";
import {IERC1967} from "@openzeppelin/contracts/interfaces/IERC1967.sol";
import {UnsafeUpgrades, Upgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";
import {Options as UpgradeOptions} from "openzeppelin-foundry-upgrades/Options.sol";

import {ProofMarket, MerkleProofish, AssessorJournal, TransientPrice, TransientPriceLib} from "../src/ProofMarket.sol";
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
import {RiscZeroSetVerifier} from "../src/RiscZeroSetVerifier.sol";

Vm constant VM = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

bytes32 constant APP_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000001;
bytes32 constant SET_BUILDER_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000002;
bytes32 constant ASSESSOR_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000003;

bytes constant APP_JOURNAL = bytes("GUEST JOURNAL");

contract Client {
    using SafeCast for uint256;
    using SafeCast for int256;
    using ProofMarketLib for Requirements;
    using ProofMarketLib for ProvingRequest;
    using ProofMarketLib for Offer;

    string public identifier;
    Vm.Wallet public wallet;
    IProofMarket public proofMarket;

    /// A snapshot of the client balance for later comparison.
    int256 internal balanceSnapshot;

    receive() external payable {}

    function initialize(string memory _identifier, IProofMarket _proofMarket) public {
        identifier = _identifier;
        proofMarket = _proofMarket;
        wallet = VM.createWallet(identifier);
        balanceSnapshot = type(int256).max;
    }

    function defaultOffer() public view returns (Offer memory) {
        return Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            biddingStart: uint64(block.number),
            rampUpPeriod: uint32(10),
            timeout: uint32(100),
            lockinStake: 1 ether
        });
    }

    function defaultRequirements() public pure returns (Requirements memory) {
        return Requirements({
            imageId: bytes32(APP_IMAGE_ID),
            predicate: Predicate({predicateType: PredicateType.DigestMatch, data: abi.encode(sha256(APP_JOURNAL))})
        });
    }

    function request(uint32 idx) public view returns (ProvingRequest memory) {
        return ProvingRequest({
            id: ProofMarketLib.requestId(wallet.addr, idx),
            requirements: defaultRequirements(),
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: defaultOffer()
        });
    }

    function request(uint32 idx, Offer memory offer) public view returns (ProvingRequest memory) {
        return ProvingRequest({
            id: ProofMarketLib.requestId(wallet.addr, idx),
            requirements: defaultRequirements(),
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: offer
        });
    }

    function sign(ProvingRequest memory req) public returns (bytes memory) {
        bytes32 structDigest = MessageHashUtils.toTypedDataHash(proofMarket.eip712DomainSeparator(), req.eip712Digest());
        (uint8 v, bytes32 r, bytes32 s) = VM.sign(wallet, structDigest);
        return abi.encodePacked(r, s, v);
    }

    function snapshotBalance() public {
        balanceSnapshot = proofMarket.balanceOf(wallet.addr).toInt256();
        console2.log("%s balance at block %d: %d", identifier, block.number, balanceSnapshot.toUint256());
    }

    function expectBalanceChange(int256 change) public view {
        require(balanceSnapshot != type(int256).max, "balance snapshot is not set");
        int256 newBalance = proofMarket.balanceOf(wallet.addr).toInt256();
        console2.log("%s balance at block %d: %d", identifier, block.number, newBalance.toUint256());
        int256 expectedBalance = balanceSnapshot + change;
        require(expectedBalance >= 0, "expected balance cannot be less than 0");
        console2.log("%s expected balance is %d", identifier, expectedBalance.toUint256());
        require(expectedBalance == newBalance, "balance is not equal to expected value");
    }
}

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
    mapping(uint256 => Client) private clients;
    Client internal testProver;
    uint256 initialBalance;

    uint256 constant DEFAULT_BALANCE = 1000 ether;

    ReceiptClaim internal APP_CLAIM = ReceiptClaimLib.ok(APP_IMAGE_ID, sha256(APP_JOURNAL));

    Vm.Wallet internal OWNER_WALLET = vm.createWallet("OWNER");

    function setUp() public {
        vm.deal(OWNER_WALLET.addr, DEFAULT_BALANCE);

        vm.startPrank(OWNER_WALLET.addr);

        // Deploy the implementation contracts
        verifier = new RiscZeroMockVerifier(bytes4(0));
        setVerifier = new RiscZeroSetVerifier(verifier, SET_BUILDER_IMAGE_ID, "https://set-builder.dev.null");

        // Deploy the UUPS proxy with the implementation
        UpgradeOptions memory opts;
        opts.constructorData = ProofMarketLib.encodeConstructorArgs(setVerifier, ASSESSOR_IMAGE_ID);
        proxy = Upgrades.deployUUPSProxy(
            "ProofMarket.sol:ProofMarket",
            abi.encodeCall(ProofMarket.initialize, (OWNER_WALLET.addr, "https://assessor.dev.null")),
            opts
        );
        proofMarket = ProofMarket(proxy);

        vm.stopPrank();

        testProver = createClientContract("PROVER");

        vm.deal(address(testProver), DEFAULT_BALANCE);
        vm.prank(address(testProver));
        proofMarket.deposit{value: DEFAULT_BALANCE}();
        testProver.snapshotBalance();

        for (uint256 i = 0; i < 5; i++) {
            getClient(i);
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
    function getClient(uint256 index) internal returns (Client) {
        if (address(clients[index]) != address(0)) {
            return clients[index];
        }

        Client client = createClientContract(string.concat("CLIENT_", vm.toString(index)));

        // Deal the client from Ether and deposit it in the market.
        vm.deal(address(client), DEFAULT_BALANCE);
        vm.prank(address(client));
        proofMarket.deposit{value: DEFAULT_BALANCE}();

        // Snapshot their initial balance.
        client.snapshotBalance();

        clients[index] = client;
        return client;
    }

    // Create a client, using a trick to set the address equal to the wallet address.
    function createClientContract(string memory identifier) internal returns (Client) {
        address payable clientAddress = payable(vm.createWallet(identifier).addr);
        vm.etch(clientAddress, address(new Client()).code);
        Client client = Client(clientAddress);
        client.initialize(identifier, proofMarket);
        return client;
    }

    function publishRoot(bytes32 root) internal {
        setVerifier.submitMerkleRoot(
            root, verifier.mockProve(SET_BUILDER_IMAGE_ID, sha256(abi.encodePacked(SET_BUILDER_IMAGE_ID, root))).seal
        );
    }

    function fulfillRequest(ProvingRequest memory request, bytes memory journal, address prover)
        internal
        returns (Fulfillment memory, bytes memory assessorSeal)
    {
        ProvingRequest[] memory requests = new ProvingRequest[](1);
        requests[0] = request;
        bytes[] memory journals = new bytes[](1);
        journals[0] = journal;
        (Fulfillment[] memory fills, bytes memory seal) = fulfillRequestBatch(requests, journals, prover);
        return (fills[0], seal);
    }

    function fulfillRequestBatch(ProvingRequest[] memory requests, bytes[] memory journals, address prover)
        internal
        returns (Fulfillment[] memory fills, bytes memory assessorSeal)
    {
        bytes32 root;
        (fills, assessorSeal, root) = createFills(requests, journals, prover, true);
        // submit the root to the set verifier
        publishRoot(root);
        return (fills, assessorSeal);
    }

    function createFills(ProvingRequest[] memory requests, bytes[] memory journals, address prover, bool requirePayment)
        internal
        view
        returns (Fulfillment[] memory fills, bytes memory assessorSeal, bytes32 root)
    {
        // initialize the fullfillments; one for each request;
        // the seal is filled in later, by calling fillInclusionProof
        fills = new Fulfillment[](requests.length);
        for (uint256 i = 0; i < requests.length; i++) {
            Fulfillment memory fill = Fulfillment({
                id: requests[i].id,
                imageId: requests[i].requirements.imageId,
                journal: journals[i],
                seal: bytes(""),
                requirePayment: requirePayment
            });
            fills[i] = fill;
        }

        // compute the assessor claim
        ReceiptClaim memory assessorClaim =
            TestUtils.mockAssessor(fills, ASSESSOR_IMAGE_ID, proofMarket.eip712DomainSeparator(), prover);
        // compute the batchRoot of the batch Merkle Tree (without the assessor)
        (bytes32 batchRoot, bytes32[][] memory tree) = TestUtils.mockSetBuilder(fills);

        root = MerkleProofish._hashPair(batchRoot, assessorClaim.digest());

        // compute all the inclusion proofs for the fullfillments
        TestUtils.fillInclusionProofs(setVerifier, fills, assessorClaim.digest(), tree);
        // compute the assessor seal
        assessorSeal = TestUtils.mockAssessorSeal(setVerifier, batchRoot);

        return (fills, assessorSeal, root);
    }

    function testDeposit() public {
        vm.deal(address(testProver), 1 ether);
        // Deposit funds into the market
        vm.expectEmit(true, true, true, true);
        emit IProofMarket.Deposit(address(testProver), 1 ether);
        vm.prank(address(testProver));
        proofMarket.deposit{value: 1 ether}();
        testProver.expectBalanceChange(1 ether);
    }

    function testWithdraw() public {
        // Deposit funds into the market
        vm.deal(address(testProver), 1 ether);
        vm.prank(address(testProver));
        proofMarket.deposit{value: 1 ether}();

        // Withdraw funds from the market
        vm.expectEmit(true, true, true, true);
        emit IProofMarket.Withdrawal(address(testProver), 1 ether);
        vm.prank(address(testProver));
        proofMarket.withdraw(1 ether);
        checkProofMarketBalance();

        // Attempt to withdraw extra funds from the market.
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.InsufficientBalance.selector, address(testProver)));
        vm.prank(address(testProver));
        proofMarket.withdraw(DEFAULT_BALANCE + 1);
        checkProofMarketBalance();
    }

    function testSubmitRequest() public {
        Client client = getClient(1);
        ProvingRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);

        // Submit the request with no funds
        // Expect the event to be emitted
        vm.expectEmit(true, true, true, true);
        emit IProofMarket.RequestSubmitted(request.id, request, clientSignature);
        proofMarket.submitRequest(request, clientSignature);

        // Submit the request with funds
        // Expect the event to be emitted
        vm.expectEmit(true, true, true, true);
        emit IProofMarket.Deposit(address(client), uint256(request.offer.maxPrice));
        vm.expectEmit(true, true, true, true);
        emit IProofMarket.RequestSubmitted(request.id, request, clientSignature);
        vm.deal(address(client), request.offer.maxPrice);
        vm.prank(address(client));
        proofMarket.submitRequest{value: request.offer.maxPrice}(request, clientSignature);
    }

    function _testLockin(bool withSig) private returns (Client, ProvingRequest memory) {
        Client client = getClient(1);
        ProvingRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.sign(request);

        // Expect the event to be emitted
        vm.expectEmit(true, true, true, true);
        emit IProofMarket.RequestLockedin(request.id, address(testProver));
        if (withSig) {
            proofMarket.lockinWithSig(request, clientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            proofMarket.lockin(request, clientSignature);
        }

        // Ensure the balances are correct
        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(-1 ether);

        // Verify the lockin
        assertTrue(proofMarket.requestIsLocked(request.id), "Request should be locked-in");

        checkProofMarketBalance();

        return (client, request);
    }

    function testLockin() public returns (Client, ProvingRequest memory) {
        return _testLockin(true);
    }

    function testLockinWithSig() public returns (Client, ProvingRequest memory) {
        return _testLockin(false);
    }

    function _testLockinAlreadyLocked(bool withSig) private {
        (Client client, ProvingRequest memory request) = _testLockin(withSig);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.sign(request);

        // Attempt to lock in the request again
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.RequestIsLocked.selector, request.id));
        if (withSig) {
            proofMarket.lockinWithSig(request, clientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            proofMarket.lockin(request, clientSignature);
        }

        checkProofMarketBalance();
    }

    function testLockinAlreadyLocked() public {
        return _testLockinAlreadyLocked(true);
    }

    function testLockinWithSigAlreadyLocked() public {
        return _testLockinAlreadyLocked(false);
    }

    function _testLockinBadClientSignature(bool withSig) private {
        Client clientA = getClient(1);
        Client clientB = getClient(2);
        ProvingRequest memory request1 = clientA.request(1);
        ProvingRequest memory request2 = clientA.request(2);
        bytes memory proverSignature = testProver.sign(request1);

        // case: request signed by a different client
        bytes memory badClientSignature = clientB.sign(request1);
        vm.expectRevert(IProofMarket.InvalidSignature.selector);
        if (withSig) {
            proofMarket.lockinWithSig(request1, badClientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            proofMarket.lockin(request1, badClientSignature);
        }

        // case: client signed a different request
        badClientSignature = clientA.sign(request2);
        vm.expectRevert(IProofMarket.InvalidSignature.selector);
        if (withSig) {
            proofMarket.lockinWithSig(request1, badClientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            proofMarket.lockin(request1, badClientSignature);
        }

        clientA.expectBalanceChange(0 ether);
        clientB.expectBalanceChange(0 ether);
        testProver.expectBalanceChange(0 ether);
        checkProofMarketBalance();
    }

    function testLockinBadClientSignature() public {
        return _testLockinBadClientSignature(true);
    }

    function testLockinWithSigBadClientSignature() public {
        return _testLockinBadClientSignature(false);
    }

    function testLockinWithSigBadProverSignature() public {
        Client client = getClient(1);
        ProvingRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        // Bad signature is over the wrong request.
        bytes memory badProverSignature = testProver.sign(client.request(2));

        // NOTE: Error is "InsufficientBalance" because we will recover _some_ address.
        // It should be completed random and never correspond to a real account.
        // TODO: This address will need to change anytime we change the ProvingRequest struct or
        // the way it is hashed for signatures. Find a good way to avoid this.
        vm.expectRevert(
            abi.encodeWithSelector(
                IProofMarket.InsufficientBalance.selector, address(0x5c541fA34e0b605E586fB688EFa1550169d80ECb)
            )
        );
        proofMarket.lockinWithSig(request, clientSignature, badProverSignature);

        client.expectBalanceChange(0 ether);
        testProver.expectBalanceChange(0 ether);
        checkProofMarketBalance();
    }

    function _testLockinNotEnoughFunds(bool withSig) private {
        Client client = getClient(1);
        ProvingRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.sign(request);

        vm.prank(address(client));
        proofMarket.withdraw(DEFAULT_BALANCE);

        // case: client does not have enough funds to cover for the lockin
        // should revert with "InsufficientBalance(address requester)"
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.InsufficientBalance.selector, address(client)));
        if (withSig) {
            proofMarket.lockinWithSig(request, clientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            proofMarket.lockin(request, clientSignature);
        }

        vm.prank(address(client));
        proofMarket.deposit{value: DEFAULT_BALANCE}();

        vm.prank(address(testProver));
        proofMarket.withdraw(DEFAULT_BALANCE);

        // case: prover does not have enough funds to cover for the lockin stake
        // should revert with "InsufficientBalance(address requester)"
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.InsufficientBalance.selector, address(testProver)));
        if (withSig) {
            proofMarket.lockinWithSig(request, clientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            proofMarket.lockin(request, clientSignature);
        }
    }

    function testLockinNotEnoughFunds() public {
        return _testLockinNotEnoughFunds(true);
    }

    function testLockinWithSigNotEnoughFunds() public {
        return _testLockinNotEnoughFunds(false);
    }

    function _testLockinExpired(bool withSig) private {
        Client client = getClient(1);
        ProvingRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.sign(request);

        vm.roll(request.offer.deadline() + 1);

        // Attempt to lock in the request after it has expired
        // should revert with "RequestIsExpired({requestId: request.id, deadline: deadline})"
        vm.expectRevert(
            abi.encodeWithSelector(IProofMarket.RequestIsExpired.selector, request.id, request.offer.deadline())
        );
        if (withSig) {
            proofMarket.lockinWithSig(request, clientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            proofMarket.lockin(request, clientSignature);
        }

        checkProofMarketBalance();
    }

    function testLockinExpired() public {
        return _testLockinExpired(true);
    }

    function testLockinWithSigExpired() public {
        return _testLockinExpired(false);
    }

    function _testLockinInvalidRequest1(bool withSig) private {
        Offer memory offer = Offer({
            minPrice: 2 ether,
            maxPrice: 1 ether,
            biddingStart: uint64(block.number),
            rampUpPeriod: uint32(0),
            timeout: uint32(1),
            lockinStake: 10 ether
        });

        Client client = getClient(1);
        ProvingRequest memory request = client.request(1, offer);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.sign(request);

        // Attempt to lockin a request with maxPrice smaller than minPrice
        // should revert with "maxPrice cannot be smaller than minPrice"
        vm.expectRevert("maxPrice cannot be smaller than minPrice");
        if (withSig) {
            proofMarket.lockinWithSig(request, clientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            proofMarket.lockin(request, clientSignature);
        }

        checkProofMarketBalance();
    }

    function testLockinInvalidRequest1() public {
        return _testLockinInvalidRequest1(true);
    }

    function testLockinWithSigInvalidRequest1() public {
        return _testLockinInvalidRequest1(false);
    }

    function _testLockinInvalidRequest2(bool withSig) private {
        Offer memory offer = Offer({
            minPrice: 1 ether,
            maxPrice: 1 ether,
            biddingStart: uint64(0),
            rampUpPeriod: uint32(2),
            timeout: uint32(1),
            lockinStake: 10 ether
        });

        Client client = getClient(1);
        ProvingRequest memory request = client.request(1, offer);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.sign(request);

        // Attempt to lockin a request with rampUpPeriod greater than timeout
        // should revert with "Request cannot expire before end of bidding period"
        vm.expectRevert("Request cannot expire before end of bidding period");
        if (withSig) {
            proofMarket.lockinWithSig(request, clientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            proofMarket.lockin(request, clientSignature);
        }

        checkProofMarketBalance();
    }

    function testLockinInvalidRequest2() public {
        return _testLockinInvalidRequest2(true);
    }

    function testLockinWithSigInvalidRequest2() public {
        return _testLockinInvalidRequest2(false);
    }

    function _testFulfill(uint32 requestIdx) private returns (Client, ProvingRequest memory) {
        Client client = getClient(1);
        ProvingRequest memory request = client.request(requestIdx);
        bytes memory clientSignature = client.sign(request);

        client.snapshotBalance();
        testProver.snapshotBalance();

        vm.prank(address(testProver));
        proofMarket.lockin(request, clientSignature);

        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, address(testProver));

        vm.expectEmit(true, true, true, true);
        emit IProofMarket.RequestFulfilled(request.id);
        vm.expectEmit(true, true, true, false);
        emit IProofMarket.ProofDelivered(request.id, hex"", hex"");
        proofMarket.fulfill(fill, assessorSeal, address(testProver));

        // Check that the proof was submitted
        assertTrue(proofMarket.requestIsFulfilled(fill.id), "Request should have fulfilled status");

        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        checkProofMarketBalance();

        return (client, request);
    }

    // TODO(victor): Add tests that follow the same logic, but without lockin.
    function testFulfill() public returns (Client, ProvingRequest memory) {
        return _testFulfill(1);
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

    function testFulfillViaLockinWithSig() public {
        Client client = getClient(1);
        ProvingRequest memory request = client.request(3);

        proofMarket.lockinWithSig(request, client.sign(request), testProver.sign(request));
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, address(testProver));
        proofMarket.fulfill(fill, assessorSeal, address(testProver));

        // Check that the proof was submitted
        assertTrue(proofMarket.requestIsFulfilled(fill.id), "Request should have fulfilled status");

        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        checkProofMarketBalance();
    }

    function testFulfillBatchWithSig() public {
        // Provide a batch definition as an array of clients and how many requests each submits.
        uint256[5] memory batch = [uint256(1), 2, 1, 3, 1];
        uint256 batchSize = 0;
        for (uint256 i = 0; i < batch.length; i++) {
            batchSize += batch[i];
        }

        ProvingRequest[] memory requests = new ProvingRequest[](batchSize);
        bytes[] memory journals = new bytes[](batchSize);
        uint96 expectedRevenue = 0;
        uint256 idx = 0;
        for (uint256 i = 0; i < batch.length; i++) {
            Client client = getClient(i);

            for (uint256 j = 0; j < batch[i]; j++) {
                ProvingRequest memory request = client.request(uint32(j));

                // TODO: This is a fragile part of this test. It should be improved.
                uint96 desiredPrice = uint96(1.5 ether);
                vm.roll(request.offer.blockAtPrice(desiredPrice));
                expectedRevenue += desiredPrice;

                proofMarket.lockinWithSig(request, client.sign(request), testProver.sign(request));

                requests[idx] = request;
                journals[idx] = APP_JOURNAL;
                idx++;
            }
        }

        (Fulfillment[] memory fills, bytes memory assessorSeal) =
            fulfillRequestBatch(requests, journals, address(testProver));

        for (uint256 i = 0; i < fills.length; i++) {
            vm.expectEmit(true, true, true, true);
            emit IProofMarket.RequestFulfilled(fills[i].id);
            vm.expectEmit(true, true, true, false);
            emit IProofMarket.ProofDelivered(fills[i].id, hex"", hex"");
        }
        proofMarket.fulfillBatch(fills, assessorSeal, address(testProver));

        for (uint256 i = 0; i < fills.length; i++) {
            // Check that the proof was submitted
            assertTrue(proofMarket.requestIsFulfilled(fills[i].id), "Request should have fulfilled status");
        }

        testProver.expectBalanceChange(int256(uint256(expectedRevenue)));
        checkProofMarketBalance();
    }

    function testFulfillDistinctProversRequirePayment() public {
        Client client = getClient(1);
        ProvingRequest memory request = client.request(3);

        proofMarket.lockinWithSig(request, client.sign(request), testProver.sign(request));
        // address(3) is just a standin for some other address.
        address mockOtherProverAddr = address(uint160(3));
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, mockOtherProverAddr);

        vm.expectRevert(abi.encodeWithSelector(IProofMarket.RequestIsLocked.selector, request.id));
        proofMarket.fulfill(fill, assessorSeal, mockOtherProverAddr);

        assertFalse(proofMarket.requestIsFulfilled(fill.id), "Request should not have fulfilled status");

        // Prover should have their original balance less the stake amount.
        testProver.expectBalanceChange(-int256(uint256(request.offer.lockinStake)));
        checkProofMarketBalance();
    }

    function testFulfillDistinctProversNoPayment() public {
        Client client = getClient(1);

        ProvingRequest memory request = client.request(3);

        proofMarket.lockinWithSig(request, client.sign(request), testProver.sign(request));
        // address(3) is just a standin for some other address.
        address mockOtherProverAddr = address(uint160(3));
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, mockOtherProverAddr);
        fill.requirePayment = false;

        vm.expectEmit(true, true, true, true);
        emit IProofMarket.PaymentRequirementsFailed(
            abi.encodeWithSelector(IProofMarket.RequestIsLocked.selector, request.id)
        );
        proofMarket.fulfill(fill, assessorSeal, mockOtherProverAddr);

        assertTrue(proofMarket.requestIsFulfilled(fill.id), "Request should have fulfilled status");

        // Prover should have their original balance less the stake amount.
        testProver.expectBalanceChange(-int256(uint256(request.offer.lockinStake)));
        checkProofMarketBalance();
    }

    function testFulfillFulfillProverAddrDoesNotMatchAssessorReceipt() public {
        Client client = getClient(1);

        ProvingRequest memory request = client.request(3);

        proofMarket.lockinWithSig(request, client.sign(request), testProver.sign(request));
        // address(3) is just a standin for some other address.
        address mockOtherProverAddr = address(uint160(3));
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, address(testProver));

        vm.expectRevert(VerificationFailed.selector);
        proofMarket.fulfill(fill, assessorSeal, mockOtherProverAddr);

        // Prover should have their original balance less the stake amount.
        testProver.expectBalanceChange(-int256(uint256(request.offer.lockinStake)));
        checkProofMarketBalance();
    }

    function testPriceAndFulfill() external {
        Client client = getClient(1);
        ProvingRequest memory request = client.request(3);

        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, address(testProver));

        Fulfillment[] memory fills = new Fulfillment[](1);
        fills[0] = fill;
        ProvingRequest[] memory requests = new ProvingRequest[](1);
        requests[0] = request;
        bytes[] memory clientSignatures = new bytes[](1);
        clientSignatures[0] = client.sign(request);

        vm.expectEmit(true, true, true, true);
        emit IProofMarket.RequestFulfilled(request.id);
        vm.expectEmit(true, true, true, false);
        emit IProofMarket.ProofDelivered(request.id, hex"", hex"");
        proofMarket.priceAndFulfillBatch(requests, clientSignatures, fills, assessorSeal, address(testProver));

        assertTrue(proofMarket.requestIsFulfilled(fill.id), "Request should have fulfilled status");

        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        checkProofMarketBalance();
    }

    function testFulfillAlreadyFulfilled() public {
        (, ProvingRequest memory request) = testFulfill();

        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, address(testProver));
        // Attempt to fulfill a request already fulfilled
        // should revert with "RequestIsFulfilled({requestId: request.id})"
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.RequestIsFulfilled.selector, request.id));
        proofMarket.fulfill(fill, assessorSeal, address(testProver));

        checkProofMarketBalance();
    }

    function testFulfillRequestNotLocked() public {
        Client client = getClient(1);
        ProvingRequest memory request = client.request(1);
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, address(testProver));

        // Attempt to fulfill a request without locking or pricing it.
        // should revert with "RequestIsNotLocked({requestId: request.id})"
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.RequestIsNotLocked.selector, request.id));
        proofMarket.fulfill(fill, assessorSeal, address(testProver));

        checkProofMarketBalance();
    }

    function testFulfillExpired() public returns (Client, ProvingRequest memory) {
        Client client = getClient(1);
        ProvingRequest memory request = client.request(1);

        proofMarket.lockinWithSig(request, client.sign(request), testProver.sign(request));
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, address(testProver));

        vm.roll(request.offer.deadline() + 1);

        // Attempt to fulfill an expired request
        // should revert with "RequestIsExpired({requestId: request.id, deadline: deadline})"
        vm.expectRevert(
            abi.encodeWithSelector(IProofMarket.RequestIsExpired.selector, request.id, request.offer.deadline())
        );
        proofMarket.fulfill(fill, assessorSeal, address(testProver));

        // Prover should have their original balance less the stake amount.
        testProver.expectBalanceChange(-int256(uint256(request.offer.lockinStake)));
        checkProofMarketBalance();

        return (client, request);
    }

    function testSlash() public returns (Client, ProvingRequest memory) {
        (Client client, ProvingRequest memory request) = testFulfillExpired();

        // Slash the request
        vm.expectEmit(true, true, true, true);
        emit IProofMarket.ProverSlashed(request.id, request.offer.lockinStake, 0);
        proofMarket.slash(request.id);

        // NOTE: This should be updated is not all the stake  burned.
        client.expectBalanceChange(0 ether);
        testProver.expectBalanceChange(-int256(uint256(request.offer.lockinStake)));
        checkBurnedBalance(request.offer.lockinStake);

        return (client, request);
    }

    function testSlashInvalidRequestID() public {
        // Attempt to slash an invalid request ID
        // should revert with "RequestIsNotLocked({requestId: request.id})"
        vm.expectRevert(abi.encodeWithSelector(IProofMarket.RequestIsNotLocked.selector, 0xa));
        proofMarket.slash(0xa);

        checkProofMarketBalance();
    }

    function testSlashNotExpired() public {
        (, ProvingRequest memory request) = testLockin();

        // Attempt to slash a request not expired
        // should revert with "RequestIsNotExpired({requestId: request.id,  deadline: deadline})"
        vm.expectRevert(
            abi.encodeWithSelector(IProofMarket.RequestIsNotExpired.selector, request.id, request.offer.deadline())
        );
        proofMarket.slash(request.id);

        checkProofMarketBalance();
    }

    function testSlashFulfilled() public {
        (, ProvingRequest memory request) = testFulfill();

        vm.expectRevert(abi.encodeWithSelector(IProofMarket.RequestIsNotLocked.selector, request.id));
        proofMarket.slash(request.id);

        checkProofMarketBalance();
    }

    function testSlashSlash() public {
        (, ProvingRequest memory request) = testSlash();

        vm.expectRevert(abi.encodeWithSelector(IProofMarket.RequestIsNotLocked.selector, request.id));
        proofMarket.slash(request.id);

        checkBurnedBalance(request.offer.lockinStake);
    }

    function newBatch(uint256 batchSize) internal returns (ProvingRequest[] memory requests, bytes[] memory journals) {
        requests = new ProvingRequest[](batchSize);
        journals = new bytes[](batchSize);
        for (uint256 j = 0; j < 5; j++) {
            getClient(j);
        }
        for (uint256 i = 0; i < batchSize; i++) {
            Client client = clients[i % 5];
            ProvingRequest memory request = client.request(uint32(i / 5));
            bytes memory clientSignature = client.sign(request);
            vm.prank(address(testProver));
            proofMarket.lockin(request, clientSignature);
            requests[i] = request;
            journals[i] = APP_JOURNAL;
        }
    }

    function benchFulfillBatch(uint256 batchSize) public {
        (ProvingRequest[] memory requests, bytes[] memory journals) = newBatch(batchSize);
        (Fulfillment[] memory fills, bytes memory assessorSeal) =
            fulfillRequestBatch(requests, journals, address(testProver));

        uint256 gasBefore = gasleft();
        proofMarket.fulfillBatch(fills, assessorSeal, address(testProver));
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

    function testsubmitRootAndFulfillBatch() public {
        (ProvingRequest[] memory requests, bytes[] memory journals) = newBatch(2);
        (Fulfillment[] memory fills, bytes memory assessorSeal, bytes32 root) =
            createFills(requests, journals, address(testProver), true);

        bytes memory seal =
            verifier.mockProve(SET_BUILDER_IMAGE_ID, sha256(abi.encodePacked(SET_BUILDER_IMAGE_ID, root))).seal;
        proofMarket.submitRootAndFulfillBatch(root, seal, fills, assessorSeal, address(testProver));

        for (uint256 j = 0; j < fills.length; j++) {
            assertTrue(proofMarket.requestIsFulfilled(fills[j].id), "Request should have fulfilled status");
        }
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

    // TODO(#109) Refactor these tests to check for upgradeability from a prior commit to the latest version.
    // With that, we might also check that it is possible to upgrade to a notional future version, or we might
    // want to drop the ProofMarketV2Test contract.
    function testUnsafeUpgrade() public {
        vm.startPrank(OWNER_WALLET.addr);
        proxy = UnsafeUpgrades.deployUUPSProxy(
            address(new ProofMarket(setVerifier, ASSESSOR_IMAGE_ID)),
            abi.encodeCall(ProofMarket.initialize, (OWNER_WALLET.addr, "https://assessor.dev.null"))
        );
        proofMarket = ProofMarket(proxy);
        address implAddressV1 = UnsafeUpgrades.getImplementationAddress(proxy);

        // Should emit an `Upgraded` event
        vm.expectEmit(false, true, true, true);
        emit IERC1967.Upgraded(address(0));
        UnsafeUpgrades.upgradeProxy(
            proxy, address(new ProofMarket(setVerifier, ASSESSOR_IMAGE_ID)), "", OWNER_WALLET.addr
        );
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

contract TransientPriceLibTest is Test {
    using TransientPriceLib for TransientPrice;

    /// forge-config: default.fuzz.runs = 10000
    function testFuzz_PackUnpack(bool valid, uint96 price) public pure {
        TransientPrice memory original = TransientPrice({valid: valid, price: price});

        uint256 packed = TransientPriceLib.pack(original);
        TransientPrice memory unpacked = TransientPriceLib.unpack(packed);

        assertEq(unpacked.valid, original.valid, "Valid flag mismatch");
        assertEq(unpacked.price, original.price, "Price mismatch");
    }
}
