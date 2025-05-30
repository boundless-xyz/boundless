import biomePlugin from "vite-plugin-biome";
import VitePluginSitemap from "vite-plugin-sitemap";
import { defineConfig } from "vocs";

const SHARED_LINKS = [
  { text: "For Developers", link: "/developers/why" },
  { text: "For Provers", link: "/provers/quick-start" },
];

const DEVELOPERS_ITEMS = [
  {
    text: "Introduction",
    items: [
      {
        text: "What is Boundless?",
        link: "/developers/what",
      },
      {
        text: "Why use Boundless?",
        link: "/developers/why",
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
    text: "Build on Boundless",
    items: [
      {
        text: "Quick Start",
        link: "/developers/quick-start",
      },
      {
        text: "Core Concepts",
        items: [
          {
            text: "Build a Program",
            link: "/developers/tutorials/build",
          },
          {
            text: "Request a Proof",
            link: "/developers/tutorials/request",
          },
          {
            text: "Pricing a Request",
            link: "/developers/tutorials/pricing",
          },
          {
            text: "Use a Proof",
            link: "/developers/tutorials/use",
          },
          {
            text: "Troubleshooting",
            link: "/developers/tutorials/troubleshooting",
          },
        ],
      },
      {
        text: "Tutorials",
        items: [
          {
            text: "Callbacks",
            link: "/developers/tutorials/callbacks",
          },
          {
            text: "Migrating from Bonsai?",
            link: "/developers/tutorials/bonsai",
          },
          {
            text: "Proof Composition",
            link: "/developers/tutorials/proof-composition",
          },
          {
            text: "Proof Types",
            link: "/developers/tutorials/proof-types",
          },
          {
            text: "Sensitive Inputs",
            link: "/developers/tutorials/sensitive-inputs",
          },
          {
            text: "Smart Contract Requestors",
            link: "/developers/tutorials/smart-contract-requestor",
          },
        ],
      },
      {
        text: "Dev Tooling",
        items: [
          {
            text: "Boundless SDK",
            link: "/developers/tooling/sdk",
          },
          {
            text: "Boundless CLI",
            link: "/developers/tooling/cli",
          },
        ],
      },
      {
        text: "Smart Contracts",
        items: [
          {
            text: "Boundless Contracts",
            link: "/developers/smart-contracts/reference",
          },
          {
            text: "Verifier Contracts",
            link: "/developers/smart-contracts/verifier-contracts",
          },
          {
            text: "Chains & Deployments",
            link: "/developers/smart-contracts/deployments",
          },
        ],
      },
    ],
  },
  {
    text: "ZK Coprocessing with Steel",
    items: [
      {
        text: "Quick Start",
        link: "/developers/steel/quick-start",
      },
      {
        text: "What is Steel?",
        link: "/developers/steel/what-is-steel",
      },
      {
        text: "How does Steel work?",
        link: "/developers/steel/how-it-works",
      },
      {
        text: "Commitments",
        link: "/developers/steel/commitments",
      },
      {
        text: "History",
        link: "/developers/steel/history",
      },
      {
        text: "Events",
        link: "/developers/steel/events",
      },
      {
        text: "Crate Docs",
        link: "https://risc0.github.io/risc0-ethereum/risc0_steel/",
      },
    ],
  },
  {
    text: "Hybrid Rollups with OP Kailua",
    items: [
      {
        text: "Introducing OP Kailua",
        link: "/developers/kailua/how",
      },
      {
        text: "Quick Start",
        link: "/developers/kailua/quick-start",
      },
      {
        text: "OP Kailua Book",
        link: "https://risc0.github.io/kailua/",
      },
    ],
  },
];

const PROVERS_ITEMS = [
  {
    text: "Introduction",
    items: [
      {
        text: "What is Boundless?",
        link: "/provers/what",
      },
      {
        text: "Why use Boundless?",
        link: "/provers/why",
      },
      {
        text: "Proof Lifecycle",
        link: "/provers/proof-lifecycle",
      },
      {
        text: "Terminology",
        link: "/provers/terminology",
      },
    ],
  },
  {
    text: "Getting Started",
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
      {
        text: "Bento Technical Design",
        link: "/provers/bento",
      },
    ],
  },
];

const DEVELOPERS_SIDEBAR = [...SHARED_LINKS, ...DEVELOPERS_ITEMS];
const PROVERS_SIDEBAR = [...SHARED_LINKS, ...PROVERS_ITEMS];

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
  topNav: [{ text: "Boundless Explorer", link: "https://explorer.beboundless.xyz" }],
  font: {
    mono: {
      google: "Ubuntu Mono",
    },
  },
  vite: {
    plugins: [generateSitemap(), biomePlugin()],
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
    },
  },
  iconUrl: {
    light: "/favicon.svg",
    dark: "/favicon.svg",
  },
  ogImageUrl: "https://docs.beboundless.xyz/og.png",
});
