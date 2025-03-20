import path from "node:path";
import biomePlugin from "vite-plugin-biome";
import VitePluginSitemap from "vite-plugin-sitemap";
import { defineConfig } from "vocs";

const SIDEBAR_CONFIG_OLD = [
  {
    text: "✨ Introduction",
    items: [
      {
        text: "Why Boundless?",
        link: "/introduction/why-boundless",
      },
      {
        text: "What is Boundless?",
        link: "/introduction/what-is-boundless",
        collapsed: true,
        items: [
          {
            text: "Extensions",
            link: "/introduction/extensions",
          },
        ],
      },
      {
        text: "Proof Lifecycle",
        link: "/introduction/proof-lifecycle",
      },
    ],
  },
  {
    text: "🏋️ Build",
    items: [
      {
        text: "Build a Program",
        link: "/build/build-a-program",
      },
      {
        text: "Request a Proof",
        link: "/build/request-a-proof",
        collapsed: true,
        items: [
          {
            text: "Pricing a Request",
            link: "/build/pricing-a-request",
          },
          {
            text: "Troubleshooting",
            link: "/build/troubleshooting-a-request",
          },
        ],
      },
      {
        text: "Use a Proof",
        link: "/build/use-a-proof",
      },
    ],
  },
  {
    text: "🧪 Prove",
    items: [
      {
        text: "Becoming a Prover",
        link: "/prove/becoming-a-prover",
      },
      {
        text: "Requirements",
        link: "/prove/requirements",
      },
      {
        text: "Quick Start",
        link: "/prove/quick-start",
      },
      {
        text: "Running a Boundless Prover",
        link: "/prove/proving-stack",
        collapsed: true,
        items: [
          {
            text: "The Boundless Proving Stack",
            link: "/prove/proving-stack",
          },
          {
            text: "Broker Configuration & Operation",
            link: "/prove/broker",
          },
          {
            text: "Monitoring",
            link: "/prove/monitoring",
          },
          {
            text: "Performance Optimization",
            link: "/prove/performance-optimization",
          },
        ],
      },
    ],
  },
  {
    text: "🧠 Advanced & References",
    items: [
      {
        text: "Deployments",
        link: "/deployments",
      },
      {
        text: "Smart Contracts",
        link: "/smart-contracts",
      },
      {
        text: "Terminology",
        link: "/terminology",
      },
      {
        text: "Bento Technical Design",
        link: "/bento-technical-design",
      },
    ],
  },
];


const SIDEBAR_CONFIG = [
  {
    text: "Introduction",
    items: [
      {
        text: "Why use Boundless?",
        link: "/developers/why",
      },
      {
        text: "What is Boundless?",
        link: "/developers/what",
      },
      {
        text: "Proof Lifecycle",
        link: "/developers/proof-lifecycle",
      },
      {
        text: "Terminology",
        link: "/developers/terminology",
      },
    ],
  },
  {
    text: "For Rust Devs",
    items: [
      {
        text: "Quick Start",
        link: "/developers/rust/quick-start",
      },
      {
        text: "Tutorials",
        collapsed: false,
        items: [
          {
            text: "Build a Program",
            link: "/developers/rust/tutorials/build",
          },
          {
            text: "Request a Proof",
            link: "/developers/rust/tutorials/request",
          },
          {
            text: "Pricing a Request",
            link: "/developers/rust/tutorials/pricing",
          },
          {
            text: "Troubleshooting",
            link: "/developers/rust/tutorials/troubleshooting",
          },
        ],
      },
      {
        text: "Dev Tooling",
        collapsed: false,
        items: [
          {
            text: "Boundless SDK",
            link: "/developers/rust/tooling/sdk",
          },
          {
            text: "Boundless CLI",
            link: "/developers/rust/tooling/cli",
          },
        ],
      },
    ],
  },
  {
    text: "For Solidity Devs",
    items: [
      {
        text: "Smart Contracts",
        collapsed: false,
        items: [
          {
            text: "Verify a Boundless Proof",
            link: "/developers/solidity/verify-a-proof",
          },
          {
            text: "Boundless Smart Contracts",
            link: "/developers/solidity/smart-contract-docs",
          },
          {
            text: "RISC Zero Verifier Contracts",
            link: "/developers/solidity/verifier-contracts",
          },
          {
            text: "Chains & Deployments",
            link: "/developers/solidity/deployments",
          },
        ],
      },
      {
        text: "ZK Coprocessing",
        collapsed: false,
        items: [
          {
            text: "Scale Ethereum now with ZK",
            link: "/developers/solidity/zk-coprocessing",
          },
          {
            text: "Steel: Boundless Coprocessing for EVM apps",
            collapsed: true,
            items: [
              {
                text: "Quick Start",
                link: "/developers/solidity/steel/quick-start",
              },
              {
                text: "What is Steel?",
                link: "/developers/solidity/steel/what-is-steel",
              },
              {
                text: "How does Steel work?",
                link: "/developers/solidity/steel/how-it-works",
              },
              {
                text: "Steel Commitments",
                link: "/developers/solidity/steel/commitments",
              },
              {
                text: "Steel History",
                link: "/developers/solidity/steel/history",
              },
              {
                text: "Proving Events",
                link: "/developers/solidity/steel/events",
              },
            ],
          },
        ],
      },
    ],
  },
  {
    text: "For Rollup Devs",
    items: [
      {
        text: "Why Kailua?",
        link: "/developers/rollups/why",
      },
      {
        text: "Quick Start Guide",
        link: "/developers/rollups/quick-start",
      },
      {
        text: "Kailua Book",
        link: "#",
      },
    ],
  },
];

