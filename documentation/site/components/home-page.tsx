// Code for this homepage is modified from: https://github.com/ensdomains/docs, available under CC0.

const navigation = [
  {
    title: "For Developers",
    links: [
      ["Build a Program", "/developers/tutorials/build"],
      ["Proof Lifecycle", "/developers/proof-lifecycle"],
      ["Request a Proof", "/developers/tutorials/request"],
      ["Tracking a Request", "developers/tutorials/tracking"],
      ["Use a Proof", "developers/tutorials/use"],
    ],
  },
  {
    title: "For Provers",
    links: [
      ["The Boundless Proving Stack", "/provers/proving-stack"],
      ["Broker Config", "/provers/broker"],
      ["Monitoring", "/provers/monitoring"],
      ["Performance Optimization", "/provers/performance-optimization"],
    ],
  },
  {
    title: "Tooling",
    links: [
      ["Boundless Mainnet Explorer", "https://explorer.boundless.network/orders"],
      ["Boundless Testnet Explorer", "https://explorer.testnet.boundless.network/orders"],
      ["Boundless CLI", "/developers/tooling/sdk"],
      ["Boundless SDK", "/developers/tooling/sdk"],
      ["Bento CLI", "/developers/tooling/cli"],
    ],
  },
  {
    title: "Tutorials",
    links: [
      ["Callbacks", "/developers/tutorials/callbacks"],
      ["Proof Composition", "/developers/tutorials/proof-composition"],
      ["Proof Types", "/developers/tutorials/proof-types"],
      ["Setting up a Trusted Prover", "/developers/tutorials/sensitive-inputs"],
      ["Smart Contract Requestors", "/developers/tutorials/smart-contract-requestor"],
      ["Migrating from Bonsai", "/developers/tutorials/bonsai"],
    ],
  },
  {
    title: "Reference",
    links: [
      ["Chains & Deployments", "/dao"],
      ["Smart Contract Docs", "/dao/constitution"],
      ["Verifier Contracts", "/dao/foundation"],
      ["Bento Technical Design", "/dao/token"],
    ],
  },
  {
    title: "External",
    links: [
      ["Boundless Staking Portal", "https://staking.boundless.network"],
      ["Steel Crate Docs", "https://boundless-xyz.github.io/steel/risc0_steel/index.html"],
      ["Kailua Book", "https://boundless-xyz.github.io/kailua/"],
      ["Boundless DAO", "https://app.aragon.org/dao/ethereum-mainnet/boundless.dao.eth"],
    ],
  },
];

const videos = [
  {
    title: 'Boundless Mainnet Launch',
    href: 'https://x.com/boundless_xyz/status/1968004476758863925/video/1',
    thumbnail: '/launch-video-thumbnail.jpg',
  },
  {
    title: 'Boundless Prover Quickstart (July 2025)',
    href: 'https://www.youtube.com/watch?v=MZqU-J-fW2M',
    thumbnail: '/prover-quick-start-thumbnail.jpg',
  }
]

export default function HomePage() {
  return (
    <>
      <div className="bg-[var(--vocs-color_backgroundDark)] py-10">
        <div className="mx-auto flex max-w-4xl flex-col gap-4">
          <h1 className="font-semibold text-2xl sm:text-2xl">Get Started</h1>
          <div className="flex flex-col gap-3 sm:flex-row">
            <a
              href="/developers/quick-start"
              className="rounded-lg border border-neutral-400 px-4 py-1 font-bold text-[20px] hover:bg-neutral-100 dark:border-neutral-700 dark:hover:bg-neutral-800"
            >
              Developers
            </a>
            <a
              href="/provers/quick-start"
              className="rounded-lg border border-neutral-400 px-4 py-1 font-bold text-[20px] hover:bg-neutral-100 dark:border-neutral-700 dark:hover:bg-neutral-800"
            >
              Provers
            </a>
            <a
              href="/zkc/quick-start"
              className="rounded-lg border border-neutral-400 px-4 py-1 font-bold text-[20px] hover:bg-neutral-100 dark:border-neutral-700 dark:hover:bg-neutral-800"
            >
              $ZKC
            </a>
            <a
              href="/zkc/mining/quick-start"
              className="rounded-lg border border-neutral-400 px-4 py-1 font-bold text-[20px] hover:bg-neutral-100 dark:border-neutral-700 dark:hover:bg-neutral-800"
            >
              ZK Mining
            </a>
          </div>
        </div>
      </div>
      <div className="mx-auto grid max-w-4xl grid-cols-2 px-4 lg:grid-cols-3 lg:gap-y-10">
        {navigation.map((column) => (
          <div key={column.title}>
            <div className="font-bold">{column.title}</div>
            {column.links.map(([title, href]) => (
              <div key={title} className="flex items-center gap-3">
                <a className="vocs_Anchor !no-underline" href={href}>
                  {title}
                </a>
              </div>
            ))}
          </div>
        ))}
      </div>

      <div className="mx-auto flex max-w-4xl flex-col px-4">
        <h2 className="vocs_H2 vocs_Heading">Videos</h2>
        <div className="grid grid-cols-2 gap-6 lg:gap-y-10">
          {videos.map((video) => (
            <a
              key={video.title}
              href={video.href}
              target="_blank"
              className="overflow-hidden rounded-lg border-1 border-gray-500/50"
            >
              <img src={video.thumbnail} alt={video.title} className="w-full" />
              <span className="block p-2 font-medium leading-6">{video.title}</span>
            </a>
          ))}
        </div>
      </div>
    </>
  );
}