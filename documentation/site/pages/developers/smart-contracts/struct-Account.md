# Account

Represents the account state, including balance and request flags.

```solidity
struct Account {
    uint96 balance;
    uint96 stakeBalance;
    uint64 requestFlagsInitial;
    uint256[(1 << 32) * REQUEST_FLAGS_BITWIDTH / 256] requestFlagsExtended;
}
```
