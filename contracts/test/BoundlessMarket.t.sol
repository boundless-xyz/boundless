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
import {HitPoints} from "../src/HitPoints.sol";
import {IHitPoints} from "../src/IHitPoints.sol";

import {
    BoundlessMarket,
    MerkleProofish,
    AssessorJournal,
    TransientPrice,
    TransientPriceLib
} from "../src/BoundlessMarket.sol";
import {
    Fulfillment,
    IBoundlessMarket,
    Input,
    InputType,
    Offer,
    Predicate,
    PredicateType,
    ProofRequest,
    Requirements
} from "../src/IBoundlessMarket.sol";
import {BoundlessMarketLib} from "../src/BoundlessMarketLib.sol";
import {RiscZeroSetVerifier} from "risc0/RiscZeroSetVerifier.sol";

Vm constant VM = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

bytes32 constant APP_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000001;
bytes32 constant SET_BUILDER_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000002;
bytes32 constant ASSESSOR_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000003;

bytes constant APP_JOURNAL = bytes("GUEST JOURNAL");

contract Client {
    using SafeCast for uint256;
    using SafeCast for int256;
    using BoundlessMarketLib for Requirements;
    using BoundlessMarketLib for ProofRequest;
    using BoundlessMarketLib for Offer;

    string public identifier;
    Vm.Wallet public wallet;
    IBoundlessMarket public boundlessMarket;
    IHitPoints public hitPoints;

    /// A snapshot of the client balance for later comparison.
    int256 internal balanceSnapshot;
    int256 internal hpBalanceSnapshot;

    receive() external payable {}

    function initialize(string memory _identifier, IBoundlessMarket _boundlessMarket, IHitPoints _hitPoints) public {
        identifier = _identifier;
        boundlessMarket = _boundlessMarket;
        hitPoints = _hitPoints;
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

    function request(uint32 idx) public view returns (ProofRequest memory) {
        return ProofRequest({
            id: BoundlessMarketLib.requestId(wallet.addr, idx),
            requirements: defaultRequirements(),
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: defaultOffer()
        });
    }

    function request(uint32 idx, Offer memory offer) public view returns (ProofRequest memory) {
        return ProofRequest({
            id: BoundlessMarketLib.requestId(wallet.addr, idx),
            requirements: defaultRequirements(),
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: offer
        });
    }

    function sign(ProofRequest memory req) public returns (bytes memory) {
        bytes32 structDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), req.eip712Digest());
        (uint8 v, bytes32 r, bytes32 s) = VM.sign(wallet, structDigest);
        return abi.encodePacked(r, s, v);
    }

    function snapshotBalance() public {
        balanceSnapshot = boundlessMarket.balanceOf(wallet.addr).toInt256();
        //console2.log("%s balance at block %d: %d", identifier, block.number, balanceSnapshot.toUint256());
    }

    function snapshotHPBalance() public {
        (uint256 balance,) = hitPoints.balanceOf(wallet.addr);
        hpBalanceSnapshot = balance.toInt256();
        //console2.log("%s HP balance at block %d: %d", identifier, block.number, hpBalanceSnapshot.toUint256());
    }

    function expectBalanceChange(int256 change) public view {
        require(balanceSnapshot != type(int256).max, "balance snapshot is not set");
        int256 newBalance = boundlessMarket.balanceOf(wallet.addr).toInt256();
        console2.log("%s balance at block %d: %d", identifier, block.number, newBalance.toUint256());
        int256 expectedBalance = balanceSnapshot + change;
        require(expectedBalance >= 0, "expected balance cannot be less than 0");
        console2.log("%s expected balance is %d", identifier, expectedBalance.toUint256());
        require(expectedBalance == newBalance, "balance is not equal to expected value");
    }

    function expectHPBalanceChange(int256 change) public view {
        require(hpBalanceSnapshot != type(int256).max, "balance snapshot is not set");
        (uint256 balance,) = hitPoints.balanceOf(wallet.addr);
        int256 newBalance = balance.toInt256();
        console2.log("%s HP balance at block %d: %d", identifier, block.number, newBalance.toUint256());
        int256 expectedBalance = hpBalanceSnapshot + change;
        require(expectedBalance >= 0, "expected HP balance cannot be less than 0");
        console2.log("%s expected HP balance is %d", identifier, expectedBalance.toUint256());
        require(expectedBalance == newBalance, "HP balance is not equal to expected value");
    }
}

