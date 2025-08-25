import NumberFlow from "@number-flow/react";
import { useState } from "react";

type Network = "base-mainnet" | "base-sepolia";

type PricingInputs = {
  programMegaCycles: number;
  proofDeliveryTime: number;
};

type NetworkConfig = {
  name: string;
  blocksPerMinute: number;
  currencySymbol: string;
};

const NETWORK_CONFIGS: Record<Network, NetworkConfig> = {
  "base-mainnet": {
    name: "Base Mainnet",
    blocksPerMinute: 30, // ~2 second block time
    currencySymbol: "ETH",
  },
  "base-sepolia": {
    name: "Base Sepolia Testnet",
    blocksPerMinute: 30,
    currencySymbol: "Base SepETH",
  },
};

function calculateSuggestion(
  programMegaCycles: number,
  proofDeliveryTime: number,
  networkConfig: NetworkConfig,
) {
  // from Jacob E: hardcode 100 million wei/cycle for max price
  // 10e8 wei / cycle = 10e14 wei / mcycle
  // max price in wei * 10e-18 = max price in eth
  const maxPriceInWei = programMegaCycles  * 1e14
  const maxPrice = maxPriceInWei * 1e-18 ;

  // allow people to execute before bidding go up
  const biddingStartDelay = Math.ceil(programMegaCycles / 30); // assuming 30 Mhz execution trace gen

  // assume 1000 MCycles = $10 USD lock stake
  const lockInStakeUSDC = Math.max(5, ( programMegaCycles / 1000 ) * 10);

  return {
    minPrice: 0,
    maxPrice: Math.min(0.1, maxPrice), // set to 0.1 ETH max
    biddingStartDelay,
    rampUpBlocks: Math.min(100, Math.ceil(proofDeliveryTime * 0.5 * networkConfig.blocksPerMinute)),
    lockTimeoutBlocks: Math.ceil(proofDeliveryTime * networkConfig.blocksPerMinute),
    lockInStake: lockInStakeUSDC,
  };
}

