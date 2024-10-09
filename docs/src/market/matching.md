# Market Matching Design

## The Reverse-Dutch Auction

We describe the Boundless [reverse-Dutch auction](https://en.wikipedia.org/wiki/Reverse_auction#Dutch_reverse_auctions), a matching and price discovery mechanism for proof requests.

**TL;DR**: The requestor broadcasts a proof request. Initially, the request offers a low reward; eventually the reward begins to increase, until it reaches some max. At any time prior to the request’s expiration/completion, the request can be “locked-in” by a prover who escrows some stake; this gives them the exclusive ability to be paid for a proof, and on success their reward will be based on when they “locked-in” the request.

In slightly more detail, the lifecycle of a successful proving request looks like this:
The requester broadcasts their request (e.g., via calldata on the market). The request includes requirements for the proof (Image ID, predicates on the journal, etc.) as well as an offer (the parameters of the auction), which includes (among other things) the maximum price that the requester is willing to pay.

When publishing their request, the requester escrows the necessary funds (i.e., maximum price) in the market.
An auction is held according to the parameters specified by the offer. During this time, provers can attempt to lock-in the request by submitting a bid. The first bid received by the market wins the auction, and the reward (price paid by the requester) is based on when their bid was received (see Offers below).

To submit a bid, a prover must escrow the necessary stake (described in the offer); the stake is returned to them if they submit the proof prior to the offer’s expiration.
The winning bidder completes the proof before the request expires and is paid according to their bid.

### Offers and Rewards

An offer contains the following:

- Pricing parameters
- Minimum price
- Maximum price
- Bidding start (defined as a block number)
- Length of ramp-up period (measured in blocks since the start of the bid)
- Timeout (measured in blocks since the start of the bid)
- Lock-in stake

For example, an offer might specify:

- Pricing parameters
  - Minimum price: 1 Ether
  - Maximum price: 2 Ether
- Bidding start: Block number 1000
- Length of ramp-up period: 5 blocks
- Timeout: 100 blocks
- Lock-in stake: 4 Ether

The pricing parameters are used to determine the reward that gets paid-out when the request is fulfilled (ie, the proof has been verified). The reward is governed by the price function. Its inputs are:

- The offer;
- The number of blocks that have passed since the bidding started.

The function works like so:

- During the discovery period (the initial phase of the auction before bidding start), the price is just the minimum price.
- During the ramp-up period (which immediately follows the discovery period), the price grows linearly up-to the maximum price.
- After the ramp-up period, the price is just the maximum price.

Continuing with the example offer given above, the price is constant (1 Ether, its minimum) for the first 10 blocks; on block 11 it jumps to 1.2 Ether; on 12 it jumps to 1.4 Ether; on 15 it reaches 2 Ether (its maximum), and remains at that value until the offer expires.

When a prover locks-in a request, they are agreeing to be paid the reward offered by this function at the time of their bid.