// const SIDEBAR_TOPNAV = [
//   {
//     text: "For Developers"
//     link: "/developers/why",
//   },
//   {
//     text: "For Provers"
//     link: "/provers/becoming-a-prover",
//   }
// ];

export function generateSitemap() {
  function extractRoutes(items): string[] {
    return items.flatMap((item) => {
      const routes: string[] = [];

      if (item.link) {
        routes.push(item.link);
      }

      if (item.items) {
        routes.push(...extractRoutes(item.items));
      }

      return routes;
    });
  }

  return VitePluginSitemap({
    hostname: "https://docs.beboundless.xyz",
    dynamicRoutes: extractRoutes(SIDEBAR_CONFIG),
    changefreq: "weekly",
    outDir: "site/dist",
  });
}

export default defineConfig({
  logoUrl: "/logo.svg",
  font: {
    mono: {
      google: "Ubuntu Mono",
    },
  },
  vite: {
    plugins: [generateSitemap(), biomePlugin()],
    resolve: {
      alias: {
        "lightgallery/fonts": path.resolve(__dirname, "node_modules/lightgallery/fonts"),
        "lightgallery/images": path.resolve(__dirname, "node_modules/lightgallery/images"),
      },
    },
    server: {
      fs: {
        allow: ["node_modules/lightgallery"],
      },
    },
  },
  sidebar: SIDEBAR_CONFIG,
  topNav: [
    { text: "Explorer", link: "https://explorer.beboundless.xyz" },
    { text: "Help", link: "https://t.me/+E9J7zgtyoTVlNzk1" },
    /*{
      text: process.env.LATEST_TAG || "Latest",
      items: [
        {
          text: "Releases",
          link: "https://github.com/boundless-xyz/boundless/releases",
        },
      ],
    },*/
  ],
  socials: [
    /*{
      icon: "github",
      link: "https://github.com/boundless-xyz",
    },*/
    {
      icon: "x",
      link: "https://x.com/boundless_xyz",
    },
  ],
  rootDir: "site",
  title: "Boundless Docs",
  /*logoUrl: {
    light: "/logo.png",
    dark: "/logo-dark.png",
  },*/
  theme: {
    accentColor: {
      light: "#537263", // Forest - primary accent for light mode
      dark: "#AED8C4", // Leaf - lighter accent for dark mode
    },
    variables: {
      color: {
        backgroundDark: {
          light: "#EFECE3", // Sand
          dark: "#1e1d1f",
        },
        background: {
          light: "#FFFFFF",
          dark: "#232225",
        },
      },
      content: {
        width: "calc(90ch + (var(--vocs-content_horizontalPadding) * 2))",
      },
    },
  },
  iconUrl: {
    light: "/favicon.svg",
    dark: "/favicon.svg",
  },
  banner: {
    dismissable: true,
    content:
      "BREAKING: Boundless is opening the allowlist for infrastructure companies to start proving, please fill out this [form](https://docs.google.com/forms/d/e/1FAIpQLScr5B3TZfzLKIb0Hk6oqiMMXdRh4cwpTlczi_zGqdwabvbrfw/viewform) to apply for access. See the new [proving docs](/prove/becoming-a-prover) for more info.",
  },
  ogImageUrl: "https://docs.beboundless.xyz/og.png",
});
