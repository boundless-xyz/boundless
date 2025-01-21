import Footer from "../../footer";

export default function Main() {
  return (
    <div className="fixed inset-0 overflow-auto bg-[url('/bg-light.jpg')] bg-background bg-center bg-cover bg-no-repeat p-8 pt-[60px]">
      <div className="relative z-10 flex animate-fade-in flex-col items-center justify-center pt-16 text-center">
        <div className="container max-w-screen-lg pb-40">
          <h1 className="mb-4 font-bold text-5xl text-neutral-900">
            Build <span className="font-normal font-serif">where you are,</span> with the power of ZK.
          </h1>

          <img
            className="pointer-events-none mx-auto mt-12 mb-10 animate-flickerAndFloat opacity-85"
            src="/cubes.png"
            width={300}
            height={350}
            alt="cubes"
          />

          <h2 className="mb-6 text-neutral-800 text-xl">
            <strong className="font-bold">Boundless</strong> is a protocol that brings ZK to every chain, transforming
            blockchain's greatest constraint into its greatest strength. By moving from computational scarcity to
            abundance, we enable unlimited execution while preserving each chain's security. With Boundless, developers
            can write and deploy sophisticated applications once deemed impossible. Build without worrying about
            infrastructure, execution limits, or compute overhead.
          </h2>

          <h2 className="mb-16 text-neutral-800 text-xl">
            Our <strong className="font-bold">Core Services</strong> handle proof generation, aggregation, and
            settlement, while <strong className="font-bold">Extensions</strong> like Steel and Kailua unlock
            unprecedented cost savings and speed—all using the tools and languages you already know.
          </h2>

          <h2 className="mb-16 text-neutral-900 text-xl">Focus on building while Boundless handles the rest.</h2>
          
          <div className="flex justify-center gap-4">
            <div className="group relative">
              {/* Base button that shows when not hovered */}
              <div className="pointer-events-none absolute inset-0 flex h-14 w-[180px] items-center justify-center whitespace-pre rounded-lg border border-[var(--vocs-color\_borderAccent)] bg-[var(--vocs-color\_backgroundAccent)] px-4 font-medium text-[var(--vocs-color\_backgroundAccentText)] text-xl shadow-2xl transition-all duration-300 group-hover:opacity-0">
                Quick Start
              </div>
          
              {/* Split buttons that show on hover */}
              <div className="flex h-14 w-[180px] items-center rounded-lg opacity-0 transition-all duration-300 group-hover:opacity-100">
                <a
                  href="/build/build-a-program"
                  className="flex h-full w-1/2 items-center justify-center rounded-l-lg border-[var(--vocs-color_borderAccent)] border-r bg-[var(--vocs-color\_backgroundAccent)] font-medium text-[var(--vocs-color\_backgroundAccentText)] transition-colors duration-100"
                >
                  Builder
                </a>
                <a
                  href="/prove/quick-start"
                  className="flex h-full w-1/2 items-center justify-center rounded-r-lg bg-[var(--vocs-color\_backgroundAccent)] font-medium text-[var(--vocs-color\_backgroundAccentText)] transition-colors duration-100"
                >
                  Prover
                </a>
              </div>
          
              {/* Shared border */}
              <div className="pointer-events-none absolute inset-0 rounded-lg border border-[var(--vocs-color\_borderAccent)]" />
            </div>
          
            <a
              className="flex h-14 w-fit items-center whitespace-pre rounded-lg bg-[var(--vocs-color\_background4)] px-4 font-medium text-[var(--vocs-color\_text)] text-xl shadow-2xl transition-colors duration-100"
              href="/introduction/why-boundless"
            >
              Why Boundless?
            </a>
          </div>
        </div>
        <div className="mt-16">
          <Footer />
        </div>

        <img className="pointer-events-none absolute right-24 bottom-24 z-0" src="/ys.svg" alt="decorators" />
      </div>
    </div>
  );
}
