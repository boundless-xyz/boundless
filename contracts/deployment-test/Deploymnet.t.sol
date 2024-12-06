// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.9;

import {Test} from "forge-std/Test.sol";
import {console2} from "forge-std/console2.sol";
import {Vm} from "forge-std/Vm.sol";
import {MessageHashUtils} from "@openzeppelin/contracts/utils/cryptography/MessageHashUtils.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {IRiscZeroSetVerifier} from "risc0/IRiscZeroSetVerifier.sol";
import {
    IBoundlessMarket,
    ProofRequest,
    Requirements,
    Offer,
    Predicate,
    Input,
    InputType,
    PredicateType,
    Fulfillment
} from "../src/IBoundlessMarket.sol";
import {BoundlessMarket} from "../src/BoundlessMarket.sol";
import {BoundlessMarketLib} from "../src/BoundlessMarketLib.sol";
import {ConfigLoader, DeploymentConfig} from "../scripts/Config.s.sol";

Vm constant VM = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);
bytes32 constant APP_IMAGE_ID = 0x257569e11f856439ec3c1e0fe6486fb9af90b1da7324d577f65dd0d45ec12c7d;
uint256 constant DEFAULT_BALANCE = 1000 ether;

/// Test designed to be run against a chain with an active deployment of the RISC Zero contracts.
/// Checks that the deployment matches what is recorded in the deployment.toml file.
contract DeploymentTest is Test {
    // Path to deployment config file, relative to the project root.
    string constant CONFIG_FILE = "contracts/deployment.toml";
    // Load the deployment config
    DeploymentConfig internal deployment;

    IRiscZeroVerifier internal verifier;
    IRiscZeroSetVerifier internal setVerifier;
    IBoundlessMarket internal boundlessMarket;

    mapping(uint256 => Client) internal clients;

    struct OrderFulfilled {
        bytes32 root;
        bytes seal;
        Fulfillment[] fills;
        bytes assessorSeal;
        address prover;
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

        clients[index] = client;
        return client;
    }

    // Create a client, using a trick to set the address equal to the wallet address.
    function createClientContract(string memory identifier) internal returns (Client) {
        address payable clientAddress = payable(vm.createWallet(identifier).addr);
        vm.allowCheatcodes(clientAddress);
        vm.etch(clientAddress, address(new Client()).code);
        Client client = Client(clientAddress);
        client.initialize(identifier, boundlessMarket);
        return client;
    }

    function setUp() external {
        // Load the deployment config
        deployment = ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG_FILE));

        verifier = IRiscZeroVerifier(deployment.verifier);
        setVerifier = IRiscZeroSetVerifier(deployment.setVerifier);
        boundlessMarket = IBoundlessMarket(deployment.boundlessMarket);
    }

    function testAdminIsSet() external view {
        require(deployment.admin != address(0), "no admin address is set");
    }

    function testRouterIsDeployed() external view {
        require(address(verifier) != address(0), "no verifier (router) address is set");
        require(keccak256(address(verifier).code) != keccak256(bytes("")), "verifier code is empty");
    }

    function testSetVerifierIsDeployed() external view {
        require(address(setVerifier) != address(0), "no set verifier address is set");
        require(keccak256(address(setVerifier).code) != keccak256(bytes("")), "set verifier code is empty");
    }

    function testBoundlessMarketIsDeployed() external view {
        require(address(boundlessMarket) != address(0), "no boundless market address is set");
        require(keccak256(address(boundlessMarket).code) != keccak256(bytes("")), "boundless market code is empty");
    }

    function testBoundlessMarketOwner() external view {
        require(
            deployment.admin == BoundlessMarket(address(boundlessMarket)).owner(),
            "boundless market owner does not match admin"
        );
    }

    function testAssessorInfo() external view {
        (bytes32 assessorImageId, string memory assessorGuestUrl) = boundlessMarket.imageInfo();
        require(deployment.assessorImageId == assessorImageId, "assessor image ID does not match");
        require(
            keccak256(abi.encode(deployment.assessorGuestUrl)) == keccak256(abi.encode(assessorGuestUrl)),
            "assessor guest URL does not match"
        );
    }

    function testPriceAndFulfillBatch() external {
        Client testProver = createClientContract("PROVER");
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);

        ProofRequest[] memory requests = new ProofRequest[](1);
        requests[0] = request;
        bytes[] memory clientSignatures = new bytes[](1);
        clientSignatures[0] = client.sign(request);

        (, string memory setBuilderUrl) = setVerifier.imageInfo();
        (, string memory assessorUrl) = boundlessMarket.imageInfo();

        string[] memory argv = new string[](15);
        uint256 i = 0;
        argv[i++] = "boundless-ffi";
        argv[i++] = "--set-builder-url";
        argv[i++] = setBuilderUrl;
        argv[i++] = "--assessor-url";
        argv[i++] = assessorUrl;
        argv[i++] = "--boundless-market-address";
        argv[i++] = vm.toString(address(boundlessMarket));
        argv[i++] = "--chain-id";
        argv[i++] = vm.toString(block.chainid);
        argv[i++] = "--prover-address";
        argv[i++] = vm.toString(address(testProver));
        argv[i++] = "--request";
        argv[i++] = vm.toString(abi.encode(request));
        argv[i++] = "--signature";
        argv[i++] = vm.toString(clientSignatures[0]);

        OrderFulfilled memory result = abi.decode(vm.ffi(argv), (OrderFulfilled));

        setVerifier.submitMerkleRoot(result.root, result.seal);

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id);
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(request.id, hex"", hex"");

        boundlessMarket.priceAndFulfillBatch(
            requests, clientSignatures, result.fills, result.assessorSeal, address(testProver)
        );
        Fulfillment memory fill = result.fills[0];
        assertTrue(boundlessMarket.requestIsFulfilled(fill.id), "Request should have fulfilled status");
    }
}

contract Client {
    using BoundlessMarketLib for Requirements;
    using BoundlessMarketLib for ProofRequest;
    using BoundlessMarketLib for Offer;

    string public identifier;
    Vm.Wallet public wallet;
    IBoundlessMarket public boundlessMarket;

    receive() external payable {}

    function initialize(string memory _identifier, IBoundlessMarket _boundlessMarket) public {
        identifier = _identifier;
        boundlessMarket = _boundlessMarket;
        wallet = VM.createWallet(identifier);
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
            predicate: Predicate({predicateType: PredicateType.PrefixMatch, data: hex"57656420"})
        });
    }

    function request(uint32 idx) public view returns (ProofRequest memory) {
        return ProofRequest({
            id: BoundlessMarketLib.requestId(wallet.addr, idx),
            requirements: defaultRequirements(),
            imageUrl: "https://dweb.link/ipfs/QmTx3vDKicYG5RxzMxrZEiCQJqhpgYNrSFABdVz9ri2m5P",
            input: Input({
                inputType: InputType.Inline,
                data: hex"1d000000570000006500000064000000200000004a000000750000006c0000002000000020000000330000002000000031000000340000003a00000033000000370000003a00000031000000320000002000000050000000440000005400000020000000320000003000000032000000340000000a000000"
            }),
            offer: defaultOffer()
        });
    }

    function sign(ProofRequest memory req) public returns (bytes memory) {
        bytes32 structDigest =
            MessageHashUtils.toTypedDataHash(boundlessMarket.eip712DomainSeparator(), req.eip712Digest());
        (uint8 v, bytes32 r, bytes32 s) = VM.sign(wallet, structDigest);
        return abi.encodePacked(r, s, v);
    }
}
