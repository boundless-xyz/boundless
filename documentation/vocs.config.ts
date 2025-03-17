import path from "node:path";
import biomePlugin from "vite-plugin-biome";
import VitePluginSitemap from "vite-plugin-sitemap";
import { defineConfig } from "vocs";

// Protocol sidebar
const PROTOCOL_SIDEBAR = [
  {
    text: "Introduction",
    items: [
      {
        text: "Why Boundless?",
        link: "/introduction/why-boundless",
      },
      {
        text: "What is Boundless?",
        link: "/introduction/what-is-boundless",
      },
    ],
  },
  {
    text: "Proof Lifecycle",
    link: "/introduction/proof-lifecycle",
  },
  {
    text: "Prove",
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
      {
        text: "Troubleshooting",
        link: "/build/troubleshooting-a-request",
      },
    ],
  },
  {
    text: "Pricing a Request",
    link: "/build/pricing-a-request",
  },
  {
    text: "Deployments",
    link: "/deployments",
  },
  {
    text: "Smart Contracts",
    link: "/smart-contracts",
  },
  {
    text: "Bento Technical Design",
    link: "/bento-technical-design",
  },
  {
    text: "Terminology",
    link: "/terminology",
  },
];

// zkVM sidebar
const ZKVM_SIDEBAR = [
  {
    text: "Build",
    items: [
      {
        text: "Build a Program",
        link: "/build/build-a-program",
      },
      {
        text: "Request a Proof",
        link: "/build/request-a-proof",
      },
      {
        text: "Use a Proof",
        link: "/build/use-a-proof",
      },
    ],
  },
];

// DeFi sidebar
const DEFI_SIDEBAR = [
  {
    text: "Steel",
    items: [
      {
        text: "Quick Start",
        link: "/defi/steel/quick-start",
      },
      {
        text: "Integrations",
        link: "/defi/steel/integrations",
      },
      {
        text: "Reference Contracts",
        link: "/defi/steel/reference-contracts",
      },
      {
        text: "Examples",
        link: "/defi/steel/examples",
      },
    ],
  },
];

// Rollups sidebar
const ROLLUPS_SIDEBAR = [
  {
    text: "Bridging",
    link: "/rollups/bridging",
  },
  {
    text: "L2 Integration",
    link: "/rollups/l2-integration",
  },
];

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

  const allSidebars = [PROTOCOL_SIDEBAR, ZKVM_SIDEBAR, DEFI_SIDEBAR, ROLLUPS_SIDEBAR];
  const allRoutes = allSidebars.flatMap(sidebar => extractRoutes(sidebar));

  return VitePluginSitemap({
    hostname: "https://docs.beboundless.xyz",
    dynamicRoutes: allRoutes,
    changefreq: "weekly",
    outDir: "site/dist",
  });
}

export default defineConfig({
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
  // Configure conditional sidebars based on URL pattern
  sidebar: {
    "/introduction/": PROTOCOL_SIDEBAR,
    "/prove/": PROTOCOL_SIDEBAR,
    "/deployments": PROTOCOL_SIDEBAR,
    "/smart-contracts": PROTOCOL_SIDEBAR,
    "/terminology": PROTOCOL_SIDEBAR,
    "/bento-technical-design": PROTOCOL_SIDEBAR,
    "/build/": ZKVM_SIDEBAR,
    "/defi/": DEFI_SIDEBAR,
    "/rollups/": ROLLUPS_SIDEBAR,
  },
  // Add the four main sections to the top navigation
  topNav: [
    { text: "Protocol", link: "/introduction/why-boundless" },
    { text: "zkVM", link: "/build/build-a-program" },
    { text: "DeFi", link: "/defi/steel/quick-start" },
    { text: "Rollups", link: "/rollups/bridging" },
    { text: "Explorer", link: "https://explorer.beboundless.xyz" },
    { text: "Help", link: "https://t.me/+E9J7zgtyoTVlNzk1" },
  ],
  socials: [
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
  banner: {
    dismissable: true,
    content:
      "BREAKING: Boundless is opening the allowlist for infrastructure companies to start proving, please fill out this [form](https://docs.google.com/forms/d/e/1FAIpQLScr5B3TZfzLKIb0Hk6oqiMMXdRh4cwpTlczi_zGqdwabvbrfw/viewform) to apply for access. See the new [proving docs](/prove/becoming-a-prover) for more info.",
  },
  ogImageUrl: "https://docs.beboundless.xyz/og.png",
});
