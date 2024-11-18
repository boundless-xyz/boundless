import VitePluginCssMediaSplitter from "css-media-splitter/vite-plugin";
import biomePlugin from "vite-plugin-biome";
import VitePluginSitemap from "vite-plugin-sitemap";
import { defineConfig } from "vocs";

const SIDEBAR_CONFIG = [
  {
    text: "Market",
    items: [
      {
        text: "Introduction",
        link: "/market/introduction",
      },
      {
        text: "Boundless Market RFC",
        link: "/market/boundless-market-rfc",
      },
      {
        text: "Market Matching Design",
        link: "/market/market-matching-design",
      },
      {
        text: "Local Development",
        link: "/market/local-development",
      },
      {
        text: "Public Deployments",
        link: "/market/public-deployments",
      },
    ],
  },
  {
    text: "Requestor Manual",
    items: [
      {
        text: "Introduction",
        link: "/requestor-manual/introduction",
      },
      {
        text: "Broadcasting Requests",
        link: "/requestor-manual/broadcasting-requests",
      },
    ],
  },
  {
    text: "Prover Manual",
    items: [
      {
        text: "Introduction",
        link: "/prover-manual/introduction",
      },
      {
        text: "Bento",
        items: [
          {
            text: "Introduction",
            link: "/prover-manual/bento/introduction",
          },
          {
            text: "Running",
            link: "/prover-manual/bento/running",
          },
          {
            text: "Performance Tuning",
            link: "/prover-manual/bento/performance-tuning",
          },
        ],
      },
      {
        text: "Broker",
        items: [
          {
            text: "Introduction",
            link: "/prover-manual/broker/introduction",
          },
          {
            text: "Configuration",
            link: "/prover-manual/broker/configuration",
          },
          {
            text: "Operation",
            link: "/prover-manual/broker/operation",
          },
        ],
      },
      {
        text: "Monitoring",
        link: "/prover-manual/monitoring",
      },
    ],
  },
  {
    text: "Smart Contracts Documentation",
    link: "/smart-contracts",
  },
  {
    text: "Reference",
    link: "/reference",
  },
  {
    text: "Glossary",
    link: "/glossary",
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

  return VitePluginSitemap({
    hostname: "https://docs.beboundless.xyz",
    dynamicRoutes: extractRoutes(SIDEBAR_CONFIG),
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
    plugins: [generateSitemap(), VitePluginCssMediaSplitter(), biomePlugin()],
  },
  sidebar: SIDEBAR_CONFIG,
  topNav: [
    { text: "Indexer", link: "https://indexer.beboundless.xyz" },
    {
      text: process.env.LATEST_TAG || "Latest",
      items: [
        {
          text: "Releases",
          link: "https://github.com/boundless-xyz/boundless/releases",
        },
      ],
    },
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
  logoUrl: {
    light: "/logo.png",
    dark: "/logo-dark.png",
  },
  theme: {
    accentColor: {
      light: "#474444", // Forest - primary accent
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
    light: "/favicon.png",
    dark: "/favicon-dark.png",
  },
  // banner: "Read the [Boundless Blog Article](https://risczero.com/blog/boundless-the-verifiable-compute-layer)",
  editLink: {
    pattern: "https://github.com/boundless-xyz/boundless/edit/main/documentation/site/pages/:path",
    text: "Edit on GitHub",
  },
  ogImageUrl:
    "https://vocs.dev/api/og?logo=https://boundless-documentation.vercel.app/logo-dark.png&title=%title&description=%description",
});