contract BoundlessMarketTest is Test {
    using ReceiptClaimLib for ReceiptClaim;
    using BoundlessMarketLib for Requirements;
    using BoundlessMarketLib for ProofRequest;
    using BoundlessMarketLib for Offer;
    using TestUtils for RiscZeroSetVerifier;

    RiscZeroMockVerifier internal verifier;
    BoundlessMarket internal boundlessMarket;
    address internal proxy;
    RiscZeroSetVerifier internal setVerifier;
    HitPoints internal hitPoints;
    mapping(uint256 => Client) internal clients;
    Client internal testProver;
    uint256 initialBalance;
    uint256 initialHPSupply;

    uint256 constant DEFAULT_BALANCE = 1000 ether;

    ReceiptClaim internal APP_CLAIM = ReceiptClaimLib.ok(APP_IMAGE_ID, sha256(APP_JOURNAL));

    Vm.Wallet internal OWNER_WALLET = vm.createWallet("OWNER");

    function setUp() public {
        vm.deal(OWNER_WALLET.addr, DEFAULT_BALANCE);

        vm.startPrank(OWNER_WALLET.addr);

        // Deploy the implementation contracts
        verifier = new RiscZeroMockVerifier(bytes4(0));
        setVerifier = new RiscZeroSetVerifier(verifier, SET_BUILDER_IMAGE_ID, "https://set-builder.dev.null");
        hitPoints = new HitPoints(OWNER_WALLET.addr);

        // Deploy the UUPS proxy with the implementation
        UpgradeOptions memory opts;
        opts.constructorData = BoundlessMarketLib.encodeConstructorArgs(setVerifier, ASSESSOR_IMAGE_ID);
        proxy = Upgrades.deployUUPSProxy(
            "BoundlessMarket.sol:BoundlessMarket",
            abi.encodeCall(
                BoundlessMarket.initialize, (OWNER_WALLET.addr, "https://assessor.dev.null", address(hitPoints))
            ),
            opts
        );
        boundlessMarket = BoundlessMarket(proxy);

        hitPoints.authorize(OWNER_WALLET.addr);
        hitPoints.authorize(proxy);
        vm.stopPrank();

        testProver = createClientContract("PROVER");

        vm.prank(OWNER_WALLET.addr);
        hitPoints.mint(address(testProver), DEFAULT_BALANCE);

        vm.deal(address(testProver), DEFAULT_BALANCE);
        testProver.snapshotBalance();
        testProver.snapshotHPBalance();

        for (uint256 i = 0; i < 5; i++) {
            getClient(i);
        }

        initialBalance = address(boundlessMarket).balance;
        initialHPSupply = hitPoints.totalSupply();

        // Verify that OWNER is the actual owner
        assertEq(boundlessMarket.owner(), OWNER_WALLET.addr, "OWNER address is not the contract owner after deployment");
    }

    function expectMarketBalanceUnchanged() internal view {
        uint256 finalBalance = address(boundlessMarket).balance;
        //console2.log("Initial balance:", initialBalance);
        //console2.log("Final balance:", finalBalance);
        require(finalBalance == initialBalance, "Contract balance changed during the test");
    }

    function expectMarketBalanceBurned(uint256 burnedBalance) internal view {
        uint256 finalBalance = address(boundlessMarket).balance;
        //console2.log("Initial balance:", initialBalance);
        //console2.log("Final balance:", finalBalance);
        require(finalBalance == initialBalance - burnedBalance, "Contract balance changed during the test");
        require(address(0).balance == burnedBalance, "Burned balance did not go to the null address");
    }

    function expectHPSupplyBurned(uint256 burnedBalance) internal view {
        uint256 finalHPSupply = hitPoints.totalSupply();
        require(finalHPSupply == initialHPSupply - burnedBalance, "HP total supply changed during the test");
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
        boundlessMarket.deposit{value: DEFAULT_BALANCE}();

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
        client.initialize(identifier, boundlessMarket, hitPoints);
        return client;
    }

    function publishRoot(bytes32 root) internal {
        setVerifier.submitMerkleRoot(
            root, verifier.mockProve(SET_BUILDER_IMAGE_ID, sha256(abi.encodePacked(SET_BUILDER_IMAGE_ID, root))).seal
        );
    }

    function fulfillRequest(ProofRequest memory request, bytes memory journal, address prover)
        internal
        returns (Fulfillment memory, bytes memory assessorSeal)
    {
        ProofRequest[] memory requests = new ProofRequest[](1);
        requests[0] = request;
        bytes[] memory journals = new bytes[](1);
        journals[0] = journal;
        (Fulfillment[] memory fills, bytes memory seal) = fulfillRequestBatch(requests, journals, prover);
        return (fills[0], seal);
    }

    function fulfillRequestBatch(ProofRequest[] memory requests, bytes[] memory journals, address prover)
        internal
        returns (Fulfillment[] memory fills, bytes memory assessorSeal)
    {
        bytes32 root;
        (fills, assessorSeal, root) = createFills(requests, journals, prover, true);
        // submit the root to the set verifier
        publishRoot(root);
        return (fills, assessorSeal);
    }

    function createFills(ProofRequest[] memory requests, bytes[] memory journals, address prover, bool requirePayment)
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
                requestDigest: MessageHashUtils.toTypedDataHash(
                    boundlessMarket.eip712DomainSeparator(), requests[i].eip712Digest()
                ),
                imageId: requests[i].requirements.imageId,
                journal: journals[i],
                seal: bytes(""),
                requirePayment: requirePayment
            });
            fills[i] = fill;
        }

        // compute the assessor claim
        ReceiptClaim memory assessorClaim = TestUtils.mockAssessor(fills, ASSESSOR_IMAGE_ID, prover);
        // compute the batchRoot of the batch Merkle Tree (without the assessor)
        (bytes32 batchRoot, bytes32[][] memory tree) = TestUtils.mockSetBuilder(fills);

        root = MerkleProofish._hashPair(batchRoot, assessorClaim.digest());

        // compute all the inclusion proofs for the fullfillments
        TestUtils.fillInclusionProofs(setVerifier, fills, assessorClaim.digest(), tree);
        // compute the assessor seal
        assessorSeal = TestUtils.mockAssessorSeal(setVerifier, batchRoot);

        return (fills, assessorSeal, root);
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
            vm.prank(address(testProver));
            boundlessMarket.lockin(request, clientSignature);
            requests[i] = request;
            journals[i] = APP_JOURNAL;
        }
    }
}

