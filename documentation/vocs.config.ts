import path from "node:path";
import biomePlugin from "vite-plugin-biome";
import VitePluginSitemap from "vite-plugin-sitemap";
import { defineConfig } from "vocs";


const SHARED_LINKS = [
  { text: "Developers", link: "/developers/why" },
  { text: "Provers", link: "/provers/quick-start" },
]

const DEVELOPERS_ITEMS = [
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
    text: "For App Devs",
    items: [
      {
        text: "Quick Start",
        link: "/developers/apps/quick-start",
      },
      {
        text: "Tutorials",
        items: [
          {
            text: "Build a Program",
            link: "/developers/apps/tutorials/build",
          },
          {
            text: "Request a Proof",
            link: "/developers/apps/tutorials/request",
          },
          {
            text: "Pricing a Request",
            link: "/developers/apps/tutorials/pricing",
          },
          {
            text: "Troubleshooting",
            link: "/developers/apps/tutorials/troubleshooting",
          },
        ],
      },
      {
        text: "Dev Tooling",
        items: [
          {
            text: "Boundless SDK",
            link: "/developers/apps/tooling/sdk",
          },
          {
            text: "Boundless CLI",
            link: "/developers/apps/tooling/cli",
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
        items: [
          {
            text: "Boundless Smart Contract Reference",
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
        text: "Steel: Boundless ZK Coprocessing",
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
        text: "Quick Start",
        link: "/developers/rollups/quick-start",
      },
      {
        text: "Kailua Book",
        link: "https://risc0.github.io/kailua/",
      },
    ],
  },
];

const PROVERS_ITEMS = [
  {
    text: "Running a Prover",
    items: [
      {
        text: "Who should run a prover?",
        link: "/provers/becoming-a-prover",
      },
      {
        text: "Requirements",
        link: "/provers/requirements",
      },
      {
        text: "Quick Start",
        link: "/provers/quick-start",
      },
    ],
  },
  {
    text: "Running a Boundless Prover",
    items: [
      {
        text: "The Boundless Proving Stack",
        link: "/provers/proving-stack",
      },
      {
        text: "Broker Configuration & Operation",
        link: "/provers/broker",
      },
      {
        text: "Monitoring",
        link: "/provers/monitoring",
      },
      {
        text: "Performance Optimization",
        link: "/provers/performance-optimization",
      },
    ],
  }
];

const DEVELOPERS_SIDEBAR = [...SHARED_LINKS, ...DEVELOPERS_ITEMS]
const PROVERS_SIDEBAR = [...SHARED_LINKS, ...PROVERS_ITEMS]

export function generateSitemap() {
  const allSidebarItems = [...DEVELOPERS_SIDEBAR, ...PROVERS_SIDEBAR];
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
    dynamicRoutes: extractRoutes(allSidebarItems),
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
  sidebar: {
    "/developers/": DEVELOPERS_SIDEBAR,
    "/provers/": PROVERS_SIDEBAR,
  },
  socials: [
    {
      icon: "github",
      link: "https://github.com/boundless-xyz",
    },
    {
      icon: "x",
      link: "https://x.com/boundless_xyz",
    },
  ],
  rootDir: "site",
  title: "Boundless Docs",
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
  ogImageUrl: "https://docs.beboundless.xyz/og.png",
});