export default function PricingCalculator() {
  const [network, setNetwork] = useState<Network>("base-mainnet");
  const [inputs, setInputs] = useState<PricingInputs>({
    programMegaCycles: 10,
    proofDeliveryTime: 10,
  });
  const [copied, setCopied] = useState(false);

  const networkConfig = NETWORK_CONFIGS[network];
  const suggestion = calculateSuggestion(
    inputs.programMegaCycles,
    inputs.proofDeliveryTime,
    networkConfig,
  );

  const yamlConfig = `offer:
    min_price: ${suggestion.minPrice * 1e18} # wei
    max_price: ${suggestion.maxPrice * 1e18} # wei
    biddingStart: ${suggestion.biddingStartDelay} # blocks
    rampUpPeriod: ${suggestion.rampUpBlocks} # blocks
    timeout: 2700 # seconds
    lockTimeout: ${suggestion.lockTimeoutBlocks} # blocks
    lockStake: ${suggestion.lockInStake * 1e6} # USDC`;

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(yamlConfig);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (err) {
      console.error('Failed to copy text: ', err);
    }
  };

  const handleNumericInput = (e, field: keyof PricingInputs) => {
    const value = e.target.value.replace(/[^0-9]/g, "");
    setInputs((prev) => ({
      ...prev,
      [field]: value ? Number(value) : 0,
    }));
  };

  return (
    <div className="my-8 rounded-lg border border-[var(--vocs-color_border);] p-6">
      <div className="space-y-4">

        {/* Network Dropdown */}
        <div>
          <label htmlFor="network-select" className="mb-2 block font-medium text-sm">
            Network
          </label>
          <div className="relative">
            <select
              id="network-select"
              value={network}
              onChange={(e) => setNetwork(e.target.value as Network)}
              className="w-full rounded border border-[var(--vocs-color_border);] bg-white px-3 py-2 pr-10"
              style={{ appearance: 'none', WebkitAppearance: 'none', MozAppearance: 'none' }}
            >
              <option value="base-sepolia">Base Sepolia</option>
              <option value="base-mainnet">Base Mainnet</option>
            </select>
            <div className="pointer-events-none absolute inset-y-0 right-3 flex items-center">
              <svg className="h-4 w-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" />
              </svg>
            </div>
          </div>
        </div>

        {/* Program Cycles input in MCycles */}
        <div>
          <label htmlFor="programMegaCycles" className="mb-1 block text-sm">
            Program Cycles (MCycles)
          </label>
          <input
            id="programMegaCycles"
            value={inputs.programMegaCycles}
            onChange={(e) => handleNumericInput(e, "programMegaCycles")}
            className="w-full rounded border border-[var(--vocs-color_border);] px-3 py-2"
          />
        </div>

        {/* Proof Delivery Time in minutes */}
        <div>
          <label htmlFor="proofDeliveryTime" className="mb-1 block text-sm">
            Desired Proof Delivery Time (minutes)
          </label>
          <input
            id="proofDeliveryTime"
            value={inputs.proofDeliveryTime}
            onChange={(e) => handleNumericInput(e, "proofDeliveryTime")}
            className="w-full rounded border border-[var(--vocs-color_border);] px-3 py-2"
          />
        </div>

        {/* Suggested Offer Parameters */}
        <div className="pt-4">
          <h4 className="mb-2 font-medium">Suggested Offer Parameters</h4>
          <div className="rounded border border-[var(--vocs-color_border);] bg-muted p-4">
            <dl className="space-y-2 text-sm">
              <div className="flex justify-between">
                <dt>Minimum Price:</dt>
                <dd>
                  <NumberFlow
                    className="font-mono"
                    format={{
                      minimumFractionDigits: 8,
                    }}
                    value={suggestion.minPrice}
                    suffix={` ${networkConfig.currencySymbol}`}
                  />
                </dd>
              </div>
              <div className="flex justify-between">
                <dt>Maximum Price:</dt>
                <dd>
                  <NumberFlow
                    format={{
                      minimumFractionDigits: 8,
                    }}
                    className="font-mono"
                    value={suggestion.maxPrice}
                    suffix={` ${networkConfig.currencySymbol}`}
                  />
                </dd>
              </div>
              <div className="flex justify-between">
                <dt>Bidding Start Delay:</dt>
                <dd>
                  <NumberFlow
                    className="font-mono"
                    value={suggestion.biddingStartDelay}
                    suffix={suggestion.biddingStartDelay === 1 ? " block" : " blocks"}
                  />
                </dd>
              </div>
              <div className="flex justify-between">
                <dt>Ramp-up Period:</dt>
                <dd>
                  <NumberFlow
                    className="font-mono"
                    value={suggestion.rampUpBlocks}
                    suffix={suggestion.rampUpBlocks === 1 ? " block" : " blocks"}
                  />
                </dd>
              </div>
              <div className="flex justify-between">
                <dt>Timeout:</dt>
                <dd>
                  <NumberFlow
                    className="font-mono"
                    value={suggestion.lockTimeoutBlocks}
                    suffix={suggestion.lockTimeoutBlocks === 1 ? " block" : " blocks"}
                  />
                </dd>
              </div>
              <div className="flex justify-between">
                <dt>Lock-in Stake:</dt>
                <dd>
                  <NumberFlow
                    format={{
                      minimumFractionDigits: 6,
                      maximumFractionDigits: 6,
                    }}
                    className="font-mono"
                    value={suggestion.lockInStake}
                    suffix=" USDC"
                  />
                </dd>
              </div>
            </dl>
          </div>
        </div>

        {/* Request YAML Configuration Generator */}
        <div className="pt-4">
          <div className="mb-2 flex items-center justify-between">
            <h4 className="font-medium">
              Offer Parameters {" "}
              <a
                href="https://github.com/boundless-xyz/boundless/blob/main/request.yaml"
                target="_blank"
                rel="noopener noreferrer"
                className="text-[var(--vocs-color_textAccent)] underline hover:opacity-80"
              >
              (request.yaml)
              </a>
            </h4>
            <button
              type="button"
              onClick={handleCopy}
              className="flex items-center gap-1 rounded border border-[var(--vocs-color_border);] px-2 py-1 text-xs transition-colors hover:bg-muted"
            >
              {copied ? (
                <>
                  <svg className="h-3 w-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
                  </svg>
                  Copied!
                </>
              ) : (
                <>
                  <svg className="h-3 w-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" />
                  </svg>
                  Copy
                </>
              )}
            </button>
          </div>
          <div className="rounded border border-[var(--vocs-color_border);] bg-muted p-4">
            <pre className="overflow-x-auto text-sm">
              <code className="language-yaml">
                {yamlConfig}
              </code>
            </pre>
          </div>
        </div>




      </div>
    </div>
  );
}