contract BoundlessMarketBasicTest is BoundlessMarketTest {
    using BoundlessMarketLib for Offer;
    using BoundlessMarketLib for ProofRequest;

    function testDeposit() public {
        vm.deal(address(testProver), 1 ether);
        // Deposit funds into the market
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.Deposit(address(testProver), 1 ether);
        vm.prank(address(testProver));
        boundlessMarket.deposit{value: 1 ether}();
        testProver.expectBalanceChange(1 ether);
    }

    function testWithdraw() public {
        // Deposit funds into the market
        vm.deal(address(testProver), 1 ether);
        vm.prank(address(testProver));
        boundlessMarket.deposit{value: 1 ether}();

        // Withdraw funds from the market
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.Withdrawal(address(testProver), 1 ether);
        vm.prank(address(testProver));
        boundlessMarket.withdraw(1 ether);
        expectMarketBalanceUnchanged();

        // Attempt to withdraw extra funds from the market.
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InsufficientBalance.selector, address(testProver)));
        vm.prank(address(testProver));
        boundlessMarket.withdraw(DEFAULT_BALANCE + 1);
        expectMarketBalanceUnchanged();
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

        // Submit the request with funds
        // Expect the event to be emitted
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.Deposit(address(client), uint256(request.offer.maxPrice));
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestSubmitted(request.id, request, clientSignature);
        vm.deal(address(client), request.offer.maxPrice);
        vm.prank(address(client));
        boundlessMarket.submitRequest{value: request.offer.maxPrice}(request, clientSignature);
    }

    function _testLockin(bool withSig) private returns (Client, ProofRequest memory) {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.sign(request);

        // Expect the event to be emitted
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestLockedin(request.id, address(testProver));
        if (withSig) {
            boundlessMarket.lockinWithSig(request, clientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            boundlessMarket.lockin(request, clientSignature);
        }

        // Ensure the balances are correct
        client.expectBalanceChange(-1 ether);
        testProver.expectHPBalanceChange(-1 ether);

        // Verify the lockin
        assertTrue(boundlessMarket.requestIsLocked(request.id), "Request should be locked-in");

        expectMarketBalanceUnchanged();

        return (client, request);
    }

    function testLockin() public returns (Client, ProofRequest memory) {
        return _testLockin(true);
    }

    function testLockinWithSig() public returns (Client, ProofRequest memory) {
        return _testLockin(false);
    }

    function _testLockinAlreadyLocked(bool withSig) private {
        (Client client, ProofRequest memory request) = _testLockin(withSig);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.sign(request);

        // Attempt to lock in the request again
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsLocked.selector, request.id));
        if (withSig) {
            boundlessMarket.lockinWithSig(request, clientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            boundlessMarket.lockin(request, clientSignature);
        }

        expectMarketBalanceUnchanged();
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
        ProofRequest memory request1 = clientA.request(1);
        ProofRequest memory request2 = clientA.request(2);
        bytes memory proverSignature = testProver.sign(request1);

        // case: request signed by a different client
        bytes memory badClientSignature = clientB.sign(request1);
        vm.expectRevert(IBoundlessMarket.InvalidSignature.selector);
        if (withSig) {
            boundlessMarket.lockinWithSig(request1, badClientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            boundlessMarket.lockin(request1, badClientSignature);
        }

        // case: client signed a different request
        badClientSignature = clientA.sign(request2);
        vm.expectRevert(IBoundlessMarket.InvalidSignature.selector);
        if (withSig) {
            boundlessMarket.lockinWithSig(request1, badClientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            boundlessMarket.lockin(request1, badClientSignature);
        }

        clientA.expectBalanceChange(0 ether);
        clientB.expectBalanceChange(0 ether);
        testProver.expectBalanceChange(0 ether);
        expectMarketBalanceUnchanged();
    }

    function testLockinBadClientSignature() public {
        return _testLockinBadClientSignature(true);
    }

    function testLockinWithSigBadClientSignature() public {
        return _testLockinBadClientSignature(false);
    }

    function testLockinWithSigBadProverSignature() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        // Bad signature is over the wrong request.
        bytes memory badProverSignature = testProver.sign(client.request(2));

        // NOTE: Error is "InsufficientBalance" because we will recover _some_ address.
        // It should be random and never correspond to a real account.
        // TODO: This address will need to change anytime we change the ProofRequest struct or
        // the way it is hashed for signatures. Find a good way to avoid this.
        vm.expectRevert(
            abi.encodeWithSelector(
                IBoundlessMarket.InsufficientBalance.selector,
                address(0x0F34d88fC95E5b27063a97f5ad37BBA958399192)
            )
        );
        boundlessMarket.lockinWithSig(request, clientSignature, badProverSignature);

        client.expectBalanceChange(0 ether);
        testProver.expectBalanceChange(0 ether);
        expectMarketBalanceUnchanged();
    }

    function _testLockinNotEnoughFunds(bool withSig) private {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.sign(request);

        vm.prank(address(client));
        boundlessMarket.withdraw(DEFAULT_BALANCE);

        // case: client does not have enough funds to cover for the lockin
        // should revert with "InsufficientBalance(address requester)"
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InsufficientBalance.selector, address(client)));
        if (withSig) {
            boundlessMarket.lockinWithSig(request, clientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            boundlessMarket.lockin(request, clientSignature);
        }

        vm.prank(address(client));
        boundlessMarket.deposit{value: DEFAULT_BALANCE}();

        vm.prank(address(boundlessMarket));
        hitPoints.lock(address(testProver), uint128(DEFAULT_BALANCE));

        // case: prover does not have enough funds to cover for the lockin stake
        // should revert with "InsufficientBalance(address requester)"
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InsufficientBalance.selector, address(testProver)));
        if (withSig) {
            boundlessMarket.lockinWithSig(request, clientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            boundlessMarket.lockin(request, clientSignature);
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
        ProofRequest memory request = client.request(1);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.sign(request);

        vm.roll(request.offer.deadline() + 1);

        // Attempt to lock in the request after it has expired
        // should revert with "RequestIsExpired({requestId: request.id, deadline: deadline})"
        vm.expectRevert(
            abi.encodeWithSelector(IBoundlessMarket.RequestIsExpired.selector, request.id, request.offer.deadline())
        );
        if (withSig) {
            boundlessMarket.lockinWithSig(request, clientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            boundlessMarket.lockin(request, clientSignature);
        }

        expectMarketBalanceUnchanged();
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
        ProofRequest memory request = client.request(1, offer);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.sign(request);

        // Attempt to lockin a request with maxPrice smaller than minPrice
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InvalidRequest.selector));
        if (withSig) {
            boundlessMarket.lockinWithSig(request, clientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            boundlessMarket.lockin(request, clientSignature);
        }

        expectMarketBalanceUnchanged();
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
        ProofRequest memory request = client.request(1, offer);
        bytes memory clientSignature = client.sign(request);
        bytes memory proverSignature = testProver.sign(request);

        // Attempt to lockin a request with rampUpPeriod greater than timeout
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InvalidRequest.selector));
        if (withSig) {
            boundlessMarket.lockinWithSig(request, clientSignature, proverSignature);
        } else {
            vm.prank(address(testProver));
            boundlessMarket.lockin(request, clientSignature);
        }

        expectMarketBalanceUnchanged();
    }

    function testLockinInvalidRequest2() public {
        return _testLockinInvalidRequest2(true);
    }

    function testLockinWithSigInvalidRequest2() public {
        return _testLockinInvalidRequest2(false);
    }

    enum LockinMethod {
        Lockin,
        LockinWithSig,
        None
    }

    // Base for fulfillment tests with different methods for lockin, including none. All paths should yield the same result.
    function _testFulfill(uint32 requestIdx, LockinMethod lockinMethod) private returns (Client, ProofRequest memory) {
        Client client = getClient(1);
        ProofRequest memory request = client.request(requestIdx);
        bytes memory clientSignature = client.sign(request);

        client.snapshotBalance();
        testProver.snapshotBalance();

        if (lockinMethod == LockinMethod.Lockin) {
            vm.prank(address(testProver));
            boundlessMarket.lockin(request, clientSignature);
        } else if (lockinMethod == LockinMethod.LockinWithSig) {
            boundlessMarket.lockinWithSig(request, clientSignature, testProver.sign(request));
        }

        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, address(testProver));

        if (lockinMethod == LockinMethod.None) {
            // Annoying boilerplate for creating singleton lists.
            Fulfillment[] memory fills = new Fulfillment[](1);
            fills[0] = fill;
            ProofRequest[] memory requests = new ProofRequest[](1);
            requests[0] = request;
            bytes[] memory clientSignatures = new bytes[](1);
            clientSignatures[0] = client.sign(request);

            vm.expectEmit(true, true, true, true);
            emit IBoundlessMarket.RequestFulfilled(request.id);
            vm.expectEmit(true, true, true, false);
            emit IBoundlessMarket.ProofDelivered(request.id, hex"", hex"");
            boundlessMarket.priceAndFulfillBatch(requests, clientSignatures, fills, assessorSeal, address(testProver));
        } else {
            vm.expectEmit(true, true, true, true);
            emit IBoundlessMarket.RequestFulfilled(request.id);
            vm.expectEmit(true, true, true, false);
            emit IBoundlessMarket.ProofDelivered(request.id, hex"", hex"");
            boundlessMarket.fulfill(fill, assessorSeal, address(testProver));
        }

        // Check that the proof was submitted
        assertTrue(boundlessMarket.requestIsFulfilled(fill.id), "Request should have fulfilled status");

        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();

        return (client, request);
    }

    function testFulfillViaLockin() public {
        _testFulfill(1, LockinMethod.Lockin);
    }

    function testFulfillViaLockinWithSig() public {
        _testFulfill(1, LockinMethod.LockinWithSig);
    }

    function testFulfillWithoutLockin() public {
        _testFulfill(1, LockinMethod.None);
    }

    /// Fulfill without lockin should still work even if the prover does not have HP.
    function testFulfillWithoutLockinNoHP() public {
        vm.prank(OWNER_WALLET.addr);
        hitPoints.lock(address(testProver), DEFAULT_BALANCE);

        _testFulfill(1, LockinMethod.None);
    }

    // Check that a single client can create many requests, with the full range of indices, and
    // complete the flow each time.
    function testFulfillRangeOfRequestIdx() public {
        for (uint32 idx = 0; idx < 512; idx++) {
            _testFulfill(idx, LockinMethod.Lockin);
        }
        _testFulfill(0xdeadbeef, LockinMethod.Lockin);
        _testFulfill(0xffffffff, LockinMethod.Lockin);
    }

    // TODO Refactor and move this test
    function testFulfillBatch() public {
        // Provide a batch definition as an array of clients and how many requests each submits.
        uint256[5] memory batch = [uint256(1), 2, 1, 3, 1];
        uint256 batchSize = 0;
        for (uint256 i = 0; i < batch.length; i++) {
            batchSize += batch[i];
        }

        ProofRequest[] memory requests = new ProofRequest[](batchSize);
        bytes[] memory journals = new bytes[](batchSize);
        uint256 expectedRevenue = 0;
        uint256 idx = 0;
        for (uint256 i = 0; i < batch.length; i++) {
            Client client = getClient(i);

            for (uint256 j = 0; j < batch[i]; j++) {
                ProofRequest memory request = client.request(uint32(j));

                // TODO: This is a fragile part of this test. It should be improved.
                uint256 desiredPrice = uint256(1.5 ether);
                vm.roll(request.offer.blockAtPrice(desiredPrice));
                expectedRevenue += desiredPrice;

                boundlessMarket.lockinWithSig(request, client.sign(request), testProver.sign(request));

                requests[idx] = request;
                journals[idx] = APP_JOURNAL;
                idx++;
            }
        }

        (Fulfillment[] memory fills, bytes memory assessorSeal) =
            fulfillRequestBatch(requests, journals, address(testProver));

        for (uint256 i = 0; i < fills.length; i++) {
            vm.expectEmit(true, true, true, true);
            emit IBoundlessMarket.RequestFulfilled(fills[i].id);
            vm.expectEmit(true, true, true, false);
            emit IBoundlessMarket.ProofDelivered(fills[i].id, hex"", hex"");
        }
        boundlessMarket.fulfillBatch(fills, assessorSeal, address(testProver));

        for (uint256 i = 0; i < fills.length; i++) {
            // Check that the proof was submitted
            assertTrue(boundlessMarket.requestIsFulfilled(fills[i].id), "Request should have fulfilled status");
        }

        testProver.expectBalanceChange(int256(uint256(expectedRevenue)));
        expectMarketBalanceUnchanged();
    }

    function testFulfillDistinctProversRequirePayment() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(3);

        boundlessMarket.lockinWithSig(request, client.sign(request), testProver.sign(request));
        // address(3) is just a standin for some other address.
        address mockOtherProverAddr = address(uint160(3));
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, mockOtherProverAddr);

        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsLocked.selector, request.id));
        boundlessMarket.fulfill(fill, assessorSeal, mockOtherProverAddr);

        assertFalse(boundlessMarket.requestIsFulfilled(fill.id), "Request should not have fulfilled status");

        // Prover should have their original balance.
        testProver.expectHPBalanceChange(-int256(uint256(request.offer.lockinStake)));
        expectMarketBalanceUnchanged();
    }

    function testFulfillDistinctProversNoPayment() public returns (Client, ProofRequest memory) {
        Client client = getClient(1);
        ProofRequest memory request = client.request(3);

        boundlessMarket.lockinWithSig(request, client.sign(request), testProver.sign(request));
        // address(3) is just a standin for some other address.
        address mockOtherProverAddr = address(uint160(3));
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, mockOtherProverAddr);
        fill.requirePayment = false;

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.PaymentRequirementsFailed(
            abi.encodeWithSelector(IBoundlessMarket.RequestIsLocked.selector, request.id)
        );
        boundlessMarket.fulfill(fill, assessorSeal, mockOtherProverAddr);

        assertTrue(boundlessMarket.requestIsFulfilled(fill.id), "Request should have fulfilled status");

        // Prover should have their original balance.
        testProver.expectHPBalanceChange(-int256(uint256(request.offer.lockinStake)));
        expectMarketBalanceUnchanged();

        return (client, request);
    }

    // In some cases, a request can be fulfilled without payment being sent. This test starts with
    // one of those cases and checks that the prover can submit fulfillment again to get payment.
    function testCollectPaymentOnFulfilledRequest() public {
        (, ProofRequest memory request) = testFulfillDistinctProversNoPayment();

        testProver.snapshotBalance();
        testProver.snapshotHPBalance();

        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, address(testProver));
        boundlessMarket.fulfill(fill, assessorSeal, address(testProver));

        assertTrue(boundlessMarket.requestIsFulfilled(fill.id), "Request should have fulfilled status");

        // Prover should now have received back their stake plus payment for the request.
        testProver.expectBalanceChange(1 ether);
        testProver.expectHPBalanceChange(1 ether);
        expectMarketBalanceUnchanged();
    }

    function testFulfillFulfillProverAddrDoesNotMatchAssessorReceipt() public {
        Client client = getClient(1);

        ProofRequest memory request = client.request(3);

        boundlessMarket.lockinWithSig(request, client.sign(request), testProver.sign(request));
        // address(3) is just a standin for some other address.
        address mockOtherProverAddr = address(uint160(3));
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, address(testProver));

        vm.expectRevert(VerificationFailed.selector);
        boundlessMarket.fulfill(fill, assessorSeal, mockOtherProverAddr);

        // Prover should have their original balance less the stake amount.
        testProver.expectHPBalanceChange(-int256(uint256(request.offer.lockinStake)));
        expectMarketBalanceUnchanged();
    }

    function testPriceAndFulfill() external {
        Client client = getClient(1);
        ProofRequest memory request = client.request(3);

        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, address(testProver));

        Fulfillment[] memory fills = new Fulfillment[](1);
        fills[0] = fill;
        ProofRequest[] memory requests = new ProofRequest[](1);
        requests[0] = request;
        bytes[] memory clientSignatures = new bytes[](1);
        clientSignatures[0] = client.sign(request);

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(request.id, hex"", hex"");
        boundlessMarket.priceAndFulfillBatch(requests, clientSignatures, fills, assessorSeal, address(testProver));

        assertTrue(boundlessMarket.requestIsFulfilled(fill.id), "Request should have fulfilled status");

        client.expectBalanceChange(-1 ether);
        testProver.expectBalanceChange(1 ether);
        expectMarketBalanceUnchanged();
    }

    function _testFulfillAlreadyFulfilled(uint32 idx, LockinMethod lockinMethod) private {
        (, ProofRequest memory request) = _testFulfill(idx, lockinMethod);

        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, address(testProver));
        // Attempt to fulfill a request already fulfilled
        // should revert with "RequestIsFulfilled({requestId: request.id})"
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsFulfilled.selector, request.id));
        boundlessMarket.fulfill(fill, assessorSeal, address(testProver));

        expectMarketBalanceUnchanged();
    }

    function testFulfillAlreadyFulfilled() public {
        _testFulfillAlreadyFulfilled(1, LockinMethod.Lockin);
        _testFulfillAlreadyFulfilled(2, LockinMethod.LockinWithSig);
        _testFulfillAlreadyFulfilled(3, LockinMethod.None);
    }

    function testFulfillRequestNotLocked() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, address(testProver));

        // Attempt to fulfill a request without locking or pricing it.
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsNotPriced.selector, request.id));
        boundlessMarket.fulfill(fill, assessorSeal, address(testProver));

        expectMarketBalanceUnchanged();
    }

    function testFulfillExpired() public returns (Client, ProofRequest memory) {
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);

        boundlessMarket.lockinWithSig(request, client.sign(request), testProver.sign(request));
        (Fulfillment memory fill, bytes memory assessorSeal) = fulfillRequest(request, APP_JOURNAL, address(testProver));

        vm.roll(request.offer.deadline() + 1);

        // Attempt to fulfill an expired request
        // should revert with "RequestIsExpired({requestId: request.id, deadline: deadline})"
        vm.expectRevert(
            abi.encodeWithSelector(IBoundlessMarket.RequestIsExpired.selector, request.id, request.offer.deadline())
        );
        boundlessMarket.fulfill(fill, assessorSeal, address(testProver));

        // Prover should have their original balance less the stake amount.
        testProver.expectHPBalanceChange(-int256(uint256(request.offer.lockinStake)));
        expectMarketBalanceUnchanged();

        return (client, request);
    }

    function _testFulfillRepeatIndex(LockinMethod lockinMethod) private {
        Client client = getClient(1);

        // Create two distinct requests with the same ID. It should be the case that only one can be
        // filled, and if one is locked, the other cannot be filled.
        Offer memory offerA = client.defaultOffer();
        Offer memory offerB = client.defaultOffer();
        offerB.maxPrice = 3 ether;
        ProofRequest memory requestA = client.request(1, offerA);
        ProofRequest memory requestB = client.request(1, offerB);
        bytes memory clientSignatureA = client.sign(requestA);

        client.snapshotBalance();
        testProver.snapshotBalance();

        // Lock-in request A.
        if (lockinMethod == LockinMethod.Lockin) {
            vm.prank(address(testProver));
            boundlessMarket.lockin(requestA, clientSignatureA);
        } else if (lockinMethod == LockinMethod.LockinWithSig) {
            boundlessMarket.lockinWithSig(requestA, clientSignatureA, testProver.sign(requestA));
        }

        // Attempt to fill request B.
        (Fulfillment memory fill, bytes memory assessorSeal) =
            fulfillRequest(requestB, APP_JOURNAL, address(testProver));

        if (lockinMethod == LockinMethod.None) {
            // Annoying boilerplate for creating singleton lists.
            Fulfillment[] memory fills = new Fulfillment[](1);
            fills[0] = fill;
            // Here we price with request A and try to fill with request B.
            ProofRequest[] memory requests = new ProofRequest[](1);
            requests[0] = requestA;
            bytes[] memory clientSignatures = new bytes[](1);
            clientSignatures[0] = clientSignatureA;

            vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsNotPriced.selector, requestA.id));
            boundlessMarket.priceAndFulfillBatch(requests, clientSignatures, fills, assessorSeal, address(testProver));
        } else {
            vm.expectRevert(
                abi.encodeWithSelector(
                    IBoundlessMarket.RequestLockFingerprintDoesNotMatch.selector,
                    requestA.id,
                    bytes8(
                        MessageHashUtils.toTypedDataHash(
                            boundlessMarket.eip712DomainSeparator(), requestB.eip712Digest()
                        )
                    ),
                    bytes8(
                        MessageHashUtils.toTypedDataHash(
                            boundlessMarket.eip712DomainSeparator(), requestA.eip712Digest()
                        )
                    )
                )
            );
            boundlessMarket.fulfill(fill, assessorSeal, address(testProver));
        }

        // Check that the request ID is not marked as fulfilled.
        assertTrue(!boundlessMarket.requestIsFulfilled(fill.id), "Request should have fulfilled status");

        if (lockinMethod == LockinMethod.None) {
            client.expectBalanceChange(0 ether);
            testProver.expectBalanceChange(0 ether);
        } else {
            client.expectBalanceChange(-1 ether);
            testProver.expectHPBalanceChange(-1 ether);
        }
        expectMarketBalanceUnchanged();
    }

    function testFulfillViaLockinRepeatIndex() public {
        _testFulfillRepeatIndex(LockinMethod.Lockin);
    }

    function testFulfillViaLockinWithSigRepeatIndex() public {
        _testFulfillRepeatIndex(LockinMethod.LockinWithSig);
    }

    function testFulfillWithoutLockinRepeatIndex() public {
        _testFulfillRepeatIndex(LockinMethod.None);
    }

    function testSlash() public returns (Client, ProofRequest memory) {
        (Client client, ProofRequest memory request) = testFulfillExpired();

        // Slash the request
        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.ProverSlashed(request.id, request.offer.lockinStake, 0);
        boundlessMarket.slash(request.id);

        // NOTE: This should be updated if not all the stake burned.
        client.expectBalanceChange(0 ether);
        testProver.expectHPBalanceChange(-int256(uint256(request.offer.lockinStake)));
        expectHPSupplyBurned(request.offer.lockinStake);

        return (client, request);
    }

    function testSlashInvalidRequestID() public {
        // Attempt to slash an invalid request ID
        // should revert with "RequestIsNotLocked({requestId: request.id})"
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsNotLocked.selector, 0xa));
        boundlessMarket.slash(0xa);

        expectMarketBalanceUnchanged();
    }

    function testSlashNotExpired() public {
        (, ProofRequest memory request) = testLockin();

        // Attempt to slash a request not expired
        // should revert with "RequestIsNotExpired({requestId: request.id,  deadline: deadline})"
        vm.expectRevert(
            abi.encodeWithSelector(IBoundlessMarket.RequestIsNotExpired.selector, request.id, request.offer.deadline())
        );
        boundlessMarket.slash(request.id);

        expectMarketBalanceUnchanged();
    }

    function _testSlashFulfilled(uint32 idx, LockinMethod lockinMethod) private {
        (, ProofRequest memory request) = _testFulfill(idx, lockinMethod);

        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsNotLocked.selector, request.id));
        boundlessMarket.slash(request.id);

        expectMarketBalanceUnchanged();
    }

    function testSlashFulfilled() public {
        _testFulfill(1, LockinMethod.Lockin);
        _testFulfill(2, LockinMethod.LockinWithSig);
        _testFulfill(3, LockinMethod.None);
    }

    function testSlashSlash() public {
        (, ProofRequest memory request) = testSlash();

        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsNotLocked.selector, request.id));
        boundlessMarket.slash(request.id);

        expectHPSupplyBurned(request.offer.lockinStake);
    }

    function testSubmitRootAndFulfillBatch() public {
        (ProofRequest[] memory requests, bytes[] memory journals) = newBatch(2);
        (Fulfillment[] memory fills, bytes memory assessorSeal, bytes32 root) =
            createFills(requests, journals, address(testProver), true);

        bytes memory seal =
            verifier.mockProve(SET_BUILDER_IMAGE_ID, sha256(abi.encodePacked(SET_BUILDER_IMAGE_ID, root))).seal;
        boundlessMarket.submitRootAndFulfillBatch(root, seal, fills, assessorSeal, address(testProver));

        for (uint256 j = 0; j < fills.length; j++) {
            assertTrue(boundlessMarket.requestIsFulfilled(fills[j].id), "Request should have fulfilled status");
        }
    }
}

