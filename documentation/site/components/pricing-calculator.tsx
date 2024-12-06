import NumberFlow from "@number-flow/react";
import { Skeleton } from "@risc0/ui/skeleton";
import { useEffect, useState } from "react";
import { useBlockNumber, usePublicClient } from "wagmi";

const SEPOLIA_CHAIN_ID = 11_155_111;

type PricingInputs = {
  programCycles: number;
  desiredTimeMinutes: number;
};

function calculateSuggestion(cycles: number, minutes: number, blocksPerMinute: number) {
  const basePrice = (cycles / 1_000_000) * 0.0001;
  const biddingStartDelay = Math.ceil(cycles / (30 * 1_000_000)); // cycles / 30MHz in blocks

  return {
    minPrice: basePrice,
    maxPrice: basePrice * 2,
    biddingStartDelay,
    rampUpBlocks: Math.min(100, Math.ceil(minutes * 0.5 * blocksPerMinute)), // Cap at 100 blocks
    timeoutBlocks: Math.ceil(minutes * blocksPerMinute),
    lockInStake: basePrice * 4,
  };
}

export default function PricingCalculator() {
  const { data: blockNumber } = useBlockNumber({ chainId: SEPOLIA_CHAIN_ID });
  const publicClient = usePublicClient({ chainId: SEPOLIA_CHAIN_ID });
  const [blocksPerMinute, setBlocksPerMinute] = useState<number | undefined>(undefined);
  const [inputs, setInputs] = useState<PricingInputs>({
    programCycles: 1_000_000,
    desiredTimeMinutes: 10,
  });

  const handleNumericInput = (e, field: keyof PricingInputs) => {
    const value = e.target.value.replace(/[^0-9]/g, "");
    setInputs((prev) => ({
      ...prev,
      [field]: value ? Number(value) : 0,
    }));
  };

  useEffect(() => {
    async function calculateBlocksPerMinute() {
      if (!blockNumber || !publicClient) {
        return;
      }

      try {
        // Get current block timestamp
        const currentBlock = await publicClient.getBlock();

        // Get block from ~5 minutes ago
        const pastBlock = await publicClient.getBlock({
          blockNumber: blockNumber - 25n, // ~5 minutes worth of blocks
        });

        if (currentBlock.timestamp && pastBlock.timestamp) {
          const timeDiffMinutes = (Number(currentBlock.timestamp) - Number(pastBlock.timestamp)) / 60;
          const blockDiff = Number(currentBlock.number - pastBlock.number);
          const actualBlocksPerMinute = blockDiff / timeDiffMinutes;

          setBlocksPerMinute(Math.round(actualBlocksPerMinute));
        }
      } catch (error) {
        console.error("Failed to calculate blocks per minute:", error);
      }
    }

    calculateBlocksPerMinute();
  }, [blockNumber, publicClient]);

  const suggestion = blocksPerMinute
    ? calculateSuggestion(inputs.programCycles, inputs.desiredTimeMinutes, blocksPerMinute)
    : null;

  return (
    <div className="my-8 rounded-lg border border-[var(--vocs-color_border);] p-6">
      {suggestion ? (
        <>
          <h3 className="mb-4 font-semibold text-lg">Request Parameters Calculator</h3>

          <div className="space-y-4">
            <div>
              <label htmlFor="programCycles" className="mb-1 block text-sm">
                Program Cycles
              </label>
              <input
                id="programCycles"
                value={inputs.programCycles}
                onChange={(e) => handleNumericInput(e, "programCycles")}
                className="w-full rounded border border-[var(--vocs-color_border);] px-3 py-2"
              />
            </div>

            <div>
              <label htmlFor="desiredTimeMinutes" className="mb-1 block text-sm">
                Desired Proof Time (in minutes)
              </label>
              <input
                id="desiredTimeMinutes"
                value={inputs.desiredTimeMinutes}
                onChange={(e) => handleNumericInput(e, "desiredTimeMinutes")}
                className="w-full rounded border border-[var(--vocs-color_border);] px-3 py-2"
              />
            </div>

            <div className="pt-4">
              <h4 className="mb-2 font-medium">Suggested Parameters</h4>
              <div className="rounded border border-[var(--vocs-color_border);] bg-muted p-4">
                <dl className="space-y-2 text-sm">
                  <div className="flex justify-between">
                    <dt>Minimum Price:</dt>
                    <dd>
                      <NumberFlow value={suggestion.minPrice} /> Sepolia ETH
                    </dd>
                  </div>
                  <div className="flex justify-between">
                    <dt>Maximum Price:</dt>
                    <dd>
                      <NumberFlow value={suggestion.maxPrice} /> Sepolia ETH
                    </dd>
                  </div>
                  <div className="flex justify-between">
                    <dt>Bidding Start Delay:</dt>
                    <dd>
                      <NumberFlow value={suggestion.biddingStartDelay} /> blocks
                    </dd>
                  </div>
                  <div className="flex justify-between">
                    <dt>Ramp-up Period:</dt>
                    <dd>
                      <NumberFlow value={suggestion.rampUpBlocks} /> blocks
                    </dd>
                  </div>
                  <div className="flex justify-between">
                    <dt>Timeout:</dt>
                    <dd>
                      <NumberFlow value={suggestion.timeoutBlocks} /> blocks
                    </dd>
                  </div>
                  <div className="flex justify-between">
                    <dt>Lock-in Stake:</dt>
                    <dd>
                      <NumberFlow value={suggestion.lockInStake} /> Sepolia ETH
                    </dd>
                  </div>
                </dl>
              </div>
            </div>
          </div>
        </>
      ) : (
        <Skeleton className="h-[380px] w-full" />
      )}
    </div>
  );
}
