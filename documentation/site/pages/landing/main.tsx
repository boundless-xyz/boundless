export default function Main() {
  return (
    <div className="fixed inset-0 bg-[url('/bg-light.jpg')] bg-background bg-center bg-cover bg-no-repeat p-8 pt-[60px]">
      <div className="relative z-10 flex animate-fade-in flex-col items-center justify-center pt-16 pb-24">
        <h1 className="mb-4 text-center font-bold text-5xl">
          Build <span className="font-normal font-serif">where you are,</span> with the power of ZK.
        </h1>

        <img
          className="pointer-events-none my-8 animate-flickerAndFloat opacity-90 drop-shadow-[0_20px_50px_rgba(0,0,0,0.25)] dark:drop-shadow-[0_20px_50px_rgba(255,255,255,0.1)]"
          src="/cubes.png"
          width={300}
          height={300}
          alt="cubes"
        />

        <div className="mt-4 flex gap-4">
          <a
            className="flex h-9 w-fit items-center whitespace-pre rounded-[var(--vocs-borderRadius\_4)] border border-[var(--vocs-color\_borderAccent)] bg-[var(--vocs-color\_backgroundAccent)] px-4 font-medium text-[var(--vocs-color\_backgroundAccentText)] text-sm transition-colors duration-100"
            href="/build/build-a-program"
          >
            Quick Start
          </a>
          <a
            className="flex h-9 w-fit items-center whitespace-pre rounded-[var(--vocs-borderRadius\_4)] border border-[var(--vocs-color\_border)] bg-[var(--vocs-color\_background4)] px-4 font-medium text-[var(--vocs-color\_text)] text-sm transition-colors duration-100 "
            href="/introduction/why-boundless"
          >
            Why Boundless?
          </a>
        </div>
      </div>
      <img className="pointer-events-none absolute right-24 bottom-24 z-0" src="/ys.svg" alt="decorators" />
    </div>
  );
}