contract BoundlessMarketBench is BoundlessMarketTest {
    using BoundlessMarketLib for Offer;

    function benchFulfillBatch(uint256 batchSize) public {
        (ProofRequest[] memory requests, bytes[] memory journals) = newBatch(batchSize);
        (Fulfillment[] memory fills, bytes memory assessorSeal) =
            fulfillRequestBatch(requests, journals, address(testProver));

        uint256 gasBefore = gasleft();
        boundlessMarket.fulfillBatch(fills, assessorSeal, address(testProver));
        uint256 gasAfter = gasleft();
        // Calculate the gas used
        uint256 gasUsed = gasBefore - gasAfter;
        console2.log(
            "fulfillBatch - gas used: total = %d, batch-size = %d, per-order = %d",
            gasUsed,
            batchSize,
            gasUsed / batchSize
        );

        for (uint256 j = 0; j < fills.length; j++) {
            assertTrue(boundlessMarket.requestIsFulfilled(fills[j].id), "Request should have fulfilled status");
        }
    }

    // Benchmark fulfillBatch with different batch sizes
    // use the following command to run the benchmark:
    // forge test -vv --match-test "testBenchFulfillBatch"

    function testBenchFulfillBatch001() public {
        benchFulfillBatch(1);
    }

    function testBenchFulfillBatch002() public {
        benchFulfillBatch(2);
    }

    function testBenchFulfillBatch004() public {
        benchFulfillBatch(4);
    }

    function testBenchFulfillBatch008() public {
        benchFulfillBatch(8);
    }

    function testBenchFulfillBatch016() public {
        benchFulfillBatch(16);
    }

    function testBenchFulfillBatch032() public {
        benchFulfillBatch(32);
    }

    function testBenchFulfillBatch064() public {
        benchFulfillBatch(64);
    }

    function testBenchFulfillBatch128() public {
        benchFulfillBatch(128);
    }
}

