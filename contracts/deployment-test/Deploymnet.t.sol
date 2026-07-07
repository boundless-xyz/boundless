// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {Test} from "forge-std/Test.sol";
import {Vm} from "forge-std/Vm.sol";
import {MessageHashUtils} from "@openzeppelin/contracts/utils/cryptography/MessageHashUtils.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {IRiscZeroSetVerifier} from "risc0/IRiscZeroSetVerifier.sol";
import {IRiscZeroSelectable} from "risc0/IRiscZeroSelectable.sol";

// The deployment-test exercises the deployed market via its current ABI:
// requests are fulfilled through the new batched fulfill path on the new
// BoundlessMarket, so all struct + interface imports come from
// contracts/src/. The legacy ABI served via the fallback is not exercised
// here; in this deployment the legacy impl is a non-functional stub that
// the new path never invokes.
import {IBoundlessMarket} from "../src/IBoundlessMarket.sol";
import {Callback} from "../src/types/Callback.sol";
import {Fulfillment, LegacyFulfillment} from "../src/types/Fulfillment.sol";
import {FulfillmentBatch} from "../src/types/FulfillmentBatch.sol";
import {ProofRequestBatch} from "../src/types/ProofRequestBatch.sol";
import {Input, InputType} from "../src/types/Input.sol";
import {Requirements} from "../src/types/Requirements.sol";
import {Offer} from "../src/types/Offer.sol";
import {ProofRequest} from "../src/types/ProofRequest.sol";
import {PredicateLibrary} from "../src/types/Predicate.sol";
import {RequestIdLibrary} from "../src/types/RequestId.sol";

import {BoundlessMarket} from "../src/BoundlessMarket.sol";
import {BoundlessMarketLib} from "../src/libraries/BoundlessMarketLib.sol";
import {ConfigLoader, DeploymentConfig} from "../scripts/Config.s.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

