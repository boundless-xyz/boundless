import { useEffect, useState } from "react";
import Footer from "../../footer";

export default function Main() {
  const [render, setRender] = useState(false);

  useEffect(() => {
    const hasVisitedBefore = localStorage.getItem("hasVisitedDocs");

    if (hasVisitedBefore) {
      window.location.href = "/developers/why";
    } else {
      localStorage.setItem("hasVisitedDocs", "true");
      setRender(true);
    }
  }, []);

  if (!render) {
    return <></>;
  }

  return (
    <div className="relative z-10 flex animate-fade-in flex-col items-center justify-center text-center">
      <div className="container pb-40">
        <h1 className="mb-4 font-bold text-5xl">
          Build <span className="font-normal font-serif">where you are,</span> with the power of ZK.
        </h1>

        <img
          className="pointer-events-none mx-auto mt-12 mb-10 animate-flickerAndFloat opacity-85"
          src="/cubes.png"
          width={300}
          height={350}
          alt="cubes"
        />

        <h2 className="mb-6 text-xl">
          <strong className="font-bold">Boundless</strong> is a protocol that brings ZK to every chain, transforming
          blockchain's greatest constraint into its greatest strength. By moving from computational scarcity to
          abundance, we enable unlimited execution while preserving each chain's security. With Boundless, developers
          can write and deploy sophisticated applications once deemed impossible. Build without worrying about
          infrastructure, execution limits, or compute overhead.
        </h2>

        <h2 className="mb-16 text-xl">
          Our <strong className="font-bold">Core Services</strong> handle{" "}
          <a className="link underline" href="/introduction/proof-lifecycle">
            proof generation
          </a>
          , aggregation, and settlement, while{" "}
          <a className="link underline" href="/introduction/extensions">
            Extensions
          </a>{" "}
          like{" "}
          <a className="link underline" href="/introduction/extensions#steel">
            Steel
          </a>{" "}
          and{" "}
          <a className="link underline" href="/introduction/extensions#kailua">
            Kailua
          </a>{" "}
          unlock unprecedented cost savings and speed—all using the tools and languages you already know.
        </h2>

        <h2 className="mb-16 text-xl">Focus on building while Boundless handles the rest.</h2>

        <div className="flex justify-center">
          <a
            className="flex h-14 w-fit items-center whitespace-pre rounded-lg bg-[var(--vocs-color\_background4)] px-4 font-medium text-[var(--vocs-color\_text)] text-xl shadow-2xl transition-colors duration-100"
            href="/developers/why"
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
  );
}
