import path from "node:path";
import biomePlugin from "vite-plugin-biome";
import VitePluginSitemap from "vite-plugin-sitemap";
import { defineConfig } from "vocs";

const SIDEBAR_CONFIG = {
  '/protocol/': [
    {
      text: "Main Navigation",
      items: [
        {
          text: "Protocol",
          link: "/protocol",
      },
      {
        text: "zkVM",
        link: "/protocol/high-level",
      },
      {
        text: "Apps",
        link: "/protocol/why-boundless",
      },
      {
        text: "Rollups",
        link: "/protocol/what-is-boundless",
        }
      ]
    },
    {
      text: "App Developers",
      items: [
        {
          text: "Request a Proof",
          link: "/protocol/app-developers/request-a-proof",
        },
        {
          text: "Pricing a Proof",
          link: "/protocol/app-developers/pricing-a-proof",
        },
        {
          text: "Consuming (Use) a Proof",
          link: "/protocol/app-developers/consuming-a-proof",
        },
        {
          text: "Market Contract Addresses",
          link: "/protocol/app-developers/market-contract-addresses",
        }
      ]
    },
    {
      text: "Provers",
      items: [
        {
          text: "Becoming a Prover",
          link: "/protocol/provers/becoming-a-prover",
        },
        {
          text: "Requirements",
          link: "/protocol/provers/requirements",
        },
        {
          text: "Quick Start",
          link: "/protocol/provers/quick-start",
        },
        {
          text: "Running a Prover",
          link: "/protocol/provers/running-a-prover",
        },
        {
          text: "Broker Config",
          link: "/protocol/provers/broker-config",
        },
        {
          text: "Monitoring",
          link: "/protocol/provers/monitoring",
        },
        {
          text: "Performance",
          link: "/protocol/provers/performance",
        },
        {
          text: "Troubleshooting",
          link: "/protocol/provers/troubleshooting",
        }
      ]
    },
    {
      text: "References",
      items: [
        {
          text: "Bento Technical Design",
          link: "/protocol/references/bento-technical-design",
        },
        {
          text: "Market Contract Addresses",
          link: "/protocol/references/market-contract-addresses",
        }
      ]
    }
  ],
  '/zkvm/': [
    {
      text: "Main Navigation",
      items: [
        {
          text: "Protocol",
          link: "/protocol",
      },
      {
        text: "zkVM",
        link: "/protocol/high-level",
      },
      {
        text: "Apps",
        link: "/protocol/why-boundless",
      },
      {
        text: "Rollups",
        link: "/protocol/what-is-boundless",
        }
      ]
    },
    {
      text: "zkVM",
      link: "/zkvm",
    },
    {
      text: "Hello World R0VM",
      link: "/zkvm/hello-world",
    },
    {
      text: "Build A Program",
      link: "/zkvm/build-a-program",
    },
    {
      text: "RISC0 Documentation",
      items: [
        {
          text: "RISC Zero Docs",
          link: "https://dev.risczero.com/documentation",
        },
        {
          text: "Getting Started",
          link: "https://dev.risczero.com/getting-started",
        },
        {
          text: "zkVM",
          link: "https://dev.risczero.com/zkvm",
        }
      ]
    }
  ],
  '/apps/': [
    {
      text: "Main Navigation",
      items: [
        {
          text: "Protocol",
          link: "/protocol",
      },
      {
        text: "zkVM",
        link: "/protocol/high-level",
      },
      {
        text: "Apps",
        link: "/protocol/why-boundless",
      },
      {
        text: "Rollups",
        link: "/protocol/what-is-boundless",
        }
      ]
    },
    {
      text: "Steel DeFi",
      items: [
        {
          text: "Introduction to Steel",
          link: "/apps/steel/introduction",
        },
        {
          text: "Getting Started with Steel in 10 minutes",
          link: "/apps/steel/getting-started",
        },
        {
          text: "Steel API Reference",
          link: "/apps/steel/api-reference",
        }
      ]
    },
    {
      text: "Tutorials",
      items: [
        {
          text: "Building a DeFi Application",
          link: "/apps/tutorials/defi-application",
        },
        {
          text: "Private Voting System",
          link: "/apps/tutorials/private-voting",
        },
        {
          text: "Zero-Knowledge Authentication",
          link: "/apps/tutorials/zk-authentication",
        }
      ]
    }
  ],
  '/rollups/': [
    {
      text: "Main Navigation",
      items: [
        {
          text: "Protocol",
          link: "/protocol",
      },
      {
        text: "zkVM",
        link: "/protocol/high-level",
      },
      {
        text: "Apps",
        link: "/protocol/why-boundless",
      },
      {
        text: "Rollups",
        link: "/protocol/what-is-boundless",
        }
      ]
    },
    {
      text: "Rollups",
      link: "/rollups",
    },
    {
      text: "Getting Started",
      link: "/rollups/getting-started",
    },
    {
      text: "Bridging",
      items: [
        {
          text: "Bridging Overview",
          link: "/rollups/bridging/overview",
        },
        {
          text: "Deposit Assets",
          link: "/rollups/bridging/deposit",
        },
        {
          text: "Withdraw Assets",
          link: "/rollups/bridging/withdraw",
        }
      ]
    },
    {
      text: "Tutorials",
      items: [
        {
          text: "Deploy a Smart Contract",
          link: "/rollups/tutorials/deploy-contract",
        },
        {
          text: "Build a dApp on Boundless Rollup",
          link: "/rollups/tutorials/build-dapp",
        },
        {
          text: "Integrating with Existing Applications",
          link: "/rollups/tutorials/integration",
        }
      ]
    }
  ]
};

export function generateSitemap() {
  function extractRoutes(items): string[] {
    if (!Array.isArray(items)) {
      let routes: string[] = [];
      for (const key in items) {
        routes = routes.concat(extractRoutes(items[key]));
      }
      return routes;
    }

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
    // { text: "Help", link: "https://t.me/+E9J7zgtyoTVlNzk1" },
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
  banner: {
    dismissable: true,
    content:
      "BREAKING: Boundless is opening the allowlist for infrastructure companies to start proving, please fill out this [form](https://docs.google.com/forms/d/e/1FAIpQLScr5B3TZfzLKIb0Hk6oqiMMXdRh4cwpTlczi_zGqdwabvbrfw/viewform) to apply for access. See the new [proving docs](/prove/becoming-a-prover) for more info.",
  },
  ogImageUrl: "https://docs.beboundless.xyz/og.png",
});