Vm constant VM = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);
bytes32 constant APP_IMAGE_ID = 0x53cb4210cf2f5bf059e3a4f7bcbb8e21ddc5c11a690fd79e87947f9fec5522a3;
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
    IERC20 internal stakeToken;

    mapping(uint256 => Client) internal clients;

    struct OrderFulfilled {
        /// The root of the set.
        bytes32 root;
        /// The seal of the root.
        bytes seal;
        /// The batched fulfillment to submit.
        FulfillmentBatch fulfillmentBatch;
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
        stakeToken = IERC20(deployment.collateralToken);
    }

    function testAdminIsSet() external view {
        require(deployment.admin2 != address(0), "no admin address is set");
    }

    function testRouterIsDeployed() external view {
        require(address(verifier) != address(0), "no verifier (router) address is set");
        require(keccak256(address(verifier).code) != keccak256(bytes("")), "verifier code is empty");
        require(deployment.boundlessRouter != address(0), "no boundless router address is set");
        require(
            deployment.boundlessRouter == address(BoundlessMarket(payable(address(boundlessMarket))).ROUTER()),
            "boundless router address does not match boundless market"
        );
    }

    function testSetVerifierIsDeployed() external view {
        require(address(setVerifier) != address(0), "no set verifier address is set");
        require(keccak256(address(setVerifier).code) != keccak256(bytes("")), "set verifier code is empty");
    }

    function testBoundlessMarketIsDeployed() external view {
        require(address(boundlessMarket) != address(0), "no boundless market address is set");
        require(keccak256(address(boundlessMarket).code) != keccak256(bytes("")), "boundless market code is empty");
    }

    function testCollateralTokenIsDeployed() external view {
        require(address(stakeToken) != address(0), "no collateral token address is set");
        require(keccak256(address(stakeToken).code) != keccak256(bytes("")), "collateral token code is empty");
        require(
            address(stakeToken) == BoundlessMarket(payable(address(boundlessMarket))).COLLATERAL_TOKEN_CONTRACT(),
            "collateral token address does not match boundless market"
        );
    }

    function testBoundlessMarketOwner() external view {
        require(
            BoundlessMarket(payable(address(boundlessMarket)))
                .hasRole(BoundlessMarket(payable(address(boundlessMarket))).ADMIN_ROLE(), deployment.admin2),
            "boundless market admin role does not match admin"
        );
    }

    function testAssessorInfo() external view {
        // The market no longer exposes imageInfo(); the assessor image id/url and the router
        // assessor selector live in the deployment config (the adapter is registered in the router).
        require(deployment.assessorImageId != bytes32(0), "no assessor image ID is set");
        require(bytes(deployment.assessorGuestUrl).length != 0, "no assessor guest URL is set");
        require(deployment.r0AssessorSelector != bytes4(0), "no R0 assessor selector is set");
    }

    function testPriceAndFulfillWithSelector() external {
        // The fulfillment seal is a set-inclusion seal, so it leads with the set
        // verifier's selector. Sign that exact selector: the router resolves the
        // verifier entry from the seal's leading bytes4 and requires the signed
        // selector to match the entry, its class, or the chain default.
        _testPriceAndFulfillWithSelector(IRiscZeroSelectable(address(setVerifier)).SELECTOR());
    }

    function _testPriceAndFulfillWithSelector(bytes4 selector) internal {
        Client testProver = createClientContract("PROVER");
        Client client = getClient(1);
        ProofRequest memory request = client.request(1);
        request.requirements.selector = selector;

        ProofRequest[] memory requests = new ProofRequest[](1);
        requests[0] = request;
        bytes[] memory clientSignatures = new bytes[](1);
        clientSignatures[0] = client.sign(request);

        (, string memory setBuilderUrl) = setVerifier.imageInfo();
        string memory assessorUrl = deployment.assessorGuestUrl;

        string[] memory argv = new string[](17);
        uint256 i = 0;
        argv[i++] = "boundless-ffi";
        argv[i++] = "--set-builder-url";
        argv[i++] = setBuilderUrl;
        argv[i++] = "--assessor-url";
        argv[i++] = assessorUrl;
        argv[i++] = "--assessor-selector";
        argv[i++] = vm.toString(abi.encodePacked(deployment.r0AssessorSelector));
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

        // The market reconstructs and emits the domain-bound request digest from the SlimRequest.
        bytes32 requestDigest = MessageHashUtils.toTypedDataHash(
            BoundlessMarket(payable(address(boundlessMarket))).eip712DomainSeparator(), request.eip712Digest()
        );

        ProofRequestBatch[] memory requestBatches = new ProofRequestBatch[](1);
        requestBatches[0] = ProofRequestBatch({requests: requests, signatures: clientSignatures});
        FulfillmentBatch[] memory fulfillmentBatches = new FulfillmentBatch[](1);
        fulfillmentBatches[0] = result.fulfillmentBatch;

        // This request is never locked, so priceAndFulfill takes the open path: the #2052
        // front-running guard requires a matching commitment recorded a strictly earlier block.
        boundlessMarket.commitFulfillment(keccak256(abi.encode(fulfillmentBatches)));
        vm.roll(block.number + 1);

        vm.expectEmit(true, true, true, true);
        emit IBoundlessMarket.RequestFulfilled(request.id, address(testProver), requestDigest);
        // ProofDelivered carries the legacy fulfillment shape (id/requestDigest inline) for
        // pre-router client compatibility; reconstruct it from the request identity and the fill.
        Fulfillment memory fill = result.fulfillmentBatch.fills[0];
        vm.expectEmit(true, true, true, false);
        emit IBoundlessMarket.ProofDelivered(
            request.id,
            address(testProver),
            LegacyFulfillment({
                id: request.id,
                requestDigest: requestDigest,
                claimDigest: fill.claimDigest,
                fulfillmentDataType: fill.fulfillmentDataType,
                fulfillmentData: fill.fulfillmentData,
                seal: fill.seal
            })
        );

        boundlessMarket.priceAndFulfill(requestBatches, fulfillmentBatches);

        assertTrue(boundlessMarket.requestIsFulfilled(request.id), "Request should have fulfilled status");
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
            rampUpStart: uint64(block.timestamp),
            rampUpPeriod: uint32(300),
            timeout: uint32(3600),
            lockTimeout: uint32(2700),
            lockCollateral: 1 ether
        });
    }

    function defaultRequirements() public pure returns (Requirements memory) {
        return Requirements({
            predicate: PredicateLibrary.createPrefixMatchPredicate(bytes32(APP_IMAGE_ID), hex"53797374"),
            callback: Callback({gasLimit: 0, addr: address(0)}),
            selector: bytes4(0)
        });
    }

    function request(uint32 idx) public view returns (ProofRequest memory) {
        // create a request as used in `../../request.yaml`.
        return ProofRequest({
            id: RequestIdLibrary.from(wallet.addr, idx),
            requirements: defaultRequirements(),
            imageUrl: "https://gateway.beboundless.cloud/ipfs/bafkreie5vdnixfaiozgnqdfoev6akghj5ek3jftrsjt7uw2nnuiuegqsyu",
            input: Input({
                inputType: InputType.Inline,
                data: hex"0181a5737464696edc003553797374656d54696d65207b2074765f7365633a20313733383030343939382c2074765f6e7365633a20363235373837303030207d"
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