contract BoundlessMarketUpgradeTest is BoundlessMarketTest {
    using BoundlessMarketLib for Offer;

    function testUnsafeUpgrade() public {
        vm.startPrank(OWNER_WALLET.addr);
        proxy = UnsafeUpgrades.deployUUPSProxy(
            address(new BoundlessMarket(setVerifier, ASSESSOR_IMAGE_ID)),
            abi.encodeCall(BoundlessMarket.initialize, (OWNER_WALLET.addr, "https://assessor.dev.null", address(0)))
        );
        boundlessMarket = BoundlessMarket(proxy);
        address implAddressV1 = UnsafeUpgrades.getImplementationAddress(proxy);

        // Should emit an `Upgraded` event
        vm.expectEmit(false, true, true, true);
        emit IERC1967.Upgraded(address(0));
        UnsafeUpgrades.upgradeProxy(
            proxy, address(new BoundlessMarket(setVerifier, ASSESSOR_IMAGE_ID)), "", OWNER_WALLET.addr
        );
        vm.stopPrank();
        address implAddressV2 = UnsafeUpgrades.getImplementationAddress(proxy);

        assertFalse(implAddressV2 == implAddressV1);

        (bytes32 imageID, string memory imageUrl) = boundlessMarket.imageInfo();
        assertEq(imageID, ASSESSOR_IMAGE_ID, "Image ID should be the same after upgrade");
        assertEq(imageUrl, "https://assessor.dev.null", "Image URL should be the same after upgrade");
    }

    function testTransferOwnership() public {
        address newOwner = vm.createWallet("NEW_OWNER").addr;
        vm.prank(OWNER_WALLET.addr);
        boundlessMarket.transferOwnership(newOwner);

        vm.prank(newOwner);
        boundlessMarket.acceptOwnership();

        assertEq(boundlessMarket.owner(), newOwner, "Owner should be changed");
    }
}

contract MerkleProofishTest is Test {
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
