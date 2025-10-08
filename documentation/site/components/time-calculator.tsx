import React, { useState, useMemo } from "react";

export default function TimeCalculator() {
  const [cycleCount, setCycleCount] = useState(10);
  const [cycleUnit, setCycleUnit] = useState("mcycles");
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [executionMhz, setExecutionMhz] = useState("30");
  const [provingMhz, setProvingMhz] = useState("1");

  const handleNumericInput = (e, setter) => {
    const value = e.target.value.replace(/[^0-9]/g, "");
    setter(value ? Number(value) : 0);
  };

  const handleDecimalInput = (e, setter) => {
    const value = e.target.value;
    // Allow numbers and at most one decimal point
    if (/^\d*\.?\d*$/.test(value)) {
      setter(value);
    }
  };

  const { estimatedExecutionTime, estimatedProvingTime } = useMemo(() => {
    let multiplier = 1;
    if (cycleUnit === "mcycles") {
      multiplier = 1_000_000;
    } else if (cycleUnit === "gcycles") {
      multiplier = 1_000_000_000;
    }

    const totalCycles = cycleCount * multiplier;

    const execMhzNum = Number.parseFloat(executionMhz);
    const provMhzNum = Number.parseFloat(provingMhz);

    if (totalCycles <= 0 || Number.isNaN(execMhzNum) || execMhzNum <= 0 || Number.isNaN(provMhzNum) || provMhzNum <= 0) {
      return { estimatedExecutionTime: 0, estimatedProvingTime: 0 };
    }
    const executionTime = totalCycles / (execMhzNum * 1_000_000);
    const provingTime = totalCycles / (provMhzNum * 1_000_000);
    return {
      estimatedExecutionTime: executionTime,
      estimatedProvingTime: provingTime,
    };
  }, [cycleCount, cycleUnit, executionMhz, provingMhz]);

  return (
    <div className="my-8 rounded-lg border border-[var(--vocs-color_border)] p-6">
      <div className="space-y-4">
        {/* Cycle Count Input */}
        <div>
          <label
            htmlFor="cycleCount"
            className="mb-1 block text-sm"
          >
            Cycle Count
          </label>
          <div className="flex items-center space-x-2">
            <input
              id="cycleCount"
              value={cycleCount}
              onChange={(e) => handleNumericInput(e, setCycleCount)}
              className="w-full rounded border border-[var(--vocs-color_border)] px-3 py-2"
              placeholder="Enter cycle count"
            />
            <div className="relative">
              <select
                id="cycleUnit"
                value={cycleUnit}
                onChange={(e) => setCycleUnit(e.target.value)}
                className="rounded border border-[var(--vocs-color_border)] bg-white px-3 py-2 pr-10"
                style={{ appearance: 'none', WebkitAppearance: 'none', MozAppearance: 'none' }}
              >
                <option value="cycles">Cycles</option>
                <option value="mcycles">MCycles</option>
                <option value="gcycles">GCycles</option>
              </select>
              <div className="pointer-events-none absolute inset-y-0 right-3 flex items-center">
                <svg className="h-4 w-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" />
                </svg>
              </div>
            </div>
          </div>
        </div>

        {/* Advanced Settings */}
        <div>
          <button
            type="button"
            onClick={() => setShowAdvanced(!showAdvanced)}
            className="flex items-center gap-1 text-gray-600 text-sm hover:text-gray-800"
          >
            {showAdvanced ? (
                <svg xmlns="http://www.w3.org/2000/svg" className="h-4 w-4" viewBox="0 0 20 20" fill="currentColor"><path fillRule="evenodd" d="M14.707 12.707a1 1 0 01-1.414 0L10 9.414l-3.293 3.293a1 1 0 01-1.414-1.414l4-4a1 1 0 011.414 0l4 4a1 1 0 010 1.414z" clipRule="evenodd" /></svg>
            ) : (
                <svg xmlns="http://www.w3.org/2000/svg" className="h-4 w-4" viewBox="0 0 20 20" fill="currentColor"><path fillRule="evenodd" d="M5.293 7.293a1 1 0 011.414 0L10 10.586l3.293-3.293a1 1 0 111.414 1.414l-4 4a1 1 0 01-1.414 0l-4-4a1 1 0 010-1.414z" clipRule="evenodd" /></svg>
            )}
            Advanced Settings
          </button>
          {showAdvanced && (
            <div className="mt-2 space-y-4 rounded border border-[var(--vocs-color_border)] bg-gray-50 p-4">
               <div>
                <label htmlFor="executionMhz" className="mb-1 block font-medium text-sm">Execution MHz</label>
                <input
                  id="executionMhz"
                  value={executionMhz}
                  onChange={(e) => handleDecimalInput(e, setExecutionMhz)}
                  className="w-full rounded border border-[var(--vocs-color_border)] px-3 py-2"
                />
              </div>
              <div>
                <label htmlFor="provingMhz" className="mb-1 block font-medium text-sm">Proving MHz</label>
                <input
                  id="provingMhz"
                  value={provingMhz}
                  onChange={(e) => handleDecimalInput(e, setProvingMhz)}
                  className="w-full rounded border border-[var(--vocs-color_border)] px-3 py-2"
                />
              </div>
            </div>
          )}
        </div>

        {/* Estimated Times */}
        <div className="pt-4">
            <h4 className="mb-2 font-medium">Estimations</h4>
            <div className="rounded border border-[var(--vocs-color_border)] bg-gray-50 p-4">
                <dl className="space-y-2 text-sm">
                <div className="flex items-center justify-between">
                    <dt>Estimated Execution Time:</dt>
                    <dd className="font-mono">
                    {estimatedExecutionTime.toFixed(4)} seconds
                    </dd>
                </div>
                <div className="flex items-center justify-between">
                    <dt>Estimated Proving Time:</dt>
                    <dd className="font-mono">
                    {estimatedProvingTime.toFixed(4)} seconds
                    </dd>
                </div>
                </dl>
            </div>
        </div>
      </div>
    </div>
  );
}

